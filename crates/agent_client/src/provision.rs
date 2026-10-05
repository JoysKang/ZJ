//! First-use installs for agents whose ACP adapter is an npm package. The adapter goes into
//! ZJ's data directory (`npm install --prefix`, then it runs as `node <entry>`), installed
//! with this machine's npm, or with a pinned Node.js that ZJ downloads when there is none.
//! Only the system `curl`, `tar` and `shasum` / `sha256sum` are used; nothing runs through
//! npx, so no `npm exec` process stays resident next to the agent.
//!
//! Every install is staged in a temporary sibling directory and renamed into place once it is
//! complete (marked with [`MARKER`]), so an interrupted install never looks finished.

use std::{
    env,
    ffi::OsString,
    fs,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Node.js LTS downloaded when this machine has none (2026-09-07).
pub const NODE_VERSION: &str = "24.21.0";
const NODE_DOWNLOAD_MB: u32 = 53;
const MARKER: &str = ".zj-installed";
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(20 * 60);
const EXTRACT_LIMIT: Duration = Duration::from_secs(5 * 60);
const NPM_LIMIT: Duration = Duration::from_secs(15 * 60);
const VERSION_LIMIT: Duration = Duration::from_secs(5);
/// Last part of a failed step's stderr shown to the user.
const ERROR_TAIL: usize = 600;

/// `nodejs.org` build name and the sha256 of its `.tar.gz` (from `SHASUMS256.txt`).
fn node_build() -> Option<(&'static str, &'static str)> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => (
            "darwin-arm64",
            "bed7eea5325e1108f32ce5228ddd6a5f0f08a499ee42aa7442aea583702f6057",
        ),
        ("macos", "x86_64") => (
            "darwin-x64",
            "1462cb3b3046b815cf8ea436d3da450ec1a9f11dac7e5a46b0ada5305d7e8097",
        ),
        ("linux", "aarch64") => (
            "linux-arm64",
            "724282c3b43aec998aa9527380465b45d229e021b58035f5f4f63095eabfe5d5",
        ),
        ("linux", "x86_64") => (
            "linux-x64",
            "6e1db87ef58b8819e5d5402eff1536491b18edd8eb7bee5ef7897876e88dc5ff",
        ),
        _ => return None,
    })
}

pub(crate) fn can_download_node() -> bool {
    node_build().is_some()
}

/// macOS: `~/Library/Application Support/ZJ/agents`; elsewhere `$XDG_DATA_HOME/zj/agents`
/// (or `~/.local/share/zj/agents`).
pub fn default_root() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    if cfg!(target_os = "macos") {
        return Some(home.join("Library/Application Support/ZJ/agents"));
    }
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    Some(data.join("zj/agents"))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallError {
    /// The user cancelled or the client is shutting down.
    Aborted,
    Failed(String),
}

impl From<String> for InstallError {
    fn from(message: String) -> Self {
        InstallError::Failed(message)
    }
}

/// The `bin` directory of ZJ's own Node.js, when a complete install exists.
pub(crate) fn managed_node_bin(root: &Path) -> Option<PathBuf> {
    let dir = root.join("node").join(format!("v{NODE_VERSION}"));
    (dir.join(MARKER).is_file() && dir.join("bin/node").is_file()).then(|| dir.join("bin"))
}

/// `@scope/name@1.2.3` → (`@scope/name`, `1.2.3`); a spec without a version gives `""`.
pub(crate) fn split_spec(spec: &str) -> (&str, &str) {
    match spec.get(1..).and_then(|rest| rest.rfind('@')) {
        Some(i) => (&spec[..=i], &spec[i + 2..]),
        None => (spec, ""),
    }
}

fn dir_prefix(spec: &str) -> String {
    format!("{}@", split_spec(spec).0.replace('/', "+"))
}

/// `full` installs keep npm's optional dependencies (the CLI binary the adapter bundles);
/// lean ones rely on the CLI installed on this machine.
fn package_dir(root: &Path, spec: &str, full: bool) -> PathBuf {
    let version = split_spec(spec).1;
    let suffix = if full { "+full" } else { "" };
    root.join("packages")
        .join(format!("{}{version}{suffix}", dir_prefix(spec)))
}

