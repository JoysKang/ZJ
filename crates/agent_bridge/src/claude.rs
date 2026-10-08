//! ACP in front of Claude Code: one `claude -p --input-format stream-json --output-format
//! stream-json` per session, the control protocol on the same pipes (the one the Agent SDK
//! uses). Parts adapted from Orbvane (MIT OR Apache-2.0, github.com/sbaruwal/orbvane,
//! `crates/acp/src/claude.rs`).
//!
//! - `session/new` starts `claude` with `--session-id`, `session/load` with `--resume`; the
//!   control `initialize` answers with the models and commands. A session whose `claude`
//!   exited starts again (`--resume`) with its next prompt.
//! - Every user message we send carries a uuid. Claude Code's `result` names the messages it
//!   answered (`user_message_uuid(s)`), so a prompt ends when its message and every steered
//!   one have been answered: a steer (`_session/steering`, priority `now`) interrupts the
//!   running cycle, which ends with a `result` of its own that is not the turn's end.
//! - `can_use_tool` control requests become `session/request_permission`; "allow for this
//!   session" is remembered per tool (per command for Bash).
//! - Tool uses become tool calls; edits carry their diff (the file now ↔ after the edit, read
//!   before Claude Code writes it), which ZJ uses for its review snapshot.
//! - Modes are Claude Code's `--permission-mode` values, without `bypassPermissions`.

use crate::{
    Event,
    acp::{self, AUTH_REQUIRED, INTERNAL, INVALID_PARAMS, NOT_FOUND},
    child::Child,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender},
};

/// (id, name, description). `bypassPermissions` is never offered.
const MODES: [(&str, &str, &str); 4] = [
    ("default", "每次询问", "编辑文件和运行命令前都先询问"),
    (
        "acceptEdits",
        "自动接受编辑",
        "编辑文件不再询问，运行命令前询问",
    ),
    ("plan", "规划", "只读和规划，批准计划后才改动"),
    ("auto", "自动", "由 Claude Code 判断哪些操作可以直接执行"),
];

const INIT: &str = "zj-init";

/// A model from the control `initialize`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Model {
    value: String,
    name: String,
    efforts: Vec<String>,
    fast: bool,
}

/// A setting change waiting for Claude Code's answer.
enum Change {
    Mode(String),
    Model(String),
    Effort(String),
    Fast(bool),
}

/// A `can_use_tool` request asked of ZJ.
struct Asked {
    session: String,
    request_id: Value,
    tool: String,
    input: Value,
    /// What "for this session" remembers.
    rule: String,
}

#[derive(Default)]
struct Session {
    id: String,
    cwd: PathBuf,
    /// Key of the running `claude` (events carry it); `None` once it exited.
    key: Option<u64>,
    claude: Option<Child>,
    /// ZJ's `session/new` / `session/load` (true) waiting for the control `initialize`.
    starting: Option<(Value, bool)>,
    /// A prompt waiting for a restarted `claude`.
    queued: Option<(Value, Value)>,
    /// ZJ's `session/prompt` waiting for its results, and the messages not yet answered.
    prompt: Option<Value>,
    outstanding: HashSet<String>,
    interrupted: bool,
    /// Permission questions open in ZJ (a steer waits behind them: priority `later`).
    asking: usize,
    allowed: HashSet<String>,
    /// Messages whose text or thinking streamed (their full copy adds nothing), the message
    /// streaming now, and the last one written (a new one starts a new paragraph).
    streamed: HashSet<String>,
    thought: HashSet<String>,
    message: Option<String>,
    last_text: Option<String>,
    /// Tool uses shown: id → tool name.
    tools: HashMap<String, String>,
    /// Tool use blocks streaming: index → (id, name, input JSON so far).
    blocks: HashMap<u64, (String, String, String)>,
    /// Files as they were when an edit's tool use was complete, by tool use id (`None`: no
    /// such file): read before Claude Code can run the tool.
    before: HashMap<String, Option<String>>,
    mode: String,
    model: Option<String>,
    effort: Option<String>,
    fast: bool,
    models: Vec<Model>,
    commands: Vec<Value>,
    /// Our control requests waiting: request id → (ZJ's request to answer, the change).
    changes: HashMap<String, (Option<Value>, Change)>,
    next_control: u64,
}

struct Bridge {
    cli: PathBuf,
    events: Sender<Event>,
    sessions: HashMap<String, Session>,
    /// `claude` key → session id.
    keys: HashMap<u64, String>,
    next_key: u64,
    requests: acp::Requests,
    asked: HashMap<i64, Asked>,
}

pub(crate) fn serve(cli: PathBuf, events: Sender<Event>, inbox: Receiver<Event>) -> i32 {
    let mut b = Bridge {
        cli,
        events,
        sessions: HashMap::new(),
        keys: HashMap::new(),
        next_key: 0,
        requests: acp::Requests::default(),
        asked: HashMap::new(),
    };
    for event in inbox {
        match event {
            Event::Client(message) => b.on_client(message),
            Event::ClientGone => break,
            Event::Cli(key, Some(message)) => b.on_claude(key, &message),
            Event::Cli(key, None) => b.claude_gone(key),
        }
    }
    // Dropping the sessions stops their `claude`.
    0
}

impl Session {
    fn update(&self, update: Value) {
        acp::update(&self.id, update);
    }

    fn send(&mut self, message: &Value) -> bool {
        self.claude.as_mut().is_some_and(|c| c.send(message))
    }

    fn control(&mut self, request: Value) -> String {
        self.next_control += 1;
        let id = format!("zj-{}", self.next_control);
        let message = json!({ "type": "control_request", "request_id": id, "request": request });
        self.send(&message);
        id
    }

