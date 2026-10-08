//! ACP v1 on our stdout, and what both bridges share: prompt content, session modes and model
//! settings in the shapes `agent_client` reads.

use serde_json::{Value, json};
use std::io::Write;

pub(crate) const PROTOCOL_VERSION: u64 = 1;
pub(crate) const INTERNAL: i64 = -32603;
pub(crate) const NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
/// ACP's "authentication required": ZJ shows the login card and retries.
pub(crate) const AUTH_REQUIRED: i64 = -32000;
/// How many lines of a tool's output go in its tool call.
pub(crate) const OUTPUT_LINES: usize = 12;

/// One JSON-RPC message to ZJ. A failed write means ZJ is gone; the stdin reader ends the loop.
pub(crate) fn send(message: &Value) {
    let mut line = message.to_string();
    line.push('\n');
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes()).and_then(|()| out.flush());
}

pub(crate) fn reply(id: Value, result: Value) {
    send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

pub(crate) fn reply_error(id: Value, code: i64, message: &str) {
    send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
}

pub(crate) fn update(session: &str, update: Value) {
    send(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "sessionId": session, "update": update },
    }));
}

pub(crate) fn text_chunk(kind: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
}

/// Our requests to ZJ (permission questions); ids are ours, answers come back by id.
#[derive(Default)]
pub(crate) struct Requests {
    next: i64,
}

impl Requests {
    pub fn ask_permission(&mut self, session: &str, tool_call: Value, options: Value) -> i64 {
        self.next += 1;
        send(&json!({
            "jsonrpc": "2.0",
            "id": self.next,
            "method": "session/request_permission",
            "params": { "sessionId": session, "toolCall": tool_call, "options": options },
        }));
        self.next
    }
}

/// A question for the user (Claude Code's `AskUserQuestion`, Codex's `requestUserInput`;
/// both `{ question, header, options: [{ label, description }] }`) as a permission request
/// whose options are the answers (`answer:<n>`) plus "skip". ZJ shows it as a question card
/// (`agent_client::thread::permission_question`). Only questions with options can be asked.
pub(crate) fn ask_question(
    requests: &mut Requests,
    session: &str,
    call_id: String,
    q: &Value,
) -> i64 {
    let mut options: Vec<Value> = q["options"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(n, o)| {
            option(
                &format!("{ANSWER}{n}"),
                o["label"].as_str().unwrap_or(""),
                "allow_once",
            )
        })
        .collect();
    options.push(option("skip", "跳过", "reject_once"));
    let call = json!({
        "toolCallId": call_id,
        "title": q["header"].as_str().filter(|h| !h.is_empty()).or(q["question"].as_str()),
        "kind": "other",
        "status": "pending",
        "rawInput": q,
    });
    requests.ask_permission(session, call, json!(options))
}

/// Answer options' id prefix, shared with `agent_client::thread::ANSWER`.
pub(crate) const ANSWER: &str = "answer:";

/// The label of the answer ZJ picked for `q`, if any (`None`: skipped or cancelled).
pub(crate) fn answer_label<'a>(q: &'a Value, answer: &Value) -> Option<&'a str> {
    let n: usize = chosen_option(answer)?.strip_prefix(ANSWER)?.parse().ok()?;
    q["options"][n]["label"].as_str()
}

/// Whether `q` can be asked (it has text and options).
pub(crate) fn askable(q: &Value) -> bool {
    q["question"].is_string() && q["options"].as_array().is_some_and(|o| !o.is_empty())
}

/// The option ZJ picked for a permission question (`None`: cancelled).
pub(crate) fn chosen_option(answer: &Value) -> Option<&str> {
    let outcome = &answer["result"]["outcome"];
    (outcome["outcome"].as_str() == Some("selected"))
        .then(|| outcome["optionId"].as_str())
        .flatten()
}

pub(crate) fn option(id: &str, name: &str, kind: &str) -> Value {
    json!({ "optionId": id, "name": name, "kind": kind })
}

/// The answer to `initialize`: what `agent_client::host::init_from` reads.
pub(crate) fn initialize_result(name: &str, title: &str, auth_methods: Value) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": { "image": true, "embeddedContext": true },
            "sessionCapabilities": { "close": {} },
        },
        "agentInfo": { "name": name, "title": title, "version": env!("CARGO_PKG_VERSION") },
        "authMethods": auth_methods,
        "_meta": { "steering": { "supported": true } },
    })
}

/// A login run in Terminal.app: ZJ appends `args` to our own command line.
pub(crate) fn terminal_login(id: &str, name: &str, description: &str, args: &[&str]) -> Value {
    json!({ "id": id, "name": name, "description": description, "type": "terminal", "args": args })
}

/// `modes` in `session/new` and `session/load` answers. `modes`: (id, name, description).
pub(crate) fn mode_state(current: &str, modes: &[(&str, &str, &str)]) -> Value {
    let available: Vec<Value> = modes
        .iter()
        .map(
            |(id, name, description)| json!({ "id": id, "name": name, "description": description }),
        )
        .collect();
    json!({ "currentModeId": current, "availableModes": available })
}

/// A select setting (`configOptions`). `category`: `model`, `thought_level` or `model_config`.
pub(crate) fn select(
    id: &str,
    name: &str,
    category: &str,
    current: &str,
    values: &[(String, String)],
) -> Value {
    let options: Vec<Value> = values
        .iter()
        .map(|(value, name)| json!({ "value": value, "name": name }))
        .collect();
    json!({
        "id": id,
        "name": name,
        "category": category,
        "type": "select",
        "currentValue": current,
        "options": options,
    })
}