/// The script npm installed for `bin` (from the package's `package.json`).
fn entry(dir: &Path, spec: &str, bin: &str) -> Option<PathBuf> {
    let package = dir.join("node_modules").join(split_spec(spec).0);
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(package.join("package.json")).ok()?).ok()?;
    let relative = match &json["bin"] {
        serde_json::Value::String(path) => path.clone(),
        serde_json::Value::Object(bins) => bins
            .get(bin)
            .or_else(|| bins.values().next())?
            .as_str()?
            .to_string(),
        _ => return None,
    };
    let path = package.join(relative);
    path.is_file().then_some(path)
}

/// The entry of a complete install. A full install also serves a request for a lean one.
pub(crate) fn installed_entry(root: &Path, spec: &str, bin: &str, full: bool) -> Option<PathBuf> {
    let variants: &[bool] = if full { &[true] } else { &[false, true] };
    variants.iter().find_map(|&full| {
        let dir = package_dir(root, spec, full);
        if !dir.join(MARKER).is_file() {
            return None;
        }
        entry(&dir, spec, bin)
    })
}

/// A fresh staging directory next to the final one; removed on drop unless renamed away.
struct Staging(PathBuf);

impl Staging {
    fn new(parent: &Path) -> Result<Self, String> {
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = parent.join(format!(
            ".tmp-{}-{nanos}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).map_err(|e| format!("无法创建安装目录 {}：{e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Moves a finished install into place (keeping an install another window finished first) and
/// removes older versions of the same thing.
fn publish(from: &Path, to: &Path, prefix: &str) -> Result<(), String> {
    if !to.join(MARKER).is_file() {
        let _ = fs::remove_dir_all(to);
        fs::write(from.join(MARKER), b"").map_err(|e| e.to_string())?;
        if let Err(e) = fs::rename(from, to)
            && !to.join(MARKER).is_file()
        {
            return Err(format!("无法完成安装（{}）：{e}", to.display()));
        }
    }
    let keep = to.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(parent) = to.parent() else {
        return Ok(());
    };
    let version = keep
        .as_deref()
        .map(|k| k.trim_end_matches("+full").to_string());
    for entry in fs::read_dir(parent).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let same_version = version
            .as_deref()
            .is_some_and(|v| name.trim_end_matches("+full") == v);
        if name.starts_with(prefix) && !same_version {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

fn kill_group(child: &Child) {
    // SAFETY: the child was started with process_group(0), so -pid only reaches it and its
    // descendants (npm's install scripts); kill() has no memory-safety preconditions.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
}

fn tail(text: &str) -> String {
    let text = text.trim();
    let mut start = text.len().saturating_sub(ERROR_TAIL);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

/// Runs one install step in its own process group; polled so it can be aborted.
fn run(
    mut command: Command,
    what: &str,
    limit: Duration,
    abort: &dyn Fn() -> bool,
) -> Result<(), InstallError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| InstallError::Failed(format!("{what}失败：{e}")))?;
    let reader = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        })
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(InstallError::Failed(format!("{what}失败：{e}"))),
        }
        let timed_out = started.elapsed() > limit;
        if abort() || timed_out {
            kill_group(&child);
            let _ = child.wait();
            return Err(if timed_out {
                InstallError::Failed(format!("{what}超时"))
            } else {
                InstallError::Aborted
            });
        }
        thread::sleep(Duration::from_millis(100));
    };
    let stderr = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let detail = tail(&stderr);
    Err(InstallError::Failed(if detail.is_empty() {
        format!("{what}失败（{status}）")
    } else {
        format!("{what}失败：{detail}")
    }))
}

fn sha256(path: &Path) -> Result<String, String> {
    let attempts: [(&str, &[&str]); 2] = [("shasum", &["-a", "256"]), ("sha256sum", &[])];
    for (program, args) in attempts {
        let Ok(output) = Command::new(program)
            .args(args)
            .arg(path)
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(hash) = text.split_whitespace().next() {
                return Ok(hash.to_ascii_lowercase());
            }
        }
    }
    Err("无法计算校验和：没有找到 shasum 或 sha256sum".into())
}

/// Downloads, verifies and unpacks the pinned Node.js; returns its `bin` directory.
pub(crate) fn install_node(
    root: &Path,
    progress: &dyn Fn(String),
    abort: &dyn Fn() -> bool,
) -> Result<PathBuf, InstallError> {
    if let Some(bin) = managed_node_bin(root) {
        return Ok(bin);
    }
    let (build, expected) = node_build().ok_or_else(|| {
        format!(
            "这个平台不能自动下载 Node.js，请自行安装 Node.js {} 或更新版本",
            crate::registry::MIN_NODE.0
        )
    })?;
    let parent = root.join("node");
    fs::create_dir_all(&parent).map_err(|e| format!("无法创建 {}：{e}", parent.display()))?;
    let staging = Staging::new(&parent)?;
    let archive = staging.0.join("node.tar.gz");
    let name = format!("node-v{NODE_VERSION}-{build}");
    progress(format!(
        "首次使用：正在下载 Node.js {NODE_VERSION}（约 {NODE_DOWNLOAD_MB} MB）"
    ));
    eprintln!("event=agent_node_download version={NODE_VERSION} build={build}");
    let mut curl = Command::new("curl");
    curl.args([
        "-fL",
        "--silent",
        "--show-error",
        "--retry",
        "2",
        "--connect-timeout",
        "20",
        "-o",
    ])
    .arg(&archive)
    .arg(format!(
        "https://nodejs.org/dist/v{NODE_VERSION}/{name}.tar.gz"
    ));
    run(curl, "下载 Node.js", DOWNLOAD_LIMIT, abort)?;
    progress("正在校验并解压 Node.js".into());
    let actual = sha256(&archive)?;
    if actual != expected {
        eprintln!("event=agent_node_checksum_mismatch version={NODE_VERSION}");
        return Err("下载的 Node.js 校验和不对，已丢弃。请稍后重试"
            .to_string()
            .into());
    }
    let mut tar = Command::new("tar");
    tar.arg("-xzf").arg(&archive).arg("-C").arg(&staging.0);
    run(tar, "解压 Node.js", EXTRACT_LIMIT, abort)?;
    let final_dir = parent.join(format!("v{NODE_VERSION}"));
    publish(&staging.0.join(&name), &final_dir, "v")?;
    managed_node_bin(root).ok_or_else(|| "解压后没有找到 node".to_string().into())
}

/// `npm install`s `spec` into ZJ's directory with the npm next to `node_bin`; returns the
/// entry script for `bin`.
pub(crate) fn install_package(
    root: &Path,
    spec: &str,
    bin: &str,
    full: bool,
    node_bin: &Path,
    abort: &dyn Fn() -> bool,
) -> Result<PathBuf, InstallError> {
    if let Some(entry) = installed_entry(root, spec, bin, full) {
        return Ok(entry);
    }
    let npm = node_bin.join("npm");
    if !npm.is_file() {
        return Err(format!("{} 里没有 npm", node_bin.display()).into());
    }
    let parent = root.join("packages");
    fs::create_dir_all(&parent).map_err(|e| format!("无法创建 {}：{e}", parent.display()))?;
    let staging = Staging::new(&parent)?;
    let mut path: Vec<PathBuf> = vec![node_bin.to_path_buf()];
    path.extend(["/usr/bin", "/bin"].map(PathBuf::from));
    let mut command = Command::new(&npm);
    command
        .arg("install")
        .arg("--prefix")
        .arg(&staging.0)
        .args([
            "--no-audit",
            "--no-fund",
            "--no-package-lock",
            "--no-save",
            "--omit=dev",
            "--loglevel=error",
        ])
        .args((!full).then_some("--omit=optional"))
        .arg(spec)
        .current_dir(&staging.0)
        .env("PATH", env::join_paths(path).unwrap_or_else(|_| OsString::from("/usr/bin:/bin")))
        .env("npm_config_update_notifier", "false")
        // A user-wide prefix (e.g. from mise) must not redirect the install.
        .env_remove("npm_config_prefix")
        .env_remove("NPM_CONFIG_PREFIX");
    eprintln!("event=agent_package_install full={full}");
    run(command, "安装适配器", NPM_LIMIT, abort)?;
    if entry(&staging.0, spec, bin).is_none() {
        return Err(format!("安装完成，但没有找到 {bin} 的入口").into());
    }
    let final_dir = package_dir(root, spec, full);
    publish(&staging.0, &final_dir, &dir_prefix(spec))?;
    installed_entry(root, spec, bin, full)
        .ok_or_else(|| format!("安装完成，但没有找到 {bin} 的入口").into())
}

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
                kill_group(&child);
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
pub(crate) mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("zj-provision-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What `npm install` leaves behind for a package with one bin.
    pub(crate) fn fake_install(root: &Path, spec: &str, bin: &str, full: bool) -> PathBuf {
        let dir = package_dir(root, spec, full);
        let package = dir.join("node_modules").join(split_spec(spec).0);
        fs::create_dir_all(package.join("dist")).unwrap();
        fs::write(
            package.join("package.json"),
            format!(r#"{{"name":"x","bin":{{"{bin}":"dist/index.js"}}}}"#),
        )
        .unwrap();
        fs::write(package.join("dist/index.js"), "").unwrap();
        fs::write(dir.join(MARKER), "").unwrap();
        package.join("dist/index.js")
    }

    #[test]
    fn specs_split_into_name_and_version() {
        assert_eq!(
            split_spec("@agentclientprotocol/codex-acp@2.1.1"),
            ("@agentclientprotocol/codex-acp", "2.1.1")
        );
        assert_eq!(split_spec("left-pad@1.3.0"), ("left-pad", "1.3.0"));
        assert_eq!(split_spec("@scope/name"), ("@scope/name", ""));
        assert_eq!(split_spec(""), ("", ""));
        let root = Path::new("/r");
        assert_eq!(
            package_dir(root, "@a/b@1.0.0", true),
            Path::new("/r/packages/@a+b@1.0.0+full")
        );
    }

    #[test]
    fn only_complete_installs_count_and_full_serves_lean() {
        let root = temp("installed");
        let spec = "@a/tool@1.0.0";
        assert_eq!(installed_entry(&root, spec, "tool", false), None);
        let full = fake_install(&root, spec, "tool", true);
        assert_eq!(
            installed_entry(&root, spec, "tool", false),
            Some(full.clone())
        );
        assert_eq!(installed_entry(&root, spec, "tool", true), Some(full));
        let lean = fake_install(&root, spec, "tool", false);
        assert_eq!(installed_entry(&root, spec, "tool", false), Some(lean));
        // Without the marker an install is unfinished.
        fs::remove_file(package_dir(&root, spec, true).join(MARKER)).unwrap();
        assert_eq!(installed_entry(&root, spec, "tool", true), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn string_bins_and_missing_entries() {
        let root = temp("bins");
        let dir = root.join("p");
        let package = dir.join("node_modules/solo");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("package.json"), r#"{"bin":"cli.js"}"#).unwrap();
        assert_eq!(entry(&dir, "solo@1.0.0", "solo"), None);
        fs::write(package.join("cli.js"), "").unwrap();
        assert_eq!(
            entry(&dir, "solo@1.0.0", "other"),
            Some(package.join("cli.js"))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publishing_replaces_other_versions() {
        let root = temp("publish");
        let packages = root.join("packages");
        fake_install(&root, "@a/b@0.9.0", "b", false);
        fake_install(&root, "@a/b@1.0.0", "b", true);
        fake_install(&root, "@a/other@0.1.0", "o", false);
        let staging = packages.join(".tmp-x");
        fs::create_dir_all(&staging).unwrap();
        publish(&staging, &package_dir(&root, "@a/b@1.0.0", false), "@a+b@").unwrap();
        let mut names: Vec<String> = fs::read_dir(&packages)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["@a+b@1.0.0", "@a+b@1.0.0+full", "@a+other@0.1.0"]);
        assert!(packages.join("@a+b@1.0.0").join(MARKER).is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn steps_report_failures_and_can_be_aborted() {
        let mut fail = Command::new("sh");
        fail.args(["-c", "echo boom >&2; exit 3"]);
        match run(fail, "测试", Duration::from_secs(10), &|| false) {
            Err(InstallError::Failed(message)) => assert!(message.contains("boom"), "{message}"),
            other => panic!("{other:?}"),
        }
        let mut slow = Command::new("sleep");
        slow.arg("30");
        let started = Instant::now();
        assert_eq!(
            run(slow, "测试", Duration::from_secs(60), &|| true),
            Err(InstallError::Aborted)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let mut ok = Command::new("true");
        ok.arg("x");
        assert_eq!(run(ok, "测试", Duration::from_secs(10), &|| false), Ok(()));
    }

    #[test]
    fn checksums_and_versions() {
        let root = temp("sha");
        let file = root.join("abc");
        fs::write(&file, "abc").unwrap();
        assert_eq!(
            sha256(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(parse_version("2.1.285 (Claude Code)"), Some((2, 1, 285)));
        assert_eq!(parse_version("codex-cli 0.159.0"), Some((0, 159, 0)));
        assert_eq!(parse_version("v24.21.0-rc1"), Some((24, 21, 0)));
        assert_eq!(parse_version("no version"), None);
        let _ = fs::remove_dir_all(&root);
    }
}