    fn control_reply(&mut self, request_id: &Value, response: Value) {
        let message = json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        });
        self.send(&message);
    }

    fn current_model(&self) -> Option<&Model> {
        let value = self.model.as_deref().unwrap_or("default");
        self.models.iter().find(|m| m.value == value)
    }

    /// The model settings ZJ shows (model, effort, fast mode).
    fn configs(&self) -> Vec<Value> {
        if self.models.is_empty() {
            return Vec::new();
        }
        let models: Vec<(String, String)> = self
            .models
            .iter()
            .map(|m| (m.value.clone(), m.name.clone()))
            .collect();
        let mut out = vec![acp::select(
            "model",
            "模型",
            "model",
            self.model.as_deref().unwrap_or("default"),
            &models,
        )];
        if let Some(model) = self.current_model() {
            if !model.efforts.is_empty() {
                let mut levels = vec![("default".to_string(), "默认".to_string())];
                levels.extend(
                    model
                        .efforts
                        .iter()
                        .map(|e| (e.clone(), acp::effort_name(e))),
                );
                let current = self.effort.as_deref().unwrap_or("default");
                out.push(acp::select(
                    "effort",
                    "思考强度",
                    "thought_level",
                    current,
                    &levels,
                ));
            }
            if model.fast {
                let current = if self.fast { "on" } else { "off" };
                out.push(acp::select(
                    "fast",
                    "快速模式",
                    "model_config",
                    current,
                    &acp::on_off(),
                ));
            }
        }
        out
    }

    fn mode_state(&self) -> Value {
        acp::mode_state(&self.mode, &MODES)
    }

    /// Reply text from message `message` (a new message starts a new paragraph).
    fn agent_text(&mut self, message: &str, text: &str) {
        let mut text = text.to_string();
        if self.last_text.as_deref().is_some_and(|m| m != message) {
            text.insert_str(0, "\n\n");
        }
        self.last_text = Some(message.to_string());
        self.update(acp::text_chunk("agent_message_chunk", &text));
    }

    /// A user message with a uuid of ours; the turn waits for its answer.
    fn user_message(&mut self, content: Vec<Value>, priority: Option<&str>) -> bool {
        let uuid = acp::uuid();
        let mut message = json!({
            "type": "user",
            "uuid": uuid,
            "session_id": "",
            "parent_tool_use_id": null,
            "message": { "role": "user", "content": content },
        });
        if let Some(priority) = priority {
            message["priority"] = json!(priority);
        }
        self.outstanding.insert(uuid);
        self.send(&message)
    }
}

/// Claude Code's user content for ZJ's prompt blocks.
fn content(blocks: &Value) -> Vec<Value> {
    let prompt = acp::prompt(blocks);
    let mut out: Vec<Value> = prompt
        .images
        .into_iter()
        .map(|(mime, data)| {
            json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } })
        })
        .collect();
    if !prompt.text.is_empty() || out.is_empty() {
        out.push(json!({ "type": "text", "text": prompt.text }));
    }
    out
}

impl Bridge {
    fn session(&mut self, params: &Value) -> Option<&mut Session> {
        let id = params["sessionId"].as_str()?;
        self.sessions.get_mut(id)
    }

    // ------------------------------------------------------------------ ZJ

    fn on_client(&mut self, message: Value) {
        let method = message["method"].as_str().map(str::to_string);
        match (method.as_deref(), message.get("id").cloned()) {
            (Some(method), Some(id)) => self.client_request(method, id, &message["params"]),
            (Some("session/cancel"), None) => self.cancel(&message["params"]),
            (None, Some(id)) => self.client_answer(&id, &message),
            _ => {}
        }
    }

    fn client_request(&mut self, method: &str, id: Value, params: &Value) {
        match method {
            "initialize" => acp::reply(
                id,
                acp::initialize_result(
                    "claude-code",
                    "Claude Code",
                    json!([
                        acp::terminal_login(
                            "claude-ai-login",
                            "Claude 订阅",
                            "用 Claude 订阅账号登录",
                            &["--login", "--claudeai"]
                        ),
                        acp::terminal_login(
                            "console-login",
                            "Anthropic Console",
                            "用 Anthropic Console 的 API 计费登录",
                            &["--login", "--console"]
                        ),
                    ]),
                ),
            ),
            "session/new" => {
                let session = acp::uuid();
                self.start(Some((id, false)), session, params, false);
            }
            "session/load" => match params["sessionId"].as_str() {
                Some(session) => self.start(Some((id, true)), session.to_string(), params, true),
                None => acp::reply_error(id, INVALID_PARAMS, "缺少 sessionId"),
            },
            "session/prompt" => self.prompt(id, params),
            "_session/steering" => self.steer(id, params),
            "session/set_mode" => {
                let mode = params["modeId"].as_str().unwrap_or("");
                if !MODES.iter().any(|m| m.0 == mode) {
                    return acp::reply_error(id, NOT_FOUND, "Claude Code 没有这个模式");
                }
                self.change(id, params, Change::Mode(mode.to_string()));
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or("").to_string();
                let change = match params["configId"].as_str() {
                    Some("model") => Change::Model(value),
                    Some("effort") => Change::Effort(value),
                    Some("fast") => Change::Fast(value == "on"),
                    _ => return acp::reply_error(id, NOT_FOUND, "没有这个设置"),
                };
                self.change(id, params, change);
            }
            "session/close" => {
                if let Some(session) = params["sessionId"].as_str() {
                    self.close(session);
                }
                acp::reply(id, json!({}));
            }
            "authenticate" => acp::reply_error(id, INTERNAL, "请在「终端」里登录 Claude Code"),
            _ => acp::reply_error(id, NOT_FOUND, &format!("不支持 {method}")),
        }
    }

