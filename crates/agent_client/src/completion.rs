//! One isolated text-generation turn, using the same configured ACP agent and process pool.

use crate::{AgentClient, AgentEvent, ClientOptions, PromptPart, TurnOutcome};
use std::time::Duration;

/// Returns only the completed answer (never thoughts or tool output). Dropping the future
/// closes the session; a deadline also covers agents that stop responding or require login.
pub async fn generate_text(mut options: ClientOptions, prompt: String) -> Result<String, String> {
    options.text_only = true;
    options.buffers = None;
    options.resume_session = None;
    let client = AgentClient::start(options).map_err(|e| format!("无法启动 Agent：{e}"))?;
    let turn = client
        .prompt(vec![PromptPart::Text(prompt)])
        .map_err(|e| e.to_string())?;
    let events = client.events();
    let answer = async {
        let mut text = String::new();
        while let Ok(event) = events.recv().await {
            match event {
                AgentEvent::MessageChunk { text: chunk } => {
                    if text.len().saturating_add(chunk.len()) > 16 * 1024 {
                        return Err("Agent 生成的提交信息过长，请重试".into());
                    }
                    text.push_str(&chunk);
                }
                AgentEvent::TurnEnded { turn: id, outcome } if id == turn => {
                    return match outcome {
                        TurnOutcome::EndTurn if !text.trim().is_empty() => {
                            Ok(text.trim().to_string())
                        }
                        TurnOutcome::EndTurn => Err("Agent 没有返回提交信息，请重试".into()),
                        TurnOutcome::Failed(error) => Err(error),
                        _ => Err("Agent 未完成生成，请重试".into()),
                    };
                }
                AgentEvent::AuthRequired { .. } => {
                    return Err("请先在 Agent 面板完成登录，再生成提交信息".into());
                }
                AgentEvent::Error { message } => return Err(message),
                AgentEvent::Exited { .. } => return Err("Agent 已退出，请重试".into()),
                _ => {}
            }
        }
        Err("Agent 连接已关闭，请重试".into())
    };
    match crate::client::with_timeout(answer, Duration::from_secs(180)).await {
        Some(result) => result,
        None => {
            client.cancel();
            Err("生成提交信息超时，请重试".into())
        }
    }
}