pub(crate) fn on_off() -> Vec<(String, String)> {
    vec![("off".into(), "关".into()), ("on".into(), "开".into())]
}

/// A reasoning level's name (both CLIs' levels).
pub(crate) fn effort_name(level: &str) -> String {
    match level {
        "none" => "不思考",
        "minimal" => "最少",
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        "xhigh" => "很高",
        "max" => "最高",
        other => other,
    }
    .to_string()
}

/// A prompt's content: its text (references written out) and images, in order of appearance.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Prompt {
    pub text: String,
    /// (MIME type, base64 data)
    pub images: Vec<(String, String)>,
}

/// ZJ's prompt blocks (`text`, `image`, `resource_link`, `resource`) as one text plus images.
/// References are written the way the npm adapters wrote them (`[@name](uri)`, the selected
/// text in a `<context>` block), so the agents see what they saw before.
pub(crate) fn prompt(blocks: &Value) -> Prompt {
    let mut parts = Vec::new();
    let mut images = Vec::new();
    for b in blocks.as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => parts.push(b["text"].as_str().unwrap_or("").to_string()),
            Some("image") => {
                if let (Some(mime), Some(data)) = (b["mimeType"].as_str(), b["data"].as_str()) {
                    images.push((mime.to_string(), data.to_string()));
                }
            }
            Some("resource_link") => {
                let uri = b["uri"].as_str().unwrap_or("");
                parts.push(link(b["name"].as_str(), uri));
            }
            Some("resource") => {
                let r = &b["resource"];
                if let Some(text) = r["text"].as_str() {
                    let uri = r["uri"].as_str().unwrap_or("");
                    parts.push(format!(
                        "{}\n<context ref=\"{uri}\">\n{text}\n</context>",
                        link(None, uri)
                    ));
                }
            }
            _ => {}
        }
    }
    Prompt {
        text: parts.join("\n\n"),
        images,
    }
}

/// `[@name](uri)`; without a name, the file's name.
fn link(name: Option<&str>, uri: &str) -> String {
    let name = name.filter(|n| !n.is_empty()).map_or_else(
        || {
            let path = uri.strip_prefix("file://").unwrap_or(uri);
            let path = path.split('#').next().unwrap_or(path).trim_end_matches('/');
            path.rsplit('/').next().unwrap_or(path).to_string()
        },
        String::from,
    );
    format!("[@{name}]({uri})")
}

/// The last `n` lines of `text`.
pub(crate) fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    let mut out = lines[start..].join("\n");
    if start > 0 {
        out.insert_str(0, "…\n");
    }
    out
}

/// A tool call's text output as ACP content (the last lines only).
pub(crate) fn output_content(text: &str) -> Option<Value> {
    (!text.trim().is_empty()).then(|| {
        json!([{ "type": "content", "content": { "type": "text", "text": tail(text, OUTPUT_LINES) } }])
    })
}

pub(crate) fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// A random (version 4) UUID.
pub(crate) fn uuid() -> String {
    use std::io::Read;
    let mut b = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if read.is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        b = (t ^ (u128::from(std::process::id()) << 64)).to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The value of a select setting in a `configOptions` list.
#[cfg(test)]
pub(crate) fn current(configs: &[Value], id: &str) -> Option<String> {
    configs
        .iter()
        .find(|c| c["id"] == id)
        .and_then(|c| c["currentValue"].as_str())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_keep_order_and_split_out_images() {
        let blocks = json!([
            { "type": "text", "text": "看看这个" },
            { "type": "image", "mimeType": "image/png", "data": "AAAA" },
            { "type": "resource", "resource": { "uri": "file:///p/a%20b.rs#L2-3", "text": "let x = 1;" } },
            { "type": "resource_link", "uri": "file:///p/src/", "name": "src" },
            { "type": "resource_link", "uri": "file:///p/a.rs" },
        ]);
        let p = prompt(&blocks);
        assert_eq!(
            p.text,
            "看看这个\n\n[@a%20b.rs](file:///p/a%20b.rs#L2-3)\n<context ref=\"file:///p/a%20b.rs#L2-3\">\nlet x = 1;\n</context>\n\n[@src](file:///p/src/)\n\n[@a.rs](file:///p/a.rs)"
        );
        assert_eq!(p.images, vec![("image/png".into(), "AAAA".into())]);
    }

    #[test]
    fn tails_output_and_makes_uuids() {
        assert_eq!(tail("a\nb\nc", 2), "…\nb\nc");
        assert_eq!(tail("a", 2), "a");
        assert!(output_content("  \n").is_none());
        let id = uuid();
        assert_eq!((id.len(), &id[14..15]), (36, "4"));
        assert_ne!(uuid(), id);
    }

    #[test]
    fn options_have_the_shapes_agent_client_reads() {
        let c = select(
            "model",
            "模型",
            "model",
            "b",
            &[("a".into(), "A".into()), ("b".into(), "B".into())],
        );
        assert_eq!(c["options"][1], json!({ "value": "b", "name": "B" }));
        assert_eq!(current(&[c], "model").as_deref(), Some("b"));
        let init = initialize_result("x", "X", json!([]));
        assert_eq!(init["_meta"]["steering"]["supported"], true);
        assert!(init["agentCapabilities"]["sessionCapabilities"]["close"].is_object());
        let answer =
            json!({ "result": { "outcome": { "outcome": "selected", "optionId": "allow" } } });
        assert_eq!(chosen_option(&answer), Some("allow"));
        assert_eq!(
            chosen_option(&json!({ "result": { "outcome": { "outcome": "cancelled" } } })),
            None
        );
    }
}