    /// Starts `claude` for `session` (a new one, or `resume`d). `reply`: ZJ's request to
    /// answer once Claude Code is ready.
    fn start(
        &mut self,
        reply: Option<(Value, bool)>,
        session: String,
        params: &Value,
        resume: bool,
    ) {
        let previous = self.sessions.remove(&session);
        let existed = previous.is_some();
        let mut s = previous.unwrap_or_else(|| Session {
            id: session.clone(),
            mode: "default".into(),
            ..Default::default()
        });
        // Whatever the old `claude` still prints goes nowhere.
        if let Some(old) = s.key.take() {
            self.keys.remove(&old);
        }
        s.claude = None;
        if let Some(cwd) = params["cwd"].as_str() {
            s.cwd = PathBuf::from(cwd);
        }
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
        ]
        .map(String::from)
        .to_vec();
        args.push(s.mode.clone());
        if let Some(model) = &s.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &s.effort {
            args.extend(["--effort".into(), effort.clone()]);
        }
        args.extend([
            if resume { "--resume" } else { "--session-id" }.to_string(),
            session.clone(),
        ]);
        self.next_key += 1;
        let key = self.next_key;
        match Child::spawn(&self.cli, &args, &s.cwd, key, self.events.clone()) {
            Ok(child) => {
                eprintln!("event=agent_bridge_claude_start resume={resume}");
                s.claude = Some(child);
                s.key = Some(key);
                s.starting = reply;
                let init = json!({
                    "type": "control_request",
                    "request_id": INIT,
                    "request": { "subtype": "initialize", "hooks": null },
                });
                s.send(&init);
                self.keys.insert(key, session.clone());
                self.sessions.insert(session, s);
            }
            Err(e) => {
                let message = format!("无法启动 Claude Code（{}）：{e}", self.cli.display());
                if let Some((id, _)) = reply {
                    acp::reply_error(id, INTERNAL, &message);
                }
                if let Some((id, _)) = s.queued.take() {
                    acp::reply_error(id, INTERNAL, &message);
                }
                // Kept: the next prompt tries again.
                if existed {
                    self.sessions.insert(session, s);
                }
            }
        }
    }

    fn prompt(&mut self, id: Value, params: &Value) {
        let Some(sid) = params["sessionId"].as_str().map(String::from) else {
            return acp::reply_error(id, INVALID_PARAMS, "缺少 sessionId");
        };
        let Some(s) = self.sessions.get_mut(&sid) else {
            return acp::reply_error(id, NOT_FOUND, "没有这个会话");
        };
        if s.prompt.is_some() || s.queued.is_some() {
            return acp::reply_error(id, INTERNAL, "Claude Code 还在回答上一条消息");
        }
        if s.claude.is_none() {
            // Exited since (crash or sign-in): start it again on the same conversation.
            s.queued = Some((id, params["prompt"].clone()));
            self.start(None, sid, &json!({}), true);
            return;
        }
        if s.starting.is_some() {
            s.queued = Some((id, params["prompt"].clone()));
            return;
        }
        Self::send_prompt(s, id, &params["prompt"]);
    }

    fn send_prompt(s: &mut Session, id: Value, blocks: &Value) {
        s.outstanding.clear();
        s.interrupted = false;
        s.last_text = None;
        s.message = None;
        if s.user_message(content(blocks), None) {
            s.prompt = Some(id);
        } else {
            s.outstanding.clear();
            acp::reply_error(id, INTERNAL, "Claude Code 已退出");
        }
    }

    /// A message for the running turn: injected now (or after an open permission question);
    /// with no turn running ZJ sends it as a prompt (`promptRequired`).
    fn steer(&mut self, id: Value, params: &Value) {
        let Some(s) = self.session(params) else {
            return acp::reply_error(id, NOT_FOUND, "没有这个会话");
        };
        if s.prompt.is_none() || s.interrupted || s.claude.is_none() {
            return acp::reply(id, json!({ "outcome": "promptRequired" }));
        }
        let priority = if s.asking > 0 { "later" } else { "now" };
        if s.user_message(content(&params["prompt"]), Some(priority)) {
            acp::reply(id, json!({ "outcome": "injected" }));
        } else {
            acp::reply_error(id, INTERNAL, "Claude Code 已退出");
        }
    }

    fn cancel(&mut self, params: &Value) {
        let Some(s) = self.session(params) else {
            return;
        };
        if let Some((id, _)) = s.queued.take() {
            acp::reply(id, json!({ "stopReason": "cancelled" }));
        }
        if s.prompt.is_some() && !s.interrupted {
            s.interrupted = true;
            s.control(json!({ "subtype": "interrupt" }));
        }
    }

    /// Changes a setting: through Claude Code when it runs, else for its next start.
    fn change(&mut self, id: Value, params: &Value, change: Change) {
        let Some(s) = self.session(params) else {
            return acp::reply_error(id, NOT_FOUND, "没有这个会话");
        };
        let request = match &change {
            Change::Mode(mode) => json!({ "subtype": "set_permission_mode", "mode": mode }),
            Change::Model(model) => json!({ "subtype": "set_model", "model": model }),
            Change::Effort(effort) => {
                let level = (effort != "default").then_some(effort.as_str());
                json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": level } })
            }
            Change::Fast(on) => {
                json!({ "subtype": "apply_flag_settings", "settings": { "fastMode": on } })
            }
        };
        let config = params.get("configId").is_some();
        if s.claude.is_none() || s.starting.is_some() {
            Self::apply(s, change);
            return Self::answer_change(s, id, config);
        }
        let rid = s.control(request);
        s.changes.insert(rid, (Some(id), change));
    }

    fn apply(s: &mut Session, change: Change) {
        match change {
            Change::Mode(mode) => s.mode = mode,
            Change::Model(model) => {
                s.model = Some(model);
                // A level the new model doesn't offer falls back to its default.
                let keep = s
                    .current_model()
                    .zip(s.effort.as_ref())
                    .is_some_and(|(m, e)| m.efforts.contains(e));
                if !keep {
                    s.effort = None;
                }
            }
            Change::Effort(effort) => s.effort = (effort != "default").then_some(effort),
            Change::Fast(on) => s.fast = on,
        }
    }

    fn answer_change(s: &Session, id: Value, config: bool) {
        if config {
            acp::reply(id, json!({ "configOptions": s.configs() }));
        } else {
            acp::reply(id, json!({}));
        }
    }

    fn close(&mut self, session: &str) {
        let Some(mut s) = self.sessions.remove(session) else {
            return;
        };
        if let Some(key) = s.key.take() {
            self.keys.remove(&key);
        }
        s.claude = None;
        self.asked.retain(|_, a| a.session != session);
        eprintln!("event=agent_bridge_claude_close");
    }

    /// ZJ answered a permission question.
    fn client_answer(&mut self, id: &Value, answer: &Value) {
        let Some(asked) = id.as_i64().and_then(|id| self.asked.remove(&id)) else {
            return;
        };
        let Some(s) = self.sessions.get_mut(&asked.session) else {
            return;
        };
        s.asking = s.asking.saturating_sub(1);
        let option = acp::chosen_option(answer).unwrap_or("cancelled");
        if option == "allow_always" {
            s.allowed.insert(asked.rule.clone());
        }
        let response = match option {
            "allow" | "allow_always" | "plan_edits" | "plan_ask" => {
                json!({ "behavior": "allow", "updatedInput": asked.input })
            }
            "cancelled" => {
                json!({ "behavior": "deny", "message": "用户停止了这一轮。", "interrupt": true })
            }
            _ if asked.tool == "ExitPlanMode" => {
                json!({ "behavior": "deny", "message": "用户希望继续规划。" })
            }
            _ => json!({ "behavior": "deny", "message": format!("用户没有允许 {}。", asked.tool) }),
        };
        s.control_reply(&asked.request_id, response);
        let mode = match option {
            "plan_edits" => Some("acceptEdits"),
            "plan_ask" => Some("default"),
            _ => None,
        };
        if let Some(mode) = mode {
            let rid = s.control(json!({ "subtype": "set_permission_mode", "mode": mode }));
            s.changes.insert(rid, (None, Change::Mode(mode.into())));
        }
    }

    // ------------------------------------------------------------------ Claude Code

    fn claude_gone(&mut self, key: u64) {
        let Some(sid) = self.keys.remove(&key) else {
            return;
        };
        let Some(s) = self.sessions.get_mut(&sid) else {
            return;
        };
        if s.key != Some(key) {
            return;
        }
        eprintln!("event=agent_bridge_claude_exit");
        s.key = None;
        s.claude = None;
        s.asking = 0;
        s.changes.clear();
        self.asked.retain(|_, a| a.session != sid);
        let message = "Claude Code 已退出，再发一条消息会重新启动并接着这个会话";
        if let Some((id, _)) = s.starting.take() {
            acp::reply_error(id, INTERNAL, "Claude Code 启动时退出了");
        }
        if let Some((id, _)) = s.queued.take() {
            acp::reply_error(id, INTERNAL, message);
        }
        if let Some(id) = s.prompt.take() {
            if s.interrupted {
                acp::reply(id, json!({ "stopReason": "cancelled" }));
            } else {
                acp::reply_error(id, INTERNAL, message);
            }
        }
    }

    fn on_claude(&mut self, key: u64, m: &Value) {
        let Some(sid) = self.keys.get(&key).cloned() else {
            return;
        };
        if self.sessions.get(&sid).is_none_or(|s| s.key != Some(key)) {
            return;
        }
        match m["type"].as_str() {
            Some("control_response") => self.control_response(&sid, m),
            Some("control_request") => self.claude_asks(&sid, m),
            _ => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                let top = m["parent_tool_use_id"].is_null();
                match m["type"].as_str() {
                    Some("stream_event") if top => stream_event(s, &m["event"]),
                    Some("assistant") if top => assistant_message(s, m),
                    // A subagent's steps stay inside its tool call, but its edits are
                    // shown (with their diffs, for review).
                    Some("assistant") => {
                        let content = m["message"]["content"].as_array().into_iter().flatten();
                        for block in content.filter(|b| is_edit(b["name"].as_str().unwrap_or(""))) {
                            tool_use(s, block);
                        }
                    }
                    Some("user") => tool_results(s, &m["message"]["content"]),
                    Some("result") => result(s, m),
                    Some("system") => system(s, m),
                    _ => {}
                }
            }
        }
    }

    fn control_response(&mut self, sid: &str, m: &Value) {
        let Some(s) = self.sessions.get_mut(sid) else {
            return;
        };
        let r = &m["response"];
        let ok = r["subtype"].as_str() != Some("error");
        let error = r["error"]
            .as_str()
            .unwrap_or("Claude Code 拒绝了这个请求")
            .to_string();
        let Some(rid) = r["request_id"].as_str() else {
            return;
        };
        if rid == INIT {
            return self.ready(sid, ok, &r["response"], &error);
        }
        let Some((id, change)) = s.changes.remove(rid) else {
            return;
        };
        let is_mode = matches!(change, Change::Mode(_));
        match (ok, id) {
            (true, Some(id)) => {
                Self::apply(s, change);
                Self::answer_change(s, id, !is_mode);
            }
            // Ours (leaving plan mode, fast mode after a restart): ZJ learns by an update.
            (true, None) => {
                Self::apply(s, change);
                if is_mode {
                    s.update(
                        json!({ "sessionUpdate": "current_mode_update", "currentModeId": s.mode }),
                    );
                } else {
                    s.update(json!({ "sessionUpdate": "config_option_update", "configOptions": s.configs() }));
                }
            }
            (false, Some(id)) => acp::reply_error(id, INTERNAL, &error),
            (false, None) => eprintln!("event=agent_bridge_claude_change_failed"),
        }
    }

    /// The control `initialize` answered: the session is ready.
    fn ready(&mut self, sid: &str, ok: bool, info: &Value, error: &str) {
        let Some(s) = self.sessions.get_mut(sid) else {
            return;
        };
        let reply = s.starting.take();
        if !ok {
            let message = format!("Claude Code 没有启动：{error}");
            if let Some((id, _)) = reply {
                acp::reply_error(id, INTERNAL, &message);
            }
            if let Some((id, _)) = s.queued.take() {
                acp::reply_error(id, INTERNAL, &message);
            }
            return self.close(sid);
        }
        s.models = info["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(model)
            .collect();
        let wanted_fast = s.fast;
        s.fast = info["fast_mode_state"].as_str() == Some("on");
        s.commands = info["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(command)
            .collect();
        if let Some((id, load)) = reply {
            let mut answer = json!({ "modes": s.mode_state(), "configOptions": s.configs() });
            if !load {
                answer["sessionId"] = json!(s.id);
            }
            acp::reply(id, answer);
        }
        // Kept across restarts of `claude`, which starts with it off.
        if wanted_fast && !s.fast && s.current_model().is_some_and(|m| m.fast) {
            let request =
                json!({ "subtype": "apply_flag_settings", "settings": { "fastMode": true } });
            let rid = s.control(request);
            s.changes.insert(rid, (None, Change::Fast(true)));
        }
        if !s.commands.is_empty() {
            s.update(json!({ "sessionUpdate": "available_commands_update", "availableCommands": s.commands }));
        }
        if let Some((id, blocks)) = s.queued.take() {
            Self::send_prompt(s, id, &blocks);
        }
    }

    /// Claude Code asks to use a tool: allowed for this session already, or a question.
    fn claude_asks(&mut self, sid: &str, m: &Value) {
        let Some(s) = self.sessions.get_mut(sid) else {
            return;
        };
        let request_id = m["request_id"].clone();
        let r = &m["request"];
        if r["subtype"].as_str() != Some("can_use_tool") {
            let message = json!({
                "type": "control_response",
                "response": { "subtype": "error", "request_id": request_id, "error": "ZJ 不支持这个请求" },
            });
            s.send(&message);
            return;
        }
        let tool = r["tool_name"].as_str().unwrap_or("").to_string();
        let input = r["input"].clone();
        if tool == "AskUserQuestion" {
            let response = json!({
                "behavior": "deny",
                "message": "ZJ 的面板不能显示选择题。请直接在回复里提出问题，等用户回答。",
            });
            return s.control_reply(&request_id, response);
        }
        let rule = if tool == "Bash" {
            format!("Bash:{}", input["command"].as_str().unwrap_or(""))
        } else {
            tool.clone()
        };
        if s.allowed.contains(&rule) {
            return s.control_reply(
                &request_id,
                json!({ "behavior": "allow", "updatedInput": input }),
            );
        }
        let (tool_call, options) = if tool == "ExitPlanMode" {
            let call = json!({
                "toolCallId": r["tool_use_id"].as_str().map_or_else(acp::uuid, String::from),
                "title": "按计划开始改动？",
                "kind": "switch_mode",
                "status": "pending",
            });
            let options = json!([
                acp::option("plan_edits", "开始，并自动接受编辑", "allow_once"),
                acp::option("plan_ask", "开始，编辑前仍询问", "allow_once"),
                acp::option("reject", "继续规划", "reject_once"),
            ]);
            (call, options)
        } else {
            let mut call = tool_call(
                r["tool_use_id"]
                    .as_str()
                    .map_or_else(acp::uuid, String::from),
                &tool,
                &input,
                r["tool_use_id"]
                    .as_str()
                    .and_then(|id| s.before.get(id).cloned()),
            );
            call["status"] = json!("pending");
            let always = if tool == "Bash" {
                "本会话都允许这条命令"
            } else {
                "本会话都允许"
            };
            let options = json!([
                acp::option("allow", "允许", "allow_once"),
                acp::option("allow_always", always, "allow_always"),
                acp::option("reject", "拒绝", "reject_once"),
            ]);
            (call, options)
        };
        s.asking += 1;
        let id = self.requests.ask_permission(sid, tool_call, options);
        self.asked.insert(
            id,
            Asked {
                session: sid.to_string(),
                request_id,
                tool,
                input,
                rule,
            },
        );
    }
}

fn model(m: &Value) -> Option<Model> {
    let value = m["value"].as_str()?.to_string();
    let efforts = if m["supportsEffort"].as_bool() == Some(true) {
        m["supportedEffortLevels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().map(String::from))
            .collect()
    } else {
        Vec::new()
    };
    Some(Model {
        name: m["displayName"].as_str().unwrap_or(&value).to_string(),
        value,
        efforts,
        fast: m["supportsFastMode"].as_bool() == Some(true),
    })
}

/// A command from the control `initialize` as an ACP available command.
fn command(c: &Value) -> Option<Value> {
    let name = c["name"].as_str().filter(|n| !n.is_empty())?;
    let hint = c["argumentHint"].as_str().filter(|h| !h.is_empty());
    Some(json!({
        "name": name,
        "description": c["description"].as_str().unwrap_or(""),
        "input": hint.map(|h| json!({ "hint": h })),
    }))
}

fn stream_event(s: &mut Session, e: &Value) {
    match e["type"].as_str() {
        Some("message_start") => {
            s.message = e["message"]["id"].as_str().map(String::from);
            s.blocks.clear();
        }
        Some("content_block_delta") => {
            let d = &e["delta"];
            let message = s.message.clone().unwrap_or_default();
            match d["type"].as_str() {
                Some("text_delta") => {
                    s.streamed.insert(message.clone());
                    s.agent_text(&message, d["text"].as_str().unwrap_or(""));
                }
                Some("thinking_delta") => {
                    s.thought.insert(message);
                    let text = d["thinking"].as_str().unwrap_or("");
                    s.update(acp::text_chunk("agent_thought_chunk", text));
                }
                _ => {}
            }
        }
        Some("message_stop") => s.message = None,
        _ => {}
    }
    edit_block(s, e);
}

/// Follows a streaming tool use; once an edit's input is complete, reads the file it edits.
fn edit_block(s: &mut Session, e: &Value) {
    let Some(index) = e["index"].as_u64() else {
        return;
    };
    match (e["type"].as_str(), e["delta"]["type"].as_str()) {
        (Some("content_block_start"), _) => {
            let b = &e["content_block"];
            let name = b["name"].as_str().unwrap_or("");
            if b["type"].as_str() == Some("tool_use") && is_edit(name) {
                let id = b["id"].as_str().unwrap_or("").to_string();
                s.blocks
                    .insert(index, (id, name.to_string(), String::new()));
            }
        }
        (Some("content_block_delta"), Some("input_json_delta")) => {
            if let Some((_, _, json)) = s.blocks.get_mut(&index) {
                json.push_str(e["delta"]["partial_json"].as_str().unwrap_or(""));
            }
        }
        (Some("content_block_stop"), _) => {
            let Some((id, _, json)) = s.blocks.remove(&index) else {
                return;
            };
            let input: Value = serde_json::from_str(&json).unwrap_or_default();
            if let Some(path) = input["file_path"]
                .as_str()
                .filter(|p| Path::new(p).is_absolute())
            {
                s.before.insert(id, std::fs::read_to_string(path).ok());
            }
        }
        _ => {}
    }
}

fn is_edit(tool: &str) -> bool {
    matches!(tool, "Edit" | "MultiEdit" | "Write")
}

/// A whole message: its tool uses become tool calls (its text and thinking too, if they
/// didn't stream).
fn assistant_message(s: &mut Session, m: &Value) {
    let msg = &m["message"];
    let id = msg["id"].as_str().unwrap_or("").to_string();
    let error = m["error"].is_string();
    for block in msg["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") if !s.streamed.contains(&id) && !error => {
                let text = block["text"].as_str().unwrap_or("");
                if !text.is_empty() {
                    s.agent_text(&id, text);
                }
            }
            Some("thinking") if !s.thought.contains(&id) => {
                let text = block["thinking"].as_str().unwrap_or("");
                if !text.is_empty() {
                    s.update(acp::text_chunk("agent_thought_chunk", text));
                }
            }
            Some("tool_use") => tool_use(s, block),
            _ => {}
        }
    }
}

