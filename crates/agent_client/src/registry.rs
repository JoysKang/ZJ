//! Built-in agent presets, user-defined agents and launch resolution.
//!
//! Every agent speaks ACP over stdio. Presets prefer an adapter command already installed on
//! this machine. Otherwise the adapter's npm package (pinned to the versions in the ACP
//! registry, 2026-10-01) is installed once into ZJ's data directory — with this machine's
//! Node.js, or with one ZJ downloads ([`crate::provision`]) — and run as `node <entry>`.
//! The adapter is pointed at the agent's own CLI on this machine ([`LocalCli`]) so the user's
//! version and login are used; without one, the CLI bundled in the npm package is installed.

use crate::provision;
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
    /// An npm package (`name@version`) run as `node <entry of bin> <args>` from ZJ's install
    /// directory; installed on first use. Never through npx: `npm exec` would stay resident
    /// (~120 MB) next to the agent.
    Package {
        package: String,
        bin: String,
        args: Vec<String>,
    },
}

/// The agent's CLI on this machine, handed to its adapter through `env` instead of the copy
/// bundled in the adapter's npm package. Older versions than `min_version` are not used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalCli {
    pub program: String,
    pub env: String,
    pub min_version: (u64, u64, u64),
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
    pub local_cli: Option<LocalCli>,
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

const CLAUDE_HINT: &str = "可以运行 npm i -g @agentclientprotocol/claude-agent-acp 装成本机命令，或检查网络后重试（ZJ 会自动安装）";

fn s(v: &str) -> String {
    v.to_string()
}

/// The adapters bundle Claude Code 2.1.x and Codex 0.159.x; an older minor release on this
/// machine may not understand them.
fn claude_cli() -> Option<LocalCli> {
    Some(LocalCli {
        program: s("claude"),
        env: s("CLAUDE_CODE_EXECUTABLE"),
        min_version: (2, 1, 0),
    })
}

fn codex_cli() -> Option<LocalCli> {
    Some(LocalCli {
        program: s("codex"),
        env: s("CODEX_PATH"),
        min_version: (0, 159, 0),
    })
}

/// Claude Code, Codex and "Claude Code · DeepSeek".
pub fn builtin_presets() -> Vec<AgentPreset> {
    let claude_launch = vec![
        Launch::Binary {
            program: s("claude-agent-acp"),
            args: vec![],
        },
        Launch::Package {
            package: s(CLAUDE_ACP_PACKAGE),
            bin: s("claude-agent-acp"),
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
            install_hint: s(CLAUDE_HINT),
            modes: claude_modes(),
            session_meta: claude_meta(),
            local_cli: claude_cli(),
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
                Launch::Package {
                    package: s(CODEX_ACP_PACKAGE),
                    bin: s("codex-acp"),
                    args: vec![],
                },
            ],
            // codex-acp otherwise starts in "agent" (auto review).
            env: vec![(s("INITIAL_AGENT_MODE"), EnvValue::Literal(s("read-only")))],
            install_hint: s(
                "可以运行 npm i -g @agentclientprotocol/codex-acp 装成本机命令，或检查网络后重试（ZJ 会自动安装）",
            ),
            modes: ModePolicy::new("read-only", &["agent-full-access"]),
            session_meta: None,
            local_cli: codex_cli(),
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
            install_hint: s(CLAUDE_HINT),
            modes: claude_modes(),
            session_meta: claude_meta(),
            local_cli: claude_cli(),
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
            local_cli: None,
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

/// What [`AgentPreset::resolve`] found: a command to start now, or an npm adapter (and
/// possibly Node.js) to install first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchPlan {
    Ready(ResolvedLaunch),
    Install(PackageInstall),
}

/// A first-use install, run on the client's thread with progress messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInstall {
    agent: String,
    root: PathBuf,
    package: String,
    bin: String,
    args: Vec<String>,
    full: bool,
    /// This machine's Node.js `bin` directory; `None` downloads ZJ's own.
    node_bin: Option<PathBuf>,
    /// Without `PATH`, which depends on the Node.js used.
    env: Vec<(String, OsString)>,
    search: SearchPath,
}

