//! Environment overrides for agents, resolved without ever storing a key in the settings
//! file: `$NAME` copies a variable from ZJ's environment, `keychain:ACCOUNT` reads the macOS
//! Keychain item (service [`KEYCHAIN_SERVICE`]) through the system `security` tool. Runs off
//! the UI thread; values are never logged.

use std::collections::BTreeMap;

pub const KEYCHAIN_SERVICE: &str = "ZJ Agent";

/// The Terminal command that stores a key for `keychain:ACCOUNT` (shown in the settings).
pub fn keychain_hint(account: &str) -> String {
    format!("security add-generic-password -U -s \"{KEYCHAIN_SERVICE}\" -a {account} -w")
}

fn secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD"]
        .iter()
        .any(|s| upper.contains(s))
}

fn keychain(account: &str) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err(format!("钥匙串（keychain:{account}）只在 macOS 上可用"));
    }
    let output = std::process::Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
            "-w",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("无法运行 security：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "钥匙串里没有 {KEYCHAIN_SERVICE} / {account}；可在终端运行：{}",
            keychain_hint(account)
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    if value.is_empty() {
        return Err(format!("钥匙串里的 {account} 是空的"));
    }
    Ok(value)
}

/// Resolves one agent's overrides. `lookup` reads ZJ's environment (a parameter for tests).
pub fn resolve(
    overrides: &BTreeMap<String, String>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (name, source) in overrides {
        let value = if let Some(var) = source.strip_prefix('$') {
            lookup(var).filter(|v| !v.is_empty()).ok_or_else(|| {
                format!("{name} 取自环境变量 {var}，但它没有设置（从程序坞启动的 App 读不到 shell 里的变量，可改用 keychain:）")
            })?
        } else if let Some(account) = source.strip_prefix("keychain:") {
            keychain(account)?
        } else if secret_like(name) {
            return Err(format!(
                "设置里的 {name} 是明文；请改成 $环境变量名 或 keychain:账户名"
            ));
        } else {
            source.clone()
        };
        out.insert(name.clone(), value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_references_and_plain_values() {
        let overrides = BTreeMap::from([
            ("ANTHROPIC_AUTH_TOKEN".to_string(), "$MY_KEY".to_string()),
            ("ANTHROPIC_MODEL".to_string(), "deepseek-v4-pro".to_string()),
        ]);
        let env = |name: &str| (name == "MY_KEY").then(|| "sk-1".to_string());
        let resolved = resolve(&overrides, env).unwrap();
        assert_eq!(resolved["ANTHROPIC_AUTH_TOKEN"], "sk-1");
        assert_eq!(resolved["ANTHROPIC_MODEL"], "deepseek-v4-pro");
        let missing = resolve(&overrides, |_| None).unwrap_err();
        assert!(missing.contains("MY_KEY"), "{missing}");
    }

    #[test]
    fn plain_secrets_are_refused() {
        let overrides =
            BTreeMap::from([("ANTHROPIC_AUTH_TOKEN".to_string(), "sk-raw".to_string())]);
        assert!(resolve(&overrides, |_| None).unwrap_err().contains("明文"));
        if !cfg!(target_os = "macos") {
            let keychain = BTreeMap::from([("X_KEY".to_string(), "keychain:deepseek".to_string())]);
            assert!(resolve(&keychain, |_| None).unwrap_err().contains("macOS"));
        }
    }
}
