//! Codex's built-in `/` commands, as codex-acp offered them: a prompt starting with one of
//! these runs it instead of a turn. Reviews, compaction and goals run as turns of their own
//! (the prompt ends with that turn); the others answer at once with a message.

use super::{Bridge, Pending};
use crate::acp::{self, INTERNAL};
use serde_json::{Value, json};
use std::time::Instant;

/// (name, description, input hint).
pub(super) const COMMANDS: [(&str, &str, Option<&str>); 11] = [
    (
        "review",
        "审查未提交的改动，或按说明审查",
        Some("可选的审查说明"),
    ),
    (
        "review-branch",
        "审查相对某个基础分支的改动",
        Some("分支名"),
    ),
    ("review-commit", "审查某个提交", Some("提交 SHA")),
    ("compact", "压缩对话，避免超出上下文", None),
    ("plan", "开启或关闭规划协作模式", None),
    (
        "goal",
        "设定要持续推进的目标",
        Some("<目标> | pause | resume | clear"),
    ),
    ("status", "显示会话设置和用量", None),
    ("mcp", "列出配置的 MCP 服务", None),
    ("skills", "列出可用的技能", None),
    ("rename", "重命名这个会话", Some("新名字")),
    ("logout", "退出 Codex 登录", None),
];

/// What a command's request answers with.
pub(super) enum Reply {
    /// A turn of its own starts: the prompt ends with it.
    Turn,
    /// Done: say this.
    Say(String),
    /// `/plan` switched: say so.
    Plan(bool),
    /// `mcpServerStatus/list`: list the servers.
    Mcp,
    /// Signed out: say so.
    Logout,
    /// `/rename`: the user's name for the session.
    Renamed(String),
}

/// The built-in command a prompt starts with: (name, the rest). Names ignore case.
pub(super) fn parse(text: &str) -> Option<(&'static str, &str)> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let (name, ..) = COMMANDS
        .iter()
        .find(|(n, ..)| n.eq_ignore_ascii_case(name))?;
    Some((name, args.trim()))
}

fn hint(name: &str) -> &'static str {
    COMMANDS
        .iter()
        .find(|(n, ..)| *n == name)
        .and_then(|c| c.2)
        .unwrap_or("参数")
}

/// The commands for `available_commands_update`, before the workspace's skills.
pub(super) fn listed() -> Vec<Value> {
    COMMANDS
        .iter()
        .map(|(name, description, hint)| {
            json!({ "name": name, "description": description, "input": hint.map(|h| json!({ "hint": h })) })
        })
        .collect()
}

impl Bridge {
    /// Runs `/name rest` for ZJ's prompt `id`.
    pub(super) fn command(&mut self, id: Value, thread: &str, name: &str, rest: &str) {
        let usage = |hint: &str| format!("「/{name}」需要：{hint}。");
        let review =
            |target: Value| json!({ "threadId": thread, "target": target, "delivery": "inline" });
        let (method, params, reply) = match (name, rest) {
            ("review", "") => (
                "review/start",
                review(json!({ "type": "uncommittedChanges" })),
                Reply::Turn,
            ),
            ("review", text) => (
                "review/start",
                review(json!({ "type": "custom", "instructions": text })),
                Reply::Turn,
            ),
            ("review-branch" | "review-commit" | "rename" | "goal", "") => {
                return say(id, thread, &usage(hint(name)));
            }
            ("plan", args) if !args.is_empty() => {
                return say(id, thread, "「/plan」不带参数：开启或关闭规划模式。");
            }
            ("review-branch", branch) => (
                "review/start",
                review(json!({ "type": "baseBranch", "branch": branch })),
                Reply::Turn,
            ),
            ("review-commit", sha) => (
                "review/start",
                review(json!({ "type": "commit", "sha": sha, "title": null })),
                Reply::Turn,
            ),
            ("compact", _) => (
                "thread/compact/start",
                json!({ "threadId": thread }),
                Reply::Turn,
            ),
            ("goal", "clear") => (
                "thread/goal/clear",
                json!({ "threadId": thread }),
                Reply::Say("目标已清除。".into()),
            ),
            ("goal", "pause") => (
                "thread/goal/set",
                json!({ "threadId": thread, "status": "paused" }),
                Reply::Say("目标已暂停。".into()),
            ),
            ("goal", "resume") => (
                "thread/goal/set",
                json!({ "threadId": thread, "status": "active" }),
                Reply::Turn,
            ),
            ("goal", objective) if objective.chars().count() > 4000 => {
                return say(id, thread, "目标最多 4000 个字符。");
            }
            ("goal", objective) => (
                "thread/goal/set",
                json!({ "threadId": thread, "objective": objective, "status": "active" }),
                Reply::Turn,
            ),
            ("plan", _) => {
                let Some(t) = self.threads.get(thread) else {
                    return acp::reply_error(id, INTERNAL, "会话已关闭");
                };
                let on = !t.plan;
                let settings = json!({
                    "model": t.model(&self.models).map(|m| m.id.clone()),
                    "reasoning_effort": t.effort(&self.models),
                    "developer_instructions": null,
                });
                let mode = if on { "plan" } else { "default" };
                let params = json!({
                    "threadId": thread,
                    "collaborationMode": { "mode": mode, "settings": settings },
                });
                ("thread/settings/update", params, Reply::Plan(on))
            }
            ("status", _) => {
                let text = self.status(thread);
                return say(id, thread, &text);
            }
            ("mcp", _) => ("mcpServerStatus/list", json!({}), Reply::Mcp),
            ("skills", _) => {
                let skills = self
                    .threads
                    .get(thread)
                    .map(|t| t.skills.clone())
                    .unwrap_or_default();
                let text = if skills.is_empty() {
                    "没有可用的技能。".to_string()
                } else {
                    let names: Vec<String> = skills.iter().map(|(n, _)| format!("- {n}")).collect();
                    format!("可用的技能：\n{}", names.join("\n"))
                };
                return say(id, thread, &text);
            }
            ("rename", name) => (
                "thread/name/set",
                json!({ "threadId": thread, "name": name }),
                Reply::Renamed(name.to_string()),
            ),
            ("logout", _) => ("account/logout", json!({}), Reply::Logout),
            _ => return acp::reply_error(id, INTERNAL, &format!("不支持 /{name}")),
        };
        let Some(t) = self.threads.get_mut(thread) else {
            return acp::reply_error(id, INTERNAL, "会话已关闭");
        };
        t.begin(id);
        if let Reply::Turn = reply {
            t.expect_turn = Some((Instant::now(), name == "goal"));
        }
        self.request(method, params, Pending::Command(thread.to_string(), reply));
    }