impl PackageInstall {
    pub(crate) fn run(
        self,
        progress: &dyn Fn(String),
        abort: &dyn Fn() -> bool,
    ) -> Result<ResolvedLaunch, provision::InstallError> {
        let node_bin = match &self.node_bin {
            Some(bin) => bin.clone(),
            None => provision::install_node(&self.root, progress, abort)?,
        };
        if provision::installed_entry(&self.root, &self.package, &self.bin, self.full).is_none() {
            let size = if self.full {
                "，包含 CLI，约 280 MB"
            } else {
                ""
            };
            progress(format!(
                "首次使用：正在安装「{}」的 ACP 适配器（{}{size}）",
                self.agent,
                provision::split_spec(&self.package).1
            ));
        }
        let entry = provision::install_package(
            &self.root,
            &self.package,
            &self.bin,
            self.full,
            &node_bin,
            abort,
        )?;
        Ok(node_launch(
            &node_bin,
            &entry,
            &self.args,
            self.env,
            &self.search,
        ))
    }
}

fn node_launch(
    node_bin: &Path,
    entry: &Path,
    args: &[String],
    mut env: Vec<(String, OsString)>,
    search: &SearchPath,
) -> ResolvedLaunch {
    env.push(("PATH".into(), search.path_env(Some(node_bin))));
    let mut all_args = vec![entry.to_string_lossy().into_owned()];
    all_args.extend(args.iter().cloned());
    ResolvedLaunch {
        program: node_bin.join("node"),
        args: all_args,
        env,
    }
}