fn tool_use(s: &mut Session, block: &Value) {
    let id = block["id"].as_str().unwrap_or("").to_string();
    let name = block["name"].as_str().unwrap_or("").to_string();
    let input = &block["input"];
    if id.is_empty() || s.tools.contains_key(&id) {
        return;
    }
    s.tools.insert(id.clone(), name.clone());
    match name.as_str() {
        "TodoWrite" => s.update(json!({ "sessionUpdate": "plan", "entries": plan_entries(input) })),
        // Plan mode's plan is the reply; approving it is a permission question.
        "ExitPlanMode" => {
            let plan = input["plan"].as_str().unwrap_or("").to_string();
            s.agent_text(&id, &plan);
        }
        _ => {
            let before = s.before.remove(&id);
            let mut call = tool_call(id, &name, input, before);
            call["sessionUpdate"] = json!("tool_call");
            call["status"] = json!("in_progress");
            s.update(call);
        }
    }
}

/// An ACP tool call for a tool use: title, kind, the file it touches, the edit's diff, and
/// the input (ZJ shows a command's own text in its approval card).
fn tool_call(id: String, tool: &str, input: &Value, before: Option<Option<String>>) -> Value {
    let (title, kind) = describe(tool, input);
    let mut call = json!({ "toolCallId": id, "title": title, "kind": kind, "rawInput": input });
    let path = input["file_path"]
        .as_str()
        .or(input["notebook_path"].as_str())
        .filter(|p| Path::new(p).is_absolute());
    if let Some(path) = path {
        call["locations"] = json!([{ "path": path }]);
    }
    let diffs = edit_diffs(tool, input, before);
    if !diffs.is_empty() {
        call["content"] = json!(diffs);
    }
    call
}