    /// A command's request answered.
    pub(super) fn command_done(
        &mut self,
        thread: &str,
        reply: Reply,
        result: Result<Value, String>,
    ) {
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                let Some(t) = self.threads.get_mut(thread) else {
                    return;
                };
                t.expect_turn = None;
                if let Some(id) = t.prompt.take() {
                    acp::reply_error(id, INTERNAL, &e);
                }
                return;
            }
        };
        let text = match reply {
            Reply::Turn => {
                // A review answers with its turn; the others announce it with `turn/started`.
                if let Some(turn) = result["turn"]["id"].as_str() {
                    self.turn_known(thread, turn);
                }
                return;
            }
            Reply::Say(text) => text,
            Reply::Renamed(name) => {
                if let Some(t) = self.threads.get_mut(thread) {
                    t.renamed = true;
                }
                format!("已重命名为「{name}」。")
            }
            Reply::Logout => {
                self.signed_in = Some(false);
                "已退出 Codex 登录。".to_string()
            }
            Reply::Plan(on) => {
                if let Some(t) = self.threads.get_mut(thread) {
                    t.plan = on;
                }
                if on {
                    "已开启规划模式：先给出计划，不直接改动。"
                } else {
                    "已关闭规划模式。"
                }
                .to_string()
            }
            Reply::Mcp => {
                let lines: Vec<String> = result["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| {
                        let tools = s["tools"].as_object().map_or(0, |t| t.len());
                        let resources = s["resources"].as_array().map_or(0, Vec::len);
                        let auth = s["authStatus"].as_str().unwrap_or("unknown");
                        format!(
                            "- {}：{tools} 个工具，{resources} 个资源，认证 {auth}",
                            s["name"].as_str().unwrap_or("?")
                        )
                    })
                    .collect();
                if lines.is_empty() {
                    "没有配置 MCP 服务。".to_string()
                } else {
                    format!("配置的 MCP 服务：\n{}", lines.join("\n"))
                }
            }
        };
        if let Some(id) = self.threads.get_mut(thread).and_then(|t| t.prompt.take()) {
            say(id, thread, &text);
        }
    }

    fn status(&self, thread: &str) -> String {
        let Some(t) = self.threads.get(thread) else {
            return String::new();
        };
        let model = t.model(&self.models);
        let mode = super::MODES
            .iter()
            .find(|m| m.0 == t.mode)
            .map_or(t.mode.as_str(), |m| m.1);
        let effort = t
            .effort(&self.models)
            .map_or_else(|| "默认".into(), |e| acp::effort_name(&e));
        let mut lines = vec![
            format!("**模型：** {}", model.map_or("未知", |m| m.name.as_str())),
            format!("**思考强度：** {effort}"),
            format!("**快速模式：** {}", if t.fast { "开" } else { "关" }),
            format!("**模式：** {mode}{}", if t.plan { " · 规划" } else { "" }),
            format!("**账号：** {}", self.account),
        ];
        if let Some(tokens) = t.tokens {
            lines.push(format!("**本会话用量：** {} tokens", tokens.thread));
            if let Some(window) = tokens.window.filter(|w| *w > 0) {
                lines.push(format!("**上下文：** {} / {window} tokens", tokens.context));
            }
        }
        lines.join("\n")
    }
}

/// Answers prompt `id` with a message.
pub(super) fn say(id: Value, thread: &str, text: &str) {
    acp::update(thread, acp::text_chunk("agent_message_chunk", text));
    acp::reply(id, json!({ "stopReason": "end_turn" }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse_from_the_prompt() {
        assert_eq!(parse("/review"), Some(("review", "")));
        assert_eq!(
            parse("  /review-branch  main "),
            Some(("review-branch", "main"))
        );
        assert_eq!(
            parse("/goal 修好测试\n再提交"),
            Some(("goal", "修好测试\n再提交"))
        );
        assert_eq!(parse("/archify x"), None);
        assert_eq!(parse("review"), None);
        assert_eq!(listed().len(), COMMANDS.len());
        assert_eq!(listed()[0]["input"]["hint"], "可选的审查说明");
    }
}
