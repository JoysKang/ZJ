//! ACP `terminal` login methods: the agent's own command plus the method's arguments, run
//! interactively in Terminal.app (the user signs in there; ZJ retries the session after).

use crate::registry::ResolvedLaunch;
use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

/// Whether terminal logins can be offered (advertised as `auth.terminal`).
pub(crate) const SUPPORTED: bool = cfg!(target_os = "macos");

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A `.command` script for Terminal.app. Only `PATH` and the variables named in `keep` are
/// copied from the launch environment: the rest may hold API keys, which must not end up in
/// a file.
pub(crate) fn script(
    launch: &ResolvedLaunch,
    args: &[String],
    env: &HashMap<String, String>,
    keep: &[&str],
    cwd: &Path,
) -> String {
    let mut lines = vec![
        "#!/bin/sh".to_string(),
        r#"rm -f "$0""#.to_string(),
        format!("cd {} || exit 1", quote(&cwd.to_string_lossy())),
    ];
    for (key, value) in &launch.env {
        if key == "PATH" || keep.contains(&key.as_str()) {
            lines.push(format!("export {key}={}", quote(&value.to_string_lossy())));
        }
    }
    let mut env: Vec<_> = env.iter().collect();
    env.sort();
    for (key, value) in env {
        if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            lines.push(format!("export {key}={}", quote(value)));
        }
    }
    let command: Vec<String> = std::iter::once(launch.program.to_string_lossy().into_owned())
        .chain(launch.args.iter().cloned())
        .chain(args.iter().cloned())
        .map(|part| quote(&part))
        .collect();
    lines.push(command.join(" "));
    lines.push("status=$?".into());
    lines.push("echo".into());
    lines.push(
        r#"if [ $status -eq 0 ]; then echo "登录完成，可以回到 ZJ 点「已登录，重试」，并关闭这个窗口。"; else echo "登录没有完成（退出码 $status）。"; fi"#
            .into(),
    );
    lines.join("\n") + "\n"
}

/// Writes the script (owner-only) and opens it in Terminal.app.
pub(crate) fn open_in_terminal(script: &str) -> io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "zj-login-{}-{}.command",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, script)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    let status = Command::new("open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        let _ = fs::remove_file(&path);
        return Err(io::Error::other(format!("open 退出码 {status}")));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn scripts_quote_and_leave_secrets_out() {
        let launch = ResolvedLaunch {
            program: "/opt/node/bin/node".into(),
            args: vec!["/data/it's/index.js".into()],
            env: vec![
                ("PATH".into(), OsString::from("/opt/node/bin:/usr/bin")),
                ("CLAUDE_CODE_EXECUTABLE".into(), OsString::from("/b/claude")),
                ("ANTHROPIC_AUTH_TOKEN".into(), OsString::from("sk-secret")),
            ],
        };
        let env = HashMap::from([
            ("MODE".to_string(), "x".to_string()),
            ("BAD;rm".to_string(), "y".to_string()),
        ]);
        let text = script(
            &launch,
            &["--cli".into(), "auth".into()],
            &env,
            &["CLAUDE_CODE_EXECUTABLE"],
            Path::new("/w"),
        );
        assert!(
            text.contains("export PATH='/opt/node/bin:/usr/bin'"),
            "{text}"
        );
        assert!(text.contains("export CLAUDE_CODE_EXECUTABLE='/b/claude'"));
        assert!(text.contains("export MODE='x'"));
        assert!(!text.contains("sk-secret"));
        assert!(!text.contains("BAD"));
        assert!(text.contains(r"'/opt/node/bin/node' '/data/it'\''s/index.js' '--cli' 'auth'"));
        let output = Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&text)
            .output()
            .unwrap();
        assert!(output.status.success(), "{text}");
    }
}