fn tool_results(s: &mut Session, content: &Value) {
    for block in content.as_array().into_iter().flatten() {
        if block["type"].as_str() != Some("tool_result") {
            continue;
        }
        let id = block["tool_use_id"].as_str().unwrap_or("").to_string();
        let Some(name) = s.tools.get(&id) else {
            continue;
        };
        if name == "TodoWrite" || name == "ExitPlanMode" {
            continue;
        }
        let failed = block["is_error"].as_bool() == Some(true);
        let edit = matches!(
            name.as_str(),
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit"
        );
        let status = if failed { "failed" } else { "completed" };
        let mut update =
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status });
        if (!edit || failed)
            && let Some(output) = acp::output_content(&result_text(&block["content"]))
        {
            update["content"] = output;
        }
        s.update(update);
    }
}

/// The prompt ends once every message we sent in it was answered (a steered message
/// interrupts the cycle, whose own result names only the message it was answering).
fn result(s: &mut Session, m: &Value) {
    s.message = None;
    s.streamed.clear();
    s.thought.clear();
    let consumed: Vec<&str> = match m["user_message_uuids"].as_array() {
        Some(list) => list.iter().filter_map(Value::as_str).collect(),
        None => m["user_message_uuid"].as_str().into_iter().collect(),
    };
    if consumed.is_empty() {
        // A Claude Code that doesn't name them: the result is the turn's.
        s.outstanding.clear();
    }
    for uuid in consumed {
        s.outstanding.remove(uuid);
    }
    if fast_state(s, m) {
        s.update(json!({ "sessionUpdate": "config_option_update", "configOptions": s.configs() }));
    }
    if s.prompt.is_none() || (!s.outstanding.is_empty() && !s.interrupted) {
        return;
    }
    let Some(id) = s.prompt.take() else { return };
    s.outstanding.clear();
    if s.interrupted {
        return acp::reply(id, json!({ "stopReason": "cancelled" }));
    }
    match turn_end(m) {
        Ok(reason) => acp::reply(id, json!({ "stopReason": reason })),
        Err((code, message)) => acp::reply_error(id, code, &message),
    }
}

