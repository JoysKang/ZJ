//! Built-in agent presets, user-defined agents and launch resolution.
//!
//! Every agent speaks ACP over stdio. Presets prefer a natively installed binary and fall back
//! to `npx -y <package>` (pinned to the versions in the ACP registry, 2026-10-01). Nothing is
//! installed automatically; a missing Node.js or binary is reported with an install hint.

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

/// Monochrome Lucide glyph shown for the agent (the UI maps it to an icon).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Glyph {
    /// Claude Code — `asterisk`.
    Claude,
    /// Codex — `square-terminal`.
    Codex,
    /// Gemini CLI — `sparkle`.
    Gemini,
    /// Claude Code pointed at DeepSeek — `fish`.
    DeepSeek,
    /// User-defined agents — `bot`.
    Generic,
}

impl Glyph {
    pub fn lucide_name(self) -> &'static str {
        match self {
            Glyph::Claude => "asterisk",
            Glyph::Codex => "square-terminal",
            Glyph::Gemini => "sparkle",
            Glyph::DeepSeek => "fish",
            Glyph::Generic => "bot",
        }
    }
}

/// How an environment variable for the agent gets its value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvValue {
    Literal(String),
    /// Copied from ZJ's own environment (or an override); launching fails with a clear message
    /// when it is unset. Keys are never stored in presets.
    FromEnv(String),
}

/// One way to start an agent; presets list several and the first available one is used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    /// An executable found on the search path (or an absolute path).
    Binary { program: String, args: Vec<String> },
    /// `npx -y <package> <args>`; needs Node.js. `-y` matters: stdin carries JSON-RPC, so an
    /// install prompt would hang the agent.
    Npx { package: String, args: Vec<String> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPreset {
    pub id: String,
    pub display_name: String,
    pub glyph: Glyph,
    pub launch: Vec<Launch>,
    pub env: Vec<(String, EnvValue)>,
    /// Shown when no launch option is available.
    pub install_hint: String,
    /// Which session modes ZJ starts in and never selects.
    pub modes: ModePolicy,
    /// `_meta` sent with `session/new` and `session/load` (agent-specific switches).
    pub session_meta: Option<serde_json::Value>,
}

/// ZJ starts every session in an asking mode and never requests one that skips approvals,
/// whatever the agent's own settings say.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModePolicy {
    /// Switched to right after the session starts when the agent offers it.
    pub initial: Option<String>,
    /// Hidden from the mode picker and refused by `set_mode`.
    pub forbidden: Vec<String>,
}

impl ModePolicy {
    fn new(initial: &str, forbidden: &[&str]) -> Self {
        Self {
            initial: Some(initial.to_string()),
            forbidden: forbidden.iter().map(|m| m.to_string()).collect(),
        }
    }

    pub fn allows(&self, mode: &str) -> bool {
        !self.forbidden.iter().any(|f| f == mode)
    }
}

/// claude-agent-acp: with this the adapter leaves `bypassPermissions` out of the mode list
/// and ignores it as `permissions.defaultMode` from the user's Claude settings.
fn claude_meta() -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "claudeCode": { "options": { "allowDangerouslySkipPermissions": false } }
    }))
}

fn claude_modes() -> ModePolicy {
    ModePolicy::new("default", &["bypassPermissions"])
}

pub const CLAUDE_ACP_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp@0.85.0";
pub const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp@2.1.1";
pub const GEMINI_PACKAGE: &str = "@google/gemini-cli@0.62.0";

fn s(v: &str) -> String {
    v.to_string()
}

