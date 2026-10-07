//! A short-lived Codex app-server connection for live account limits. No agent turn is run.

use crate::{SearchPath, process::AgentProcess, registry::ResolvedLaunch};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    sync::mpsc,
    time::{Duration, Instant},
};

pub fn read_codex_quota(env: BTreeMap<String, String>) -> Result<Value, String> {
    let search = env.get("PATH").map_or_else(SearchPath::from_env, |path| {
        SearchPath::new(std::env::split_paths(path).collect())
    });
    let root = crate::provision::default_root();
    let inherited = std::env::var_os("CODEX_PATH");
    let launch =
        crate::registry::codex_app_server(&search, &env, root.as_deref(), inherited.as_deref())
            .ok_or("找不到可用的 Codex CLI，无法刷新额度")?;
    query(launch, Duration::from_secs(20))
}

fn query(launch: ResolvedLaunch, timeout: Duration) -> Result<Value, String> {
    let cwd = std::env::temp_dir();
    let mut process =
        AgentProcess::spawn(&launch, &cwd).map_err(|e| format!("无法查询 Codex 额度：{e}"))?;
    let stdout = process.stdout.take().ok_or("Codex 未提供输出流")?;
    let (tx, rx) = mpsc::sync_channel(8);
    let reader = std::thread::Builder::new()
        .name("codex-quota".into())
        .spawn(move || {
            let mut lines = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                match (&mut lines)
                    .take(64 * 1024 + 1)
                    .read_until(b'\n', &mut bytes)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) if bytes.len() > 64 * 1024 => break,
                    Ok(_) => {}
                }
                let Ok(line) = String::from_utf8(bytes) else {
                    break;
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| format!("无法读取 Codex 额度：{e}"))?;
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let stdin = process.stdin.as_mut().ok_or("Codex 未提供输入流")?;
        let mut send = |message: Value| -> Result<(), String> {
            writeln!(stdin, "{message}").map_err(|e| format!("无法请求 Codex 额度：{e}"))
        };
        let receive = |id: u64| -> Result<Value, String> {
            loop {
                if Instant::now() >= deadline {
                    return Err("Codex 额度查询超时".into());
                }
                let line = rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| "Codex 额度查询超时或连接已关闭".to_string())?;
                let message: Value = serde_json::from_str(&line)
                    .map_err(|_| "Codex 额度响应格式无效".to_string())?;
                if message.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex 额度查询失败")
                        .to_string());
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or("Codex 未返回额度结果".into());
            }
        };
        send(
            json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"zj","version":env!("CARGO_PKG_VERSION")}}}),
        )?;
        receive(1)?;
        send(json!({"method":"initialized","params":{}}))?;
        send(json!({"id":2,"method":"account/rateLimits/read"}))?;
        receive(2)
    })();
    // Drop the receiver before terminating so a full queue cannot strand the reader.
    drop(rx);
    process.terminate();
    let _ = reader.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn handshake_live_query_failure_and_timeout() {
        let root = std::env::temp_dir().join(format!("zj-live-quota-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("codex");
        let run = |body: &str, timeout| {
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            query(
                ResolvedLaunch {
                    program: script.clone(),
                    args: vec![],
                    env: vec![],
                },
                timeout,
            )
        };
        let handshake = "read -r init\ncase \"$init\" in *initialize*) ;; *) exit 1;; esac\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread -r initialized\ncase \"$initialized\" in *initialized*) ;; *) exit 1;; esac\nread -r limits\ncase \"$limits\" in *account/rateLimits/read*) ;; *) exit 1;; esac";
        let valid = format!(
            "{handshake}\nprintf '%s\\n' '{{\"method\":\"notice\"}}' '{{\"id\":2,\"result\":{{\"rateLimits\":{{\"primary\":{{\"usedPercent\":17}}}}}}}}'"
        );
        assert_eq!(
            run(&valid, Duration::from_secs(2))
                .unwrap()
                .pointer("/rateLimits/primary/usedPercent"),
            Some(&json!(17))
        );
        let fail = format!(
            "{handshake}\nprintf '%s\\n' '{{\"id\":2,\"error\":{{\"message\":\"login required\"}}}}'"
        );
        assert_eq!(
            run(&fail, Duration::from_secs(2)).unwrap_err(),
            "login required"
        );
        let started = Instant::now();
        assert!(run("sleep 30", Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        std::fs::remove_dir_all(root).unwrap();
    }
}