/// A result's stop reason, or the error ZJ shows.
fn turn_end(m: &Value) -> Result<&'static str, (i64, String)> {
    let failed = m["is_error"].as_bool() == Some(true)
        || m["subtype"]
            .as_str()
            .is_some_and(|s| s.starts_with("error"));
    if !failed {
        return Ok(if m["stop_reason"].as_str() == Some("refusal") {
            "refusal"
        } else {
            "end_turn"
        });
    }
    if m["subtype"].as_str() == Some("error_max_turns") {
        return Ok("max_turn_requests");
    }
    let text = m["result"].as_str().unwrap_or("");
    if is_auth_error(m) {
        return Err((AUTH_REQUIRED, "Claude Code 需要登录".into()));
    }
    let text = if text.is_empty() {
        "Claude Code 出错停止了".to_string()
    } else {
        text.to_string()
    };
    Err((INTERNAL, text))
}

fn is_auth_error(m: &Value) -> bool {
    let text = m["result"].as_str().unwrap_or("").to_lowercase();
    [
        "authenticate",
        "oauth",
        "log in",
        "login",
        "/login",
        "api key",
    ]
    .iter()
    .any(|k| text.contains(k))
        && (m["terminal_reason"].as_str() == Some("api_error")
            || text.contains("invalid api key")
            || text.contains("not logged in")
            || text.contains("please run /login"))
}