/// Claude Code, Codex, Gemini CLI and "Claude Code · DeepSeek".
pub fn builtin_presets() -> Vec<AgentPreset> {
    let claude_launch = vec![
        Launch::Binary {
            program: s("claude-agent-acp"),
            args: vec![],
        },
        Launch::Npx {
            package: s(CLAUDE_ACP_PACKAGE),
            args: vec![],
        },
    ];
    let deepseek_model = "deepseek-v4-pro[1m]";
    let deepseek_fast = "deepseek-v4-flash[1m]";
    vec![
        AgentPreset {
            id: s("claude-code"),
            display_name: s("Claude Code"),
            glyph: Glyph::Claude,
            launch: claude_launch.clone(),
            env: vec![],
            install_hint: s(
                "安装 Node.js 18 或更新版本（例如 brew install node），然后在终端运行一次 claude 完成登录",
            ),
            modes: claude_modes(),
            session_meta: claude_meta(),
        },
        AgentPreset {
            id: s("codex"),
            display_name: s("Codex"),
            glyph: Glyph::Codex,
            launch: vec![
                Launch::Binary {
                    program: s("codex-acp"),
                    args: vec![],
                },
                Launch::Npx {
                    package: s(CODEX_ACP_PACKAGE),
                    args: vec![],
                },
            ],
            // codex-acp otherwise starts in "agent" (auto review).
            env: vec![(s("INITIAL_AGENT_MODE"), EnvValue::Literal(s("read-only")))],
            install_hint: s(
                "安装 Node.js 18 或更新版本（例如 brew install node），然后在终端运行 codex login",
            ),
            modes: ModePolicy::new("read-only", &["agent-full-access"]),
            session_meta: None,
        },
        AgentPreset {
            id: s("gemini"),
            display_name: s("Gemini CLI"),
            glyph: Glyph::Gemini,
            launch: vec![
                Launch::Binary {
                    program: s("gemini"),
                    args: vec![s("--acp")],
                },
                Launch::Npx {
                    package: s(GEMINI_PACKAGE),
                    args: vec![s("--acp")],
                },
            ],
            env: vec![],
            install_hint: s(
                "安装 Node.js 20 或更新版本，然后运行 npm install -g @google/gemini-cli 并登录",
            ),
            modes: ModePolicy::new("default", &["yolo"]),
            session_meta: None,
        },
        AgentPreset {
            id: s("claude-code-deepseek"),
            display_name: s("Claude Code · DeepSeek"),
            glyph: Glyph::DeepSeek,
            launch: claude_launch,
            env: vec![
                (
                    s("ANTHROPIC_BASE_URL"),
                    EnvValue::Literal(s("https://api.deepseek.com/anthropic")),
                ),
                (
                    s("ANTHROPIC_AUTH_TOKEN"),
                    EnvValue::FromEnv(s("DEEPSEEK_API_KEY")),
                ),
                (s("ANTHROPIC_MODEL"), EnvValue::Literal(s(deepseek_model))),
                (
                    s("ANTHROPIC_DEFAULT_OPUS_MODEL"),
                    EnvValue::Literal(s(deepseek_model)),
                ),
                (
                    s("ANTHROPIC_DEFAULT_SONNET_MODEL"),
                    EnvValue::Literal(s(deepseek_model)),
                ),
                (
                    s("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
                    EnvValue::Literal(s(deepseek_fast)),
                ),
                (
                    s("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
                    EnvValue::Literal(s("1")),
                ),
            ],
            install_hint: s(
                "安装 Node.js 18 或更新版本，并在设置或环境变量 DEEPSEEK_API_KEY 里提供 DeepSeek API Key",
            ),
            modes: claude_modes(),
            session_meta: claude_meta(),
        },
    ]
}

/// A user-defined agent from the settings file (`agents` array).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserAgentConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Literal values; `"$NAME"` copies `NAME` from ZJ's environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl UserAgentConfig {
    pub fn into_preset(self) -> AgentPreset {
        let env = self
            .env
            .into_iter()
            .map(|(k, v)| match v.strip_prefix('$') {
                Some(name) if !name.is_empty() => (k, EnvValue::FromEnv(name.to_string())),
                _ => (k, EnvValue::Literal(v)),
            })
            .collect();
        AgentPreset {
            display_name: self.name.unwrap_or_else(|| self.id.clone()),
            id: self.id,
            glyph: Glyph::Generic,
            launch: vec![Launch::Binary {
                program: self.command,
                args: self.args,
            }],
            env,
            install_hint: "检查设置里的 command 是否正确".to_string(),
            // Unknown agents: never pick a mode for them, but refuse the usual bypass names.
            modes: ModePolicy {
                initial: None,
                forbidden: [
                    "bypassPermissions",
                    "yolo",
                    "agent-full-access",
                    "full-access",
                ]
                .map(String::from)
                .to_vec(),
            },
            session_meta: None,
        }
    }
}

/// Fully resolved process launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Variables set on top of the sanitized inherited environment (includes `PATH`).
    pub env: Vec<(String, OsString)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// Only npx launch options are left and Node.js is not installed.
    NodeMissing { agent: String, hint: String },
    NotFound {
        agent: String,
        program: String,
        hint: String,
    },
    MissingEnv {
        agent: String,
        variable: String,
        source: String,
    },
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::NodeMissing { agent, hint } => write!(
                f,
                "启动「{agent}」需要 Node.js（npx），但没有找到。{hint}。"
            ),
            LaunchError::NotFound {
                agent,
                program,
                hint,
            } => write!(f, "没有找到「{agent}」的启动命令 {program}。{hint}。"),
            LaunchError::MissingEnv {
                agent,
                variable,
                source,
            } => write!(
                f,
                "「{agent}」需要 {variable}：请在设置里填写，或设置环境变量 {source}。"
            ),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Directories searched for agent binaries. A desktop launch on macOS inherits only
/// `/usr/bin:/bin:/usr/sbin:/sbin`, so the usual Node / Homebrew locations are added.
#[derive(Clone, Debug)]
pub struct SearchPath {
    dirs: Vec<PathBuf>,
}

impl SearchPath {
    pub fn from_env() -> Self {
        let mut dirs: Vec<PathBuf> = env::var_os("PATH")
            .map(|p| env::split_paths(&p).collect())
            .unwrap_or_default();
        let home = env::var_os("HOME").map(PathBuf::from);
        let mut extra = vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ];
        if let Some(home) = &home {
            for rel in [
                ".local/bin",
                ".volta/bin",
                ".bun/bin",
                ".npm-global/bin",
                ".asdf/shims",
                "Library/pnpm",
                ".local/share/pnpm",
            ] {
                extra.push(home.join(rel));
            }
            if let Some(nvm) = newest_nvm_bin(&home.join(".nvm/versions/node")) {
                extra.push(nvm);
            }
        }
        dirs.extend(extra);
        Self::new(dirs)
    }

    pub fn new(dirs: Vec<PathBuf>) -> Self {
        let mut unique: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if !dir.as_os_str().is_empty() && !unique.contains(&dir) {
                unique.push(dir);
            }
        }
        Self { dirs: unique }
    }

    pub fn find(&self, program: &str) -> Option<PathBuf> {
        if program.contains('/') {
            let path = PathBuf::from(program);
            return is_executable(&path).then_some(path);
        }
        self.dirs
            .iter()
            .map(|dir| dir.join(program))
            .find(|path| is_executable(path))
    }

    /// `PATH` for the child: existing search directories, with `first` (where node / the
    /// agent was found) in front so `#!/usr/bin/env node` resolves to the same install.
    fn path_env(&self, first: Option<&Path>) -> OsString {
        let mut dirs: Vec<&Path> = Vec::new();
        if let Some(first) = first {
            dirs.push(first);
        }
        for dir in &self.dirs {
            if dir.is_dir() && !dirs.contains(&dir.as_path()) {
                dirs.push(dir);
            }
        }
        env::join_paths(dirs).unwrap_or_default()
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// `~/.nvm/versions/node/vX.Y.Z/bin` with the highest version.
fn newest_nvm_bin(root: &Path) -> Option<PathBuf> {
    let parse = |name: &str| -> Option<Vec<u64>> {
        name.strip_prefix('v')?
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    };
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Some((parse(&name)?, e.path().join("bin")))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

impl AgentPreset {
    pub fn find_builtin(id: &str) -> Option<AgentPreset> {
        builtin_presets().into_iter().find(|p| p.id == id)
    }

    /// Picks the first available launch option and fills in the environment. `overrides`
    /// (from the settings file) win over [`EnvValue::FromEnv`] lookups.
    pub fn resolve(
        &self,
        search: &SearchPath,
        overrides: &BTreeMap<String, String>,
    ) -> Result<ResolvedLaunch, LaunchError> {
        let mut env_vars: Vec<(String, OsString)> = Vec::new();
        for (key, value) in &self.env {
            let resolved = match (overrides.get(key), value) {
                (Some(v), _) => OsString::from(v),
                (None, EnvValue::Literal(v)) => OsString::from(v),
                (None, EnvValue::FromEnv(source)) => match env::var_os(source) {
                    Some(v) if !v.is_empty() => v,
                    _ => {
                        return Err(LaunchError::MissingEnv {
                            agent: self.display_name.clone(),
                            variable: key.clone(),
                            source: source.clone(),
                        });
                    }
                },
            };
            env_vars.push((key.clone(), resolved));
        }
        let mut missing_program = None;
        let mut wanted_node = false;
        for launch in &self.launch {
            match launch {
                Launch::Binary { program, args } => {
                    if let Some(path) = search.find(program) {
                        env_vars.push(("PATH".into(), search.path_env(path.parent())));
                        return Ok(ResolvedLaunch {
                            program: path,
                            args: args.clone(),
                            env: env_vars,
                        });
                    }
                    missing_program.get_or_insert_with(|| program.clone());
                }
                Launch::Npx { package, args } => {
                    wanted_node = true;
                    let (Some(npx), Some(node)) = (search.find("npx"), search.find("node")) else {
                        continue;
                    };
                    env_vars.push(("PATH".into(), search.path_env(node.parent())));
                    let mut all_args = vec!["-y".to_string(), package.clone()];
                    all_args.extend(args.iter().cloned());
                    return Ok(ResolvedLaunch {
                        program: npx,
                        args: all_args,
                        env: env_vars,
                    });
                }
            }
        }
        if wanted_node {
            return Err(LaunchError::NodeMissing {
                agent: self.display_name.clone(),
                hint: self.install_hint.clone(),
            });
        }
        Err(LaunchError::NotFound {
            agent: self.display_name.clone(),
            program: missing_program.unwrap_or_default(),
            hint: self.install_hint.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_bin(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("zj-registry-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn prefers_native_binary_then_npx() {
        let root = temp("prefer");
        let node_dir = root.join("node/bin");
        let search = SearchPath::new(vec![node_dir.clone(), root.join("native")]);
        let claude = AgentPreset::find_builtin("claude-code").unwrap();
        let none = BTreeMap::new();
        assert!(matches!(
            claude.resolve(&search, &none),
            Err(LaunchError::NodeMissing { .. })
        ));
        let npx = fake_bin(&node_dir, "npx");
        fake_bin(&node_dir, "node");
        let resolved = claude.resolve(&search, &none).unwrap();
        assert_eq!(resolved.program, npx);
        assert_eq!(
            resolved.args,
            vec!["-y".to_string(), CLAUDE_ACP_PACKAGE.into()]
        );
        let path = resolved.env.iter().find(|(k, _)| k == "PATH").unwrap();
        assert!(
            path.1
                .to_string_lossy()
                .starts_with(&*node_dir.to_string_lossy())
        );
        let native = fake_bin(&root.join("native"), "claude-agent-acp");
        assert_eq!(claude.resolve(&search, &none).unwrap().program, native);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deepseek_needs_a_key_from_settings_or_env() {
        let root = temp("deepseek");
        let node_dir = root.join("bin");
        fake_bin(&node_dir, "npx");
        fake_bin(&node_dir, "node");
        let search = SearchPath::new(vec![node_dir]);
        let preset = AgentPreset::find_builtin("claude-code-deepseek").unwrap();
        if env::var_os("DEEPSEEK_API_KEY").is_none() {
            let err = preset.resolve(&search, &BTreeMap::new()).unwrap_err();
            assert!(err.to_string().contains("DEEPSEEK_API_KEY"), "{err}");
        }
        let overrides =
            BTreeMap::from([("ANTHROPIC_AUTH_TOKEN".to_string(), "sk-test".to_string())]);
        let resolved = preset.resolve(&search, &overrides).unwrap();
        let get = |k: &str| {
            resolved
                .env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(
            get("ANTHROPIC_BASE_URL").unwrap(),
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(get("ANTHROPIC_AUTH_TOKEN").unwrap(), "sk-test");
        assert_eq!(get("ANTHROPIC_MODEL").unwrap(), "deepseek-v4-pro[1m]");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_agents_parse_from_settings_json() {
        let config: UserAgentConfig = serde_json::from_str(
            r#"{"id":"opencode","name":"OpenCode","command":"opencode","args":["acp"],"env":{"A":"1","B":"$HOME"}}"#,
        )
        .unwrap();
        let preset = config.into_preset();
        assert_eq!(preset.display_name, "OpenCode");
        assert_eq!(preset.glyph, Glyph::Generic);
        assert!(
            preset
                .env
                .contains(&("B".into(), EnvValue::FromEnv("HOME".into())))
        );
        let missing = preset
            .resolve(&SearchPath::new(vec![]), &BTreeMap::new())
            .unwrap_err();
        assert!(
            matches!(missing, LaunchError::NotFound { ref program, .. } if program == "opencode")
        );
    }

    #[test]
    fn presets_start_asking_and_forbid_bypass() {
        for preset in builtin_presets() {
            assert!(preset.modes.initial.is_some(), "{}", preset.id);
            assert!(!preset.modes.forbidden.is_empty(), "{}", preset.id);
            assert!(
                preset
                    .modes
                    .allows(preset.modes.initial.as_deref().unwrap())
            );
        }
        let claude = AgentPreset::find_builtin("claude-code").unwrap();
        assert!(!claude.modes.allows("bypassPermissions"));
        let meta = claude.session_meta.unwrap();
        assert_eq!(
            meta["claudeCode"]["options"]["allowDangerouslySkipPermissions"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn every_preset_has_a_glyph_and_hint() {
        for preset in builtin_presets() {
            assert!(!preset.glyph.lucide_name().is_empty());
            assert!(!preset.install_hint.is_empty());
            assert!(!preset.launch.is_empty());
        }
    }
}
