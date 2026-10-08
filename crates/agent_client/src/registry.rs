//! Built-in agent presets, user-defined agents and launch resolution.
//!
//! Every agent speaks ACP over stdio. Claude Code and Codex run behind ZJ's own bridge
//! (`--agent-bridge`, the `agent_bridge` crate, docs/adr/0009) in front of their CLI on this
//! machine; ZJ never installs a CLI, Node.js or an adapter. A missing or outdated CLI is a
//! [`LaunchError`] with an install hint, which the panel shows as soon as the agent is picked.

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
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

/// How to start an agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    /// An ACP agent: an executable found on the search path (or an absolute path).
    Binary { program: String, args: Vec<String> },
    /// ZJ's bridge (`kind`: `claude` or `codex`) in front of the agent's CLI on this machine.
    Bridge { kind: String, cli: Cli },
}

/// An agent CLI the bridge drives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cli {
    pub program: String,
    /// Names another executable to use instead (settings or ZJ's environment), as the
    /// adapters did: `CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`.
    pub env: String,
    /// Older releases lack parts of the protocol the bridge uses.
    pub min_version: (u64, u64, u64),
}

/// The argument that turns ZJ's executable into the bridge (`agent_bridge::FLAG`).
pub const BRIDGE_FLAG: &str = "--agent-bridge";
/// Points the bridge launch at another executable than ZJ's own (tests, development).
pub const BRIDGE_ENV: &str = "ZJ_AGENT_BRIDGE";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPreset {
    pub id: String,
    pub display_name: String,
    pub glyph: Glyph,
    pub launch: Launch,
    pub env: Vec<(String, EnvValue)>,
    /// Shown when the agent can't start here (not installed, too old).
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

fn claude_modes() -> ModePolicy {
    ModePolicy::new("default", &["bypassPermissions"])
}

const CLAUDE_HINT: &str = "安装 Claude Code 后重新选择：brew install --cask claude-code，或 curl -fsSL https://claude.ai/install.sh | bash";
const CODEX_HINT: &str =
    "安装 Codex 后重新选择：brew install --cask codex，或 npm i -g @openai/codex";

fn s(v: &str) -> String {
    v.to_string()
}

fn claude_launch() -> Launch {
    Launch::Bridge {
        kind: s("claude"),
        cli: Cli {
            program: s("claude"),
            env: s("CLAUDE_CODE_EXECUTABLE"),
            min_version: (2, 1, 0),
        },
    }
}

fn codex_cli() -> Cli {
    Cli {
        program: s("codex"),
        env: s("CODEX_PATH"),
        min_version: (0, 159, 0),
    }
}

/// Claude Code, Codex and "Claude Code · DeepSeek".
pub fn builtin_presets() -> Vec<AgentPreset> {
    let deepseek_model = "deepseek-v4-pro[1m]";
    let deepseek_fast = "deepseek-v4-flash[1m]";
    vec![
        AgentPreset {
            id: s("claude-code"),
            display_name: s("Claude Code"),
            glyph: Glyph::Claude,
            launch: claude_launch(),
            env: vec![],
            install_hint: s(CLAUDE_HINT),
            modes: claude_modes(),
            session_meta: None,
        },
        AgentPreset {
            id: s("codex"),
            display_name: s("Codex"),
            glyph: Glyph::Codex,
            launch: Launch::Bridge {
                kind: s("codex"),
                cli: codex_cli(),
            },
            env: vec![],
            install_hint: s(CODEX_HINT),
            // "Workspace access": edits in the workspace, asks before writing outside it or
            // using the network.
            modes: ModePolicy::new("workspace-write", &["agent-full-access"]),
            session_meta: None,
        },
        AgentPreset {
            id: s("claude-code-deepseek"),
            display_name: s("Claude Code · DeepSeek"),
            glyph: Glyph::DeepSeek,
            launch: claude_launch(),
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
            install_hint: s(CLAUDE_HINT),
            modes: claude_modes(),
            session_meta: None,
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
    /// Literal values; `"$NAME"` copies `NAME` from ZJ's environment. The app resolves these
    /// like `agent.env` before launch (`keychain:ACCOUNT`, plaintext secrets refused).
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
            launch: Launch::Binary {
                program: self.command,
                args: self.args,
            },
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
    NotFound {
        agent: String,
        program: String,
        hint: String,
    },
    /// The CLI is older than the bridge needs (or didn't say its version). Versions as
    /// `x.y.z`; `found` is empty when unknown.
    Outdated {
        agent: String,
        program: String,
        found: String,
        needed: String,
        hint: String,
    },
    MissingEnv {
        agent: String,
        variable: String,
        source: String,
    },
    /// ZJ's own executable (the bridge) couldn't be located.
    NoBridge { agent: String, reason: String },
}

fn version_text((major, minor, patch): (u64, u64, u64)) -> String {
    format!("{major}.{minor}.{patch}")
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::NotFound {
                agent,
                program,
                hint,
            } => write!(
                f,
                "本机没有安装「{agent}」（找不到 {program} 命令）。{hint}。"
            ),
            LaunchError::Outdated {
                agent,
                program,
                found,
                needed,
                hint,
            } => {
                let found = if found.is_empty() {
                    "未知版本"
                } else {
                    found
                };
                write!(
                    f,
                    "本机的「{agent}」（{program} {found}）太旧，需要 {needed} 或更新的版本。{hint}。"
                )
            }
            LaunchError::MissingEnv {
                agent,
                variable,
                source,
            } => write!(
                f,
                "「{agent}」需要 {variable}：请在设置里填写，或设置环境变量 {source}。"
            ),
            LaunchError::NoBridge { agent, reason } => {
                write!(
                    f,
                    "无法启动「{agent}」：找不到 ZJ 自己的可执行文件（{reason}）。"
                )
            }
        }
    }
}

impl std::error::Error for LaunchError {}

/// Directories searched for agent binaries. A desktop launch on macOS inherits only
/// `/usr/bin:/bin:/usr/sbin:/sbin`, so the usual Node / Homebrew / version-manager locations
/// are added.
#[derive(Clone, Debug, PartialEq, Eq)]
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
            if let Some(nvm) = newest_version_bin(&home.join(".nvm/versions/node")) {
                extra.push(nvm);
            }
        }
        // mise: the install itself first (`npm i -g` puts the adapters there), then its shims.
        let mise = env::var_os("MISE_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share/mise")));
        if let Some(mise) = mise {
            if let Some(node) = newest_version_bin(&mise.join("installs/node")) {
                extra.push(node);
            }
            extra.push(mise.join("shims"));
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

/// `<root>/X.Y.Z/bin` with the highest version (nvm names them `vX.Y.Z`); aliases such as
/// `latest` are skipped.
fn newest_version_bin(root: &Path) -> Option<PathBuf> {
    let parse = |name: &str| -> Option<Vec<u64>> {
        name.strip_prefix('v')
            .unwrap_or(name)
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

    /// The command to start, with the environment filled in. `overrides` (from the settings
    /// file) win over [`EnvValue::FromEnv`] lookups; ones the preset does not declare (e.g.
    /// `ANTHROPIC_API_KEY` for Claude Code) are passed on as they are. Checking a bridged
    /// CLI's version runs it (`--version`): not on the UI thread.
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
        env_vars.extend(
            overrides
                .iter()
                .filter(|(key, _)| !self.env.iter().any(|(declared, _)| declared == *key))
                .map(|(key, value)| (key.clone(), OsString::from(value))),
        );
        let not_found = |program: &str| LaunchError::NotFound {
            agent: self.display_name.clone(),
            program: program.to_string(),
            hint: self.install_hint.clone(),
        };
        match &self.launch {
            Launch::Binary { program, args } => {
                let path = search.find(program).ok_or_else(|| not_found(program))?;
                env_vars.push(("PATH".into(), search.path_env(path.parent())));
                Ok(ResolvedLaunch {
                    program: path,
                    args: args.clone(),
                    env: env_vars,
                })
            }
            Launch::Bridge { kind, cli } => {
                let configured = overrides
                    .get(&cli.env)
                    .map(OsString::from)
                    .or_else(|| env::var_os(&cli.env))
                    .filter(|p| !p.is_empty());
                let program = configured
                    .as_ref()
                    .map_or(cli.program.clone(), |p| p.to_string_lossy().into_owned());
                let path = search.find(&program).ok_or_else(|| not_found(&program))?;
                let found = cli_version(&path);
                if found.is_none_or(|v| v < cli.min_version) {
                    return Err(LaunchError::Outdated {
                        agent: self.display_name.clone(),
                        program,
                        found: found.map(version_text).unwrap_or_default(),
                        needed: version_text(cli.min_version),
                        hint: self.install_hint.clone(),
                    });
                }
                let bridge = bridge_program().map_err(|reason| LaunchError::NoBridge {
                    agent: self.display_name.clone(),
                    reason,
                })?;
                env_vars.push(("PATH".into(), search.path_env(path.parent())));
                Ok(ResolvedLaunch {
                    program: bridge,
                    args: vec![
                        BRIDGE_FLAG.to_string(),
                        kind.clone(),
                        path.to_string_lossy().into_owned(),
                    ],
                    env: env_vars,
                })
            }
        }
    }
}

/// ZJ's own executable (or [`BRIDGE_ENV`]).
fn bridge_program() -> Result<PathBuf, String> {
    match env::var_os(BRIDGE_ENV).filter(|p| !p.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => env::current_exe().map_err(|e| e.to_string()),
    }
}

/// `codex app-server` for the short-lived quota connection: the configured or installed
/// Codex CLI, when it is recent enough.
pub(crate) fn codex_app_server(
    search: &SearchPath,
    overrides: &BTreeMap<String, String>,
    inherited: Option<&std::ffi::OsStr>,
) -> Option<ResolvedLaunch> {
    let cli = codex_cli();
    let configured = overrides
        .get(&cli.env)
        .map(std::ffi::OsStr::new)
        .or(inherited.filter(|p| !p.is_empty()));
    let program = match configured {
        Some(path) => search.find(path.to_str()?)?,
        None => search
            .find(&cli.program)
            .filter(|p| cli_version(p).is_some_and(|v| v >= cli.min_version))?,
    };
    let mut env: Vec<(String, OsString)> = overrides
        .iter()
        .map(|(k, v)| (k.clone(), OsString::from(v)))
        .collect();
    env.push(("PATH".into(), search.path_env(None)));
    Some(ResolvedLaunch {
        program,
        args: vec!["app-server".into()],
        env,
    })
}

/// How long `<cli> --version` may take.
const VERSION_LIMIT: Duration = Duration::from_secs(5);

/// `x.y.z` from `<program> --version` (e.g. `2.1.285 (Claude Code)`, `codex-cli 0.159.0`).
pub(crate) fn cli_version(program: &Path) -> Option<(u64, u64, u64)> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < VERSION_LIMIT => {
                thread::sleep(Duration::from_millis(20))
            }
            _ => {
                // SAFETY: the child was started with process_group(0), so -pid only reaches
                // it and its descendants; kill() has no memory-safety preconditions.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                return None;
            }
        }
    }
    parse_version(&reader.join().ok()?)
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    text.split_whitespace().find_map(|word| {
        let mut parts = word.trim_start_matches('v').split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch: String = parts
            .next()?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        Some((major, minor, patch.parse().ok()?))
    })
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

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = fake_bin(dir, name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        path
    }

    fn env_of<'a>(env: &'a [(String, OsString)], key: &str) -> Option<&'a OsString> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    #[test]
    fn versions_parse_from_cli_output() {
        assert_eq!(parse_version("2.1.285 (Claude Code)"), Some((2, 1, 285)));
        assert_eq!(parse_version("codex-cli 0.159.0"), Some((0, 159, 0)));
        assert_eq!(parse_version("v24.21.0-rc1"), Some((24, 21, 0)));
        assert_eq!(parse_version("no version"), None);
    }

    #[test]
    fn bridged_agents_need_a_recent_cli_and_never_install() {
        if env::var_os("CODEX_PATH").is_some() {
            return;
        }
        let root = temp("bridge");
        let bin = root.join("bin");
        let search = SearchPath::new(vec![bin.clone()]);
        let codex = AgentPreset::find_builtin("codex").unwrap();
        let none = BTreeMap::new();
        let missing = codex.resolve(&search, &none).unwrap_err();
        assert!(matches!(missing, LaunchError::NotFound { ref program, .. } if program == "codex"));
        assert!(
            missing.to_string().contains("brew install --cask codex"),
            "{missing}"
        );
        script(&bin, "codex", "echo codex-cli 0.158.9");
        let old = codex.resolve(&search, &none).unwrap_err();
        assert!(matches!(old, LaunchError::Outdated { ref found, .. } if found == "0.158.9"));
        assert!(old.to_string().contains("0.159.0"), "{old}");
        let cli = script(&bin, "codex", "echo codex-cli 0.160.1");
        let launch = codex.resolve(&search, &none).unwrap();
        assert_eq!(
            launch.args,
            [
                BRIDGE_FLAG.to_string(),
                "codex".into(),
                cli.display().to_string()
            ]
        );
        assert!(
            env_of(&launch.env, "PATH")
                .unwrap()
                .to_string_lossy()
                .starts_with(&*bin.to_string_lossy())
        );
        // The user's own executable wins.
        let custom = script(&root.join("custom"), "my-codex", "echo codex-cli 0.161.0");
        let overrides = BTreeMap::from([("CODEX_PATH".to_string(), custom.display().to_string())]);
        assert_eq!(
            codex.resolve(&search, &overrides).unwrap().args[2],
            custom.display().to_string()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quota_uses_the_configured_or_a_recent_codex() {
        let root = temp("quota");
        let bin = root.join("bin");
        let cli = script(&bin, "custom-codex", "echo codex-cli 0.159.0");
        let old = script(&bin, "codex", "echo codex-cli 0.158.9");
        let search = SearchPath::new(vec![bin.clone()]);
        let overrides = BTreeMap::from([("CODEX_PATH".into(), cli.display().to_string())]);
        assert_eq!(
            codex_app_server(&search, &overrides, None).unwrap().program,
            cli
        );
        assert_eq!(
            codex_app_server(&search, &BTreeMap::new(), Some(cli.as_os_str()))
                .unwrap()
                .program,
            cli
        );
        assert!(codex_app_server(&search, &BTreeMap::new(), None).is_none());
        script(&bin, "codex", "echo codex-cli 0.160.0");
        let launch = codex_app_server(&search, &BTreeMap::new(), None).unwrap();
        assert_eq!(
            (launch.program, launch.args),
            (old, vec!["app-server".to_string()])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn newest_version_manager_install_is_found() {
        let root = temp("versions");
        for name in ["v18.20.1", "v22.3.0", "22.10.0", "9.0.0", "latest"] {
            std::fs::create_dir_all(root.join(name).join("bin")).unwrap();
        }
        assert_eq!(
            newest_version_bin(&root),
            Some(root.join("22.10.0").join("bin"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deepseek_needs_a_key_from_settings_or_env() {
        if env::var_os("CLAUDE_CODE_EXECUTABLE").is_some() {
            return;
        }
        let root = temp("deepseek");
        let bin = root.join("bin");
        script(&bin, "claude", "echo '2.1.285 (Claude Code)'");
        let search = SearchPath::new(vec![bin]);
        let preset = AgentPreset::find_builtin("claude-code-deepseek").unwrap();
        if env::var_os("DEEPSEEK_API_KEY").is_none() {
            let err = preset.resolve(&search, &BTreeMap::new()).unwrap_err();
            assert!(err.to_string().contains("DEEPSEEK_API_KEY"), "{err}");
        }
        let overrides =
            BTreeMap::from([("ANTHROPIC_AUTH_TOKEN".to_string(), "sk-test".to_string())]);
        let launch = preset.resolve(&search, &overrides).unwrap();
        let get = |k: &str| env_of(&launch.env, k).map(|v| v.to_string_lossy().into_owned());
        assert_eq!(
            get("ANTHROPIC_BASE_URL").unwrap(),
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(get("ANTHROPIC_AUTH_TOKEN").unwrap(), "sk-test");
        assert_eq!(get("ANTHROPIC_MODEL").unwrap(), "deepseek-v4-pro[1m]");
        assert_eq!(launch.args[1], "claude");
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
            assert!(!preset.install_hint.is_empty());
            assert!(!preset.glyph.lucide_name().is_empty());
        }
        let claude = AgentPreset::find_builtin("claude-code").unwrap();
        assert!(!claude.modes.allows("bypassPermissions"));
    }
}