/// Claude Code's fast mode state when a message reports it; true when it changed.
fn fast_state(s: &mut Session, m: &Value) -> bool {
    let Some(state) = m["fast_mode_state"].as_str() else {
        return false;
    };
    let on = state == "on";
    let changed = on != s.fast;
    s.fast = on;
    changed
}

fn system(s: &mut Session, m: &Value) {
    if let Some(mode) = m["permissionMode"].as_str()
        && mode != s.mode
        && MODES.iter().any(|x| x.0 == mode)
    {
        s.mode = mode.to_string();
        s.update(json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }));
    }
    if fast_state(s, m) {
        s.update(json!({ "sessionUpdate": "config_option_update", "configOptions": s.configs() }));
    }
}

/// A tool use's title and ACP kind.
fn describe(tool: &str, input: &Value) -> (String, &'static str) {
    let s = |key: &str| input[key].as_str().unwrap_or("").trim().to_string();
    let file = |key: &str| acp::file_name(&s(key));
    match tool {
        "Bash" => (format!("运行 `{}`", s("command")), "execute"),
        "Read" => (format!("读取 {}", file("file_path")), "read"),
        "Edit" | "MultiEdit" => (format!("编辑 {}", file("file_path")), "edit"),
        "Write" => (format!("写入 {}", file("file_path")), "edit"),
        "NotebookEdit" => (format!("编辑 {}", file("notebook_path")), "edit"),
        "Glob" => (format!("查找文件 {}", s("pattern")), "search"),
        "Grep" => (format!("搜索 {}", s("pattern")), "search"),
        "WebFetch" => (format!("获取 {}", s("url")), "fetch"),
        "WebSearch" => (format!("网页搜索 {}", s("query")), "fetch"),
        "Task" | "Agent" => (format!("子 Agent：{}", s("description")), "think"),
        "Skill" => (format!("使用技能 {}", s("skill")), "other"),
        _ => match tool.strip_prefix("mcp__").and_then(|t| t.split_once("__")) {
            Some((server, name)) => (format!("{server}: {name}"), "other"),
            None => (tool.to_string(), "other"),
        },
    }
}

/// An edit's diff (ACP diff content), from which ZJ takes its review snapshot, even when
/// Claude Code has written the file by the time the tool call arrives. A single replacement
/// is sent as its hunk (`old_string` → `new_string`): ZJ finds it in the file before or after
/// the edit. Otherwise the whole file, as read when the tool use was complete (`before`) or
/// now, against the result.
fn edit_diffs(tool: &str, input: &Value, before: Option<Option<String>>) -> Vec<Value> {
    let path = input["file_path"].as_str().unwrap_or("");
    if path.is_empty() || !Path::new(path).is_absolute() {
        return Vec::new();
    }
    if tool == "Edit"
        && let (Some(from), Some(to)) = (input["old_string"].as_str(), input["new_string"].as_str())
        && !from.is_empty()
        && !to.is_empty()
        && input["replace_all"].as_bool() != Some(true)
    {
        return vec![json!({ "type": "diff", "path": path, "oldText": from, "newText": to })];
    }
    let old = before.unwrap_or_else(|| std::fs::read_to_string(path).ok());
    let edit = |text: &str, e: &Value| -> Option<String> {
        let (from, to) = (e["old_string"].as_str()?, e["new_string"].as_str()?);
        if from.is_empty() || !text.contains(from) {
            return None;
        }
        Some(if e["replace_all"].as_bool() == Some(true) {
            text.replace(from, to)
        } else {
            text.replacen(from, to, 1)
        })
    };
    let new = match tool {
        "Write" => input["content"].as_str().map(String::from),
        "Edit" => old.as_deref().and_then(|t| edit(t, input)),
        "MultiEdit" => input["edits"]
            .as_array()
            .and_then(|edits| edits.iter().try_fold(old.clone()?, |t, e| edit(&t, e))),
        _ => None,
    };
    let Some(new) = new else {
        return Vec::new();
    };
    let mut d = json!({ "type": "diff", "path": path, "newText": new });
    if let Some(old) = old {
        d["oldText"] = json!(old);
    }
    vec![d]
}

fn plan_entries(input: &Value) -> Vec<Value> {
    input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let status = match t["status"].as_str() {
                Some("completed") => "completed",
                Some("in_progress") => "in_progress",
                _ => "pending",
            };
            json!({ "content": t["content"], "priority": "medium", "status": status })
        })
        .collect()
}