/// claude-agent-acp needs Node.js 22; an older one on this machine is passed over.
pub(crate) const MIN_NODE: (u64, u64, u64) = (22, 0, 0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// An npm adapter is needed, there is no usable Node.js and ZJ cannot download one here.
    NodeMissing { agent: String },
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
            LaunchError::NodeMissing { agent } => write!(
                f,
                "启动「{agent}」需要 Node.js {} 或更新版本，但没有找到，这个平台也不能自动下载。请先安装 Node.js。",
                MIN_NODE.0
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

    /// Picks the first available launch option and fills in the environment. `overrides`
    /// (from the settings file) win over [`EnvValue::FromEnv`] lookups; ones the preset does
    /// not declare (e.g. `ANTHROPIC_API_KEY` for Claude Code) are passed on as they are. `root` is where npm
    /// adapters are installed ([`provision::default_root`]); without it only installed
    /// commands are used.
    pub fn resolve(
        &self,
        search: &SearchPath,
        overrides: &BTreeMap<String, String>,
        root: Option<&Path>,
    ) -> Result<LaunchPlan, LaunchError> {
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
        let (has_cli, cli_env) = self.local_cli(search, overrides);
        env_vars.extend(cli_env);
        env_vars.extend(
            overrides
                .iter()
                .filter(|(key, _)| !self.env.iter().any(|(declared, _)| declared == *key))
                .map(|(key, value)| (key.clone(), OsString::from(value))),
        );
        let mut missing_program = None;
        let mut wanted_node = false;
        for launch in &self.launch {
            match launch {
                Launch::Binary { program, args } => {
                    if let Some(path) = search.find(program) {
                        env_vars.push(("PATH".into(), search.path_env(path.parent())));
                        return Ok(LaunchPlan::Ready(ResolvedLaunch {
                            program: path,
                            args: args.clone(),
                            env: env_vars,
                        }));
                    }
                    missing_program.get_or_insert_with(|| program.clone());
                }
                Launch::Package { package, bin, args } => {
                    wanted_node = true;
                    let Some(root) = root else {
                        continue;
                    };
                    let node_bin = local_node(search);
                    let managed = provision::managed_node_bin(root);
                    let full = !has_cli;
                    if let (Some(entry), Some(node)) = (
                        provision::installed_entry(root, package, bin, full),
                        node_bin.as_ref().or(managed.as_ref()),
                    ) {
                        return Ok(LaunchPlan::Ready(node_launch(
                            node, &entry, args, env_vars, search,
                        )));
                    }
                    if node_bin.is_none() && managed.is_none() && !provision::can_download_node() {
                        continue;
                    }
                    return Ok(LaunchPlan::Install(PackageInstall {
                        agent: self.display_name.clone(),
                        root: root.to_path_buf(),
                        package: package.clone(),
                        bin: bin.clone(),
                        args: args.clone(),
                        full,
                        node_bin: node_bin.or(managed),
                        env: env_vars,
                        search: search.clone(),
                    }));
                }
            }
        }
        if wanted_node {
            return Err(LaunchError::NodeMissing {
                agent: self.display_name.clone(),
            });
        }
        Err(LaunchError::NotFound {
            agent: self.display_name.clone(),
            program: missing_program.unwrap_or_default(),
            hint: self.install_hint.clone(),
        })
    }

    /// Whether the agent's CLI is usable, and the variable that points the adapter at it.
    /// A variable the user already set (settings or ZJ's environment) is left alone.
    fn local_cli(
        &self,
        search: &SearchPath,
        overrides: &BTreeMap<String, String>,
    ) -> (bool, Option<(String, OsString)>) {
        let Some(cli) = &self.local_cli else {
            return (false, None);
        };
        if overrides.contains_key(&cli.env) || env::var_os(&cli.env).is_some_and(|v| !v.is_empty())
        {
            return (true, None);
        }
        match search.find(&cli.program) {
            Some(path) if provision::cli_version(&path).is_some_and(|v| v >= cli.min_version) => {
                (true, Some((cli.env.clone(), path.into_os_string())))
            }
            _ => (false, None),
        }
    }
}

/// This machine's Node.js `bin` directory when it is new enough and has npm next to it.
fn local_node(search: &SearchPath) -> Option<PathBuf> {
    let node = search.find("node")?;
    let bin = node.parent()?.to_path_buf();
    (bin.join("npm").is_file() && provision::cli_version(&node).is_some_and(|v| v >= MIN_NODE))
        .then_some(bin)
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

    fn ready(plan: LaunchPlan) -> ResolvedLaunch {
        match plan {
            LaunchPlan::Ready(launch) => launch,
            LaunchPlan::Install(install) => panic!("expected a ready launch: {install:?}"),
        }
    }

    fn install(plan: LaunchPlan) -> PackageInstall {
        match plan {
            LaunchPlan::Install(install) => install,
            LaunchPlan::Ready(launch) => panic!("expected an install: {launch:?}"),
        }
    }

    fn env_of<'a>(env: &'a [(String, OsString)], key: &str) -> Option<&'a OsString> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    #[test]
    fn npm_adapters_install_once_then_run_with_node() {
        let root = temp("package");
        let data = root.join("data");
        let node_dir = root.join("node/bin");
        let search = SearchPath::new(vec![node_dir.clone(), root.join("native")]);
        let claude = AgentPreset::find_builtin("claude-code").unwrap();
        let none = BTreeMap::new();
        assert!(matches!(
            claude.resolve(&search, &none, None),
            Err(LaunchError::NodeMissing { .. })
        ));
        // No Node.js here: ZJ downloads its own.
        let plan = install(claude.resolve(&search, &none, Some(&data)).unwrap());
        assert_eq!(plan.node_bin, None);
        assert!(plan.full, "no claude CLI: the bundled one is installed");

        // Too old, then new enough (with npm next to it).
        script(&node_dir, "node", "echo v18.20.0");
        fake_bin(&node_dir, "npm");
        assert_eq!(
            install(claude.resolve(&search, &none, Some(&data)).unwrap()).node_bin,
            None
        );
        script(&node_dir, "node", "echo v24.1.0");
        let plan = install(claude.resolve(&search, &none, Some(&data)).unwrap());
        assert_eq!(plan.node_bin.as_deref(), Some(node_dir.as_path()));

        let entry = crate::provision::tests::fake_install(
            &data,
            CLAUDE_ACP_PACKAGE,
            "claude-agent-acp",
            true,
        );
        let launch = ready(claude.resolve(&search, &none, Some(&data)).unwrap());
        assert_eq!(launch.program, node_dir.join("node"));
        assert_eq!(launch.args, vec![entry.to_string_lossy().into_owned()]);
        assert!(
            env_of(&launch.env, "PATH")
                .unwrap()
                .to_string_lossy()
                .starts_with(&*node_dir.to_string_lossy())
        );
        assert_eq!(env_of(&launch.env, "CLAUDE_CODE_EXECUTABLE"), None);

        let native = fake_bin(&root.join("native"), "claude-agent-acp");
        assert_eq!(
            ready(claude.resolve(&search, &none, Some(&data)).unwrap()).program,
            native
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recent_local_cli_is_handed_to_the_adapter() {
        if env::var_os("CODEX_PATH").is_some() {
            return;
        }
        let root = temp("cli");
        let data = root.join("data");
        let bin = root.join("bin");
        script(&bin, "node", "echo v22.0.0");
        fake_bin(&bin, "npm");
        let search = SearchPath::new(vec![bin.clone()]);
        let codex = AgentPreset::find_builtin("codex").unwrap();
        let none = BTreeMap::new();
        let codex_bin = script(&bin, "codex", "echo codex-cli 0.158.9");
        let plan = install(codex.resolve(&search, &none, Some(&data)).unwrap());
        assert!(plan.full);
        assert_eq!(env_of(&plan.env, "CODEX_PATH"), None);
        script(&bin, "codex", "echo codex-cli 0.159.2");
        let plan = install(codex.resolve(&search, &none, Some(&data)).unwrap());
        assert!(!plan.full, "the local CLI replaces the bundled one");
        assert_eq!(
            env_of(&plan.env, "CODEX_PATH"),
            Some(&codex_bin.into_os_string())
        );
        // A full install also serves the lean case.
        crate::provision::tests::fake_install(&data, CODEX_ACP_PACKAGE, "codex-acp", true);
        let launch = ready(codex.resolve(&search, &none, Some(&data)).unwrap());
        assert_eq!(launch.program, bin.join("node"));
        assert!(env_of(&launch.env, "CODEX_PATH").is_some());
        // The user's own choice wins.
        let overrides = BTreeMap::from([("CODEX_PATH".to_string(), "/x/codex".to_string())]);
        let launch = ready(codex.resolve(&search, &overrides, Some(&data)).unwrap());
        assert_eq!(
            env_of(&launch.env, "CODEX_PATH"),
            Some(&OsString::from("/x/codex"))
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
        let root = temp("deepseek");
        let search = SearchPath::new(vec![root.join("bin")]);
        let data = root.join("data");
        let preset = AgentPreset::find_builtin("claude-code-deepseek").unwrap();
        if env::var_os("DEEPSEEK_API_KEY").is_none() {
            let err = preset
                .resolve(&search, &BTreeMap::new(), Some(&data))
                .unwrap_err();
            assert!(err.to_string().contains("DEEPSEEK_API_KEY"), "{err}");
        }
        let overrides =
            BTreeMap::from([("ANTHROPIC_AUTH_TOKEN".to_string(), "sk-test".to_string())]);
        let plan = install(preset.resolve(&search, &overrides, Some(&data)).unwrap());
        let get = |k: &str| env_of(&plan.env, k).map(|v| v.to_string_lossy().into_owned());
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
            .resolve(&SearchPath::new(vec![]), &BTreeMap::new(), None)
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