/// A tool result's text (a string, or text blocks).
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            id: "s".into(),
            mode: "default".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_steered_turn_ends_when_every_message_is_answered() {
        let mut s = session();
        s.prompt = Some(json!(7));
        s.outstanding = ["a".to_string(), "b".to_string()].into();
        // The interrupted cycle answers only the first message.
        result(
            &mut s,
            &json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "user_message_uuids": ["a"] }),
        );
        assert!(s.prompt.is_some());
        result(
            &mut s,
            &json!({ "type": "result", "subtype": "success", "user_message_uuid": "b" }),
        );
        assert!(s.prompt.is_none());
        assert!(s.outstanding.is_empty());
    }

    #[test]
    fn results_without_uuids_or_after_a_cancel_end_the_turn() {
        let mut s = session();
        s.prompt = Some(json!(1));
        s.outstanding = ["a".to_string()].into();
        result(&mut s, &json!({ "type": "result", "subtype": "success" }));
        assert!(s.prompt.is_none());
        s.prompt = Some(json!(2));
        s.outstanding = ["a".to_string(), "b".to_string()].into();
        s.interrupted = true;
        result(
            &mut s,
            &json!({ "type": "result", "subtype": "error_during_execution", "user_message_uuids": ["a"] }),
        );
        assert!(s.prompt.is_none());
    }

    #[test]
    fn results_become_stop_reasons_or_errors() {
        assert_eq!(turn_end(&json!({ "subtype": "success" })), Ok("end_turn"));
        assert_eq!(
            turn_end(&json!({ "subtype": "error_max_turns", "is_error": true })),
            Ok("max_turn_requests")
        );
        let expired = json!({ "is_error": true, "terminal_reason": "api_error", "result": "Failed to authenticate: OAuth session expired" });
        assert_eq!(turn_end(&expired).unwrap_err().0, AUTH_REQUIRED);
        let key = json!({ "is_error": true, "result": "Invalid API key · Please run /login" });
        assert_eq!(turn_end(&key).unwrap_err().0, AUTH_REQUIRED);
        let other = json!({ "is_error": true, "result": "Overloaded" });
        assert_eq!(turn_end(&other), Err((INTERNAL, "Overloaded".into())));
    }

    #[test]
    fn settings_follow_the_model() {
        let mut s = session();
        s.models = vec![
            model(&json!({ "value": "default", "displayName": "默认", "supportsEffort": true, "supportedEffortLevels": ["low", "high"], "supportsFastMode": true })).unwrap(),
            model(&json!({ "value": "haiku", "displayName": "Haiku" })).unwrap(),
        ];
        let configs = s.configs();
        assert_eq!(configs.len(), 3);
        assert_eq!(acp::current(&configs, "effort").as_deref(), Some("default"));
        Bridge::apply(&mut s, Change::Effort("high".into()));
        assert_eq!(
            acp::current(&s.configs(), "effort").as_deref(),
            Some("high")
        );
        Bridge::apply(&mut s, Change::Model("haiku".into()));
        assert_eq!(s.effort, None);
        assert_eq!(s.configs().len(), 1);
    }

    #[test]
    fn complete_edit_blocks_read_the_file_first() {
        let dir = std::env::temp_dir().join(format!("zj-bridge-before-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("b.txt");
        std::fs::write(&file, "old\n").unwrap();
        let mut s = session();
        let input = json!({ "file_path": file, "content": "new\n" }).to_string();
        let (head, rest) = input.split_at(10);
        stream_event(
            &mut s,
            &json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "w", "name": "Write" } }),
        );
        for part in [head, rest] {
            stream_event(
                &mut s,
                &json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": part } }),
            );
        }
        stream_event(&mut s, &json!({ "type": "content_block_stop", "index": 1 }));
        assert_eq!(s.before.get("w"), Some(&Some("old\n".to_string())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompts_become_claude_content() {
        let blocks = json!([
            { "type": "image", "mimeType": "image/png", "data": "AAAA" },
            { "type": "text", "text": "颜色？" },
        ]);
        let c = content(&blocks);
        assert_eq!(
            c[0]["source"],
            json!({ "type": "base64", "media_type": "image/png", "data": "AAAA" })
        );
        assert_eq!(c[1], json!({ "type": "text", "text": "颜色？" }));
        let only_image =
            content(&json!([{ "type": "image", "mimeType": "image/png", "data": "A" }]));
        assert_eq!(only_image.len(), 1);
    }

    #[test]
    fn tools_become_calls() {
        assert_eq!(
            describe("Bash", &json!({ "command": "ls -1 " })),
            ("运行 `ls -1`".into(), "execute")
        );
        assert_eq!(
            describe("mcp__github__search", &json!({})),
            ("github: search".into(), "other")
        );
        let call = tool_call("t".into(), "Bash", &json!({ "command": "ls" }), None);
        assert_eq!(call["rawInput"]["command"], "ls");
        let dir = std::env::temp_dir().join(format!("zj-bridge-diffs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "one two one\n").unwrap();
        let path = file.to_string_lossy().to_string();
        // A single replacement: its hunk, whether or not the file is written yet.
        let edit = json!({ "file_path": path, "old_string": "two", "new_string": "2" });
        let d = edit_diffs("Edit", &edit, None);
        assert_eq!(
            (d[0]["oldText"].as_str(), d[0]["newText"].as_str()),
            (Some("two"), Some("2"))
        );
        // Otherwise the whole file, preferably as read when the tool use was complete.
        let all = json!({ "file_path": path, "old_string": "one", "new_string": "1", "replace_all": true });
        let d = edit_diffs("Edit", &all, None);
        assert_eq!(
            (d[0]["oldText"].as_str(), d[0]["newText"].as_str()),
            (Some("one two one\n"), Some("1 two 1\n"))
        );
        std::fs::write(&file, "1 two 1\n").unwrap();
        let d = edit_diffs("Edit", &all, Some(Some("one two one\n".into())));
        assert_eq!(d[0]["oldText"].as_str(), Some("one two one\n"));
        let multi = json!({ "file_path": path, "edits": [{ "old_string": "1", "new_string": "one", "replace_all": true }, { "old_string": "two", "new_string": "" }] });
        assert_eq!(
            edit_diffs("MultiEdit", &multi, None)[0]["newText"].as_str(),
            Some("one  one\n")
        );
        let new = dir.join("new.txt").to_string_lossy().into_owned();
        let d = edit_diffs("Write", &json!({ "file_path": new, "content": "x" }), None);
        assert!(d[0].get("oldText").is_none());
        let rel = json!({ "file_path": "relative.txt", "old_string": "a", "new_string": "b" });
        assert!(edit_diffs("Edit", &rel, None).is_empty());
        let call = tool_call("t".into(), "Edit", &edit, None);
        assert_eq!(call["locations"][0]["path"], json!(path));
        assert_eq!(call["content"][0]["type"], "diff");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            plan_entries(&json!({ "todos": [{ "content": "A", "status": "in_progress" }] })),
            vec![json!({ "content": "A", "priority": "medium", "status": "in_progress" })]
        );
        assert_eq!(
            command(&json!({ "name": "review", "description": "d", "argumentHint": "<pr>" })),
            Some(json!({ "name": "review", "description": "d", "input": { "hint": "<pr>" } }))
        );
    }
}
