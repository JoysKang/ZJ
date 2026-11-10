//! Files the workspace index cannot see: installed packages of Python (`site-packages`),
//! JavaScript / TypeScript (`node_modules`), Rust (`$CARGO_HOME/registry/src`), Go
//! (`$GOMODCACHE`) and Ruby (installed gems). Go-to-definition reads the one file an import
//! names and parses it; nothing here is indexed, so memory does not grow with the installed
//! libraries. Pure path work plus a few `read_dir` calls; callers run it in the background.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Files the imports of `from` can name outside the workspace, in import order. Relative
/// Python imports stay inside the workspace and are not listed; `library` only finds
/// installed packages.
pub fn module_files(from: &Path, language: &str, text: &str) -> Vec<PathBuf> {
    match language {
        "python" => python_files(from, text),
        "javascript" | "typescript" | "tsx" => script_files(from, text),
        "rust" => rust_files(from, text),
        "go" => go_files(from, text),
        "ruby" => ruby_files(from, text),
        _ => Vec::new(),
    }
}

fn python_files(from: &Path, text: &str) -> Vec<PathBuf> {
    let roots = python_roots(from);
    if roots.is_empty() {
        return Vec::new();
    }
    let mut files = Vec::new();
    for module in crate::imports::python_modules(text) {
        if module.level > 0 || module.parts.is_empty() {
            continue;
        }
        for root in &roots {
            let path = module
                .parts
                .iter()
                .fold(root.to_path_buf(), |path, part| path.join(part));
            for file in [path.join("__init__.py"), path.with_extension("py")] {
                if file.is_file() && !files.contains(&file) {
                    files.push(file);
                }
            }
        }
    }
    files
}

/// `site-packages` directories an import in `from` can resolve against: those of the file's
/// own environment (when it is already inside one), then of a `.venv` / `venv` beside it or
/// above it, nearest first.
fn python_roots(from: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(directory) = from.parent() else {
        return roots;
    };
    for dir in directory.ancestors() {
        if dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == "site-packages")
        {
            push_unique(&mut roots, dir.to_path_buf());
        }
        for name in [".venv", "venv"] {
            let venv = dir.join(name);
            if venv.join("pyvenv.cfg").is_file() {
                for site in site_packages(&venv) {
                    push_unique(&mut roots, site);
                }
            }
        }
    }
    roots
}

/// The `site-packages` directories of a virtual environment root.
fn site_packages(venv: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(venv.join("lib")) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("python") {
                let site = entry.path().join("site-packages");
                if site.is_dir() {
                    found.push(site);
                }
            }
        }
    }
    // Windows layout, and venvs that keep libraries directly under `Lib`.
    let site = venv.join("Lib/site-packages");
    if site.is_dir() {
        found.push(site);
    }
    found
}

fn rust_files(from: &Path, text: &str) -> Vec<PathBuf> {
    rust_files_in(from, text, &rust_registry_roots())
}

/// The crate source trees `$CARGO_HOME/registry/src/*` holds.
fn rust_registry_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    let Some(home) = home else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = std::fs::read_dir(home.join("registry/src"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    roots.sort();
    roots
}

/// `use serde::Serialize` → the crate's `src/serde…`: the version comes from the nearest
/// `Cargo.lock`, and the path is tried from the longest module prefix down to the crate root.
fn rust_files_in(from: &Path, text: &str, registries: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if registries.is_empty() {
        return files;
    }
    let versions = cargo_lock_versions(from);
    for path in crate::imports::rust_modules(text) {
        let Some(name) = path.first() else {
            continue;
        };
        if matches!(
            name.as_str(),
            "crate" | "self" | "super" | "std" | "core" | "alloc"
        ) {
            continue;
        }
        for registry in registries {
            for crate_dir in rust_crate_dirs(registry, name, versions.get(name)) {
                for file in rust_module_files(&crate_dir, &path[1..]) {
                    push_unique(&mut files, file);
                }
            }
        }
    }
    files
}

/// Crate directories in one registry root: the locked version when known, else any version.
fn rust_crate_dirs(registry: &Path, name: &str, version: Option<&String>) -> Vec<PathBuf> {
    let names = {
        let mut names = vec![name.to_string()];
        let hyphenated = name.replace('_', "-");
        if hyphenated != name {
            names.push(hyphenated);
        }
        names
    };
    let mut dirs = Vec::new();
    for name in names {
        if let Some(version) = version {
            let dir = registry.join(format!("{name}-{version}"));
            if dir.is_dir() {
                dirs.push(dir);
                continue;
            }
        }
        let Ok(entries) = std::fs::read_dir(registry) else {
            continue;
        };
        let prefix = format!("{name}-");
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .strip_prefix(&prefix)
                    .is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit()))
            })
            .map(|entry| entry.path())
            .collect();
        found.sort();
        dirs.extend(found);
    }
    dirs
}

/// The file the module path names: the longest prefix that exists, so trailing item names
/// (`Client`) are not mistaken for modules. `a/b` → `src/a/b.rs` or `src/a/b/mod.rs`;
/// with no such file `src/a.rs` or `src/a/mod.rs`; then the crate root.
fn rust_module_files(crate_dir: &Path, segments: &[String]) -> Vec<PathBuf> {
    let src = crate_dir.join("src");
    for end in (0..=segments.len()).rev() {
        let base = segments[..end]
            .iter()
            .fold(src.clone(), |path, segment| path.join(segment));
        let files: Vec<PathBuf> = if end == 0 {
            [base.join("lib.rs"), base.join("main.rs")].to_vec()
        } else {
            [base.with_extension("rs"), base.join("mod.rs")].to_vec()
        };
        let existing: Vec<PathBuf> = files.into_iter().filter(|file| file.is_file()).collect();
        if !existing.is_empty() {
            return existing;
        }
    }
    Vec::new()
}

/// `package name = "version"` pairs from the nearest `Cargo.lock`.
fn cargo_lock_versions(from: &Path) -> HashMap<String, String> {
    let mut versions = HashMap::new();
    let Some(directory) = nearest(from.parent(), "Cargo.lock") else {
        return versions;
    };
    let Ok(text) = std::fs::read_to_string(directory.join("Cargo.lock")) else {
        return versions;
    };
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
        } else if let Some(value) = line.strip_prefix("name = ") {
            name = quoted_trimmed(value).map(str::to_string);
        } else if let Some(value) = line.strip_prefix("version = ")
            && let Some(current) = &name
        {
            if let Some(version) = quoted_trimmed(value) {
                versions.insert(current.clone(), version.to_string());
            }
            name = None;
        }
    }
    versions
}

fn go_files(from: &Path, text: &str) -> Vec<PathBuf> {
    go_files_in(from, text, go_mod_cache().as_deref())
}

/// `$GOMODCACHE`, or the default `~/go/pkg/mod`.
fn go_mod_cache() -> Option<PathBuf> {
    if let Some(cache) = std::env::var_os("GOMODCACHE") {
        return Some(PathBuf::from(cache));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("go/pkg/mod"))
}

/// Import paths resolved through the nearest `go.mod` (including `replace`) into the module
/// cache: `github.com/a/b/pkg` → `<cache>/github.com/a/b@v1.2.3/pkg/*.go`.
fn go_files_in(from: &Path, text: &str, cache: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Some(directory) = from.parent() else {
        return files;
    };
    let Some(module_dir) = nearest(Some(directory), "go.mod") else {
        return files;
    };
    let Ok(go_mod_text) = std::fs::read_to_string(module_dir.join("go.mod")) else {
        return files;
    };
    let go_mod = go_mod(&go_mod_text);
    for import in crate::imports::go_modules(text) {
        let Some((module, version)) = go_mod
            .requires
            .iter()
            .filter(|(module, _)| import == *module || import.starts_with(&format!("{module}/")))
            .max_by_key(|(module, _)| module.len())
        else {
            continue;
        };
        let subpath = import.strip_prefix(module).unwrap_or_default();
        if let Some(GoReplace::Local(local)) = go_mod.replaces.get(module) {
            let directory = module_dir.join(local).join(subpath.trim_start_matches('/'));
            push_go_package(&mut files, &normalize(&directory));
            continue;
        }
        let (module, version) = match go_mod.replaces.get(module) {
            Some(GoReplace::Module { module, version }) => (module.clone(), version.clone()),
            _ => (module.clone(), version.clone()),
        };
        let Some(cache) = cache else {
            continue;
        };
        let directory = cache
            .join(format!("{}@{}", escape_go(&module), escape_go(&version)))
            .join(subpath.trim_start_matches('/'));
        push_go_package(&mut files, &directory);
    }
    files
}

/// The `.go` files of one package directory, non-test files first.
fn push_go_package(files: &mut Vec<PathBuf>, directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut package: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "go"))
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        })
        .collect();
    package.sort();
    for file in package {
        push_unique(files, file);
    }
}

/// Resolves `.` and `..` in an absolute path (`replace … => ../local`).
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Go's module cache escapes an upper-case letter as `!` plus its lower-case form.
fn escape_go(path: &str) -> String {
    let mut escaped = String::new();
    for character in path.chars() {
        if character.is_ascii_uppercase() {
            escaped.push('!');
            escaped.push(character.to_ascii_lowercase());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[derive(Default)]
struct GoMod {
    requires: Vec<(String, String)>,
    replaces: HashMap<String, GoReplace>,
}

enum GoReplace {
    Local(PathBuf),
    Module { module: String, version: String },
}

/// `require` and `replace` directives of a `go.mod`, in either form (single line or block).
fn go_mod(text: &str) -> GoMod {
    let mut parsed = GoMod::default();
    let mut block: Option<&str> = None;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if block.is_some_and(|_| line == ")") {
            block = None;
            continue;
        }
        let (directive, rest) = if let Some(block) = block {
            (block, line)
        } else if let Some(rest) = line.strip_prefix("require") {
            ("require", rest.trim())
        } else if let Some(rest) = line.strip_prefix("replace") {
            ("replace", rest.trim())
        } else {
            continue;
        };
        if rest == "(" {
            block = Some(directive);
            continue;
        }
        match directive {
            "require" => {
                let mut words = rest.split_whitespace();
                if let (Some(module), Some(version)) = (words.next(), words.next()) {
                    parsed
                        .requires
                        .push((module.to_string(), version.to_string()));
                }
            }
            "replace" => {
                let Some((old, new)) = rest.split_once("=>") else {
                    continue;
                };
                let Some(old) = old.split_whitespace().next() else {
                    continue;
                };
                let mut words = new.split_whitespace();
                let replacement = match (words.next(), words.next()) {
                    (Some(path), None) if path.starts_with('.') || path.starts_with('/') => {
                        GoReplace::Local(PathBuf::from(path))
                    }
                    (Some(module), Some(version)) => GoReplace::Module {
                        module: module.to_string(),
                        version: version.to_string(),
                    },
                    _ => continue,
                };
                parsed.replaces.insert(old.to_string(), replacement);
            }
            _ => {}
        }
    }
    parsed
}

fn ruby_files(from: &Path, text: &str) -> Vec<PathBuf> {
    ruby_files_in(from, text, &ruby_gem_roots(from))
}

/// Directories whose children are installed gems (`<root>/<gem>-<version>/lib`), nearest
/// first: the project's `vendor/bundle`, then `$GEM_HOME` / `$GEM_PATH`, `~/.gem/ruby/*`,
/// the rbenv and asdf layouts, and the usual Homebrew / system Ruby gem directories.
fn ruby_gem_roots(from: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(directory) = from.parent() {
        for dir in directory.ancestors() {
            for bundle in ["vendor/bundle/ruby", "vendor/ruby"] {
                for version in subdirectories(&dir.join(bundle)) {
                    push_unique(&mut roots, version.join("gems"));
                }
            }
        }
    }
    for variable in ["GEM_HOME", "GEM_PATH"] {
        if let Some(value) = std::env::var_os(variable) {
            for home in std::env::split_paths(&value) {
                push_unique(&mut roots, home.join("gems"));
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for version in subdirectories(&home.join(".gem/ruby")) {
            push_unique(&mut roots, version.join("gems"));
        }
        // A version manager installs a whole Ruby per version, gems two levels below it.
        for manager in [".rbenv/versions", ".asdf/installs/ruby"] {
            for ruby in subdirectories(&home.join(manager)) {
                for version in subdirectories(&ruby.join("lib/ruby/gems")) {
                    push_unique(&mut roots, version.join("gems"));
                }
            }
        }
    }
    for prefix in [
        "/opt/homebrew/lib/ruby/gems",
        "/usr/local/lib/ruby/gems",
        "/usr/lib/ruby/gems",
    ] {
        for version in subdirectories(Path::new(prefix)) {
            push_unique(&mut roots, version.join("gems"));
        }
    }
    roots.retain(|root| root.is_dir());
    roots
}

/// `require 'active_support/core_ext'` →
/// `<root>/activesupport-7.0.4/lib/active_support/core_ext.rb`. Gems the nearest
/// `Gemfile.lock` pins come first, then gems whose name matches the require's first segment,
/// then the rest (`require 'rails'` lives in `railties`).
fn ruby_files_in(from: &Path, text: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if roots.is_empty() {
        return files;
    }
    let locked = gem_lock_versions(from);
    let gems: Vec<Vec<(PathBuf, String)>> = roots.iter().map(|root| gem_dirs(root)).collect();
    for require in crate::imports::ruby_requires(text) {
        let relative = require.trim_end_matches(".rb");
        let Some(first) = relative.split('/').next().filter(|first| !first.is_empty()) else {
            continue;
        };
        for root in &gems {
            let mut candidates: Vec<&(PathBuf, String)> = root.iter().collect();
            candidates.sort_by(|a, b| {
                let rank = |gem: &&(PathBuf, String)| {
                    (!locked_gem(&gem.1, &locked), !gem_matches(&gem.1, first))
                };
                rank(a).cmp(&rank(b)).then_with(|| a.1.cmp(&b.1))
            });
            if let Some(file) = candidates
                .iter()
                .find_map(|(gem, _)| ruby_file(gem, relative))
            {
                push_unique(&mut files, file);
            }
        }
    }
    files
}

/// The file a require path names inside one gem: `lib/<path>.rb`.
fn ruby_file(gem_dir: &Path, relative: &str) -> Option<PathBuf> {
    let file = gem_dir.join("lib").join(format!("{relative}.rb"));
    file.is_file().then_some(file)
}

/// The installed gems of one root, as directory and `name-version`.
fn gem_dirs(root: &Path) -> Vec<(PathBuf, String)> {
    let mut gems: Vec<(PathBuf, String)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (entry.path(), name)
        })
        .collect();
    gems.sort_by(|a, b| a.1.cmp(&b.1));
    gems
}

/// `activesupport-7.0.4` → `activesupport`; a gem name may itself contain `-` (`net-http`),
/// so the version starts at the last `-` followed by a digit.
fn gem_name(directory: &str) -> &str {
    for (index, _) in directory.match_indices('-').rev() {
        if directory[index + 1..].starts_with(|c: char| c.is_ascii_digit()) {
            return &directory[..index];
        }
    }
    directory
}

/// Whether a gem directory is the one the require's first segment names, ignoring `_` and
/// `-`: `active_support` matches `activesupport-7.0.4`.
fn gem_matches(directory: &str, first: &str) -> bool {
    let plain = |value: &str| -> String {
        value
            .chars()
            .filter(|c: &char| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    plain(gem_name(directory)) == plain(first)
}

/// Whether the nearest `Gemfile.lock` pins this gem directory's version; platform-specific
/// builds such as `nokogiri-1.15.4-arm64-darwin` count too.
fn locked_gem(directory: &str, locked: &HashMap<String, String>) -> bool {
    let name = gem_name(directory);
    let Some(version) = locked.get(name) else {
        return false;
    };
    directory
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|rest| rest == version || rest.starts_with(&format!("{version}-")))
}

/// Gem names and versions from the nearest `Gemfile.lock`: under a `specs:` line, specs sit
/// four spaces in and their dependencies six spaces in.
fn gem_lock_versions(from: &Path) -> HashMap<String, String> {
    let mut versions = HashMap::new();
    let Some(directory) = nearest(from.parent(), "Gemfile.lock") else {
        return versions;
    };
    let Ok(text) = std::fs::read_to_string(directory.join("Gemfile.lock")) else {
        return versions;
    };
    let mut specs = false;
    for line in text.lines() {
        if line.trim() == "specs:" {
            specs = true;
            continue;
        }
        if !specs {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with("    ") {
            specs = false;
            continue;
        }
        // A spec's own dependencies sit deeper and are not gems to resolve.
        if line.starts_with("      ") {
            continue;
        }
        let Some((name, version)) = line.trim().rsplit_once(" (") else {
            continue;
        };
        if let Some(version) = version.strip_suffix(')') {
            versions.insert(name.to_string(), version.to_string());
        }
    }
    versions
}

/// Immediate subdirectories of `directory`, sorted for a stable order.
fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// The nearest ancestor of `from` that holds `name`.
fn nearest(from: Option<&Path>, name: &str) -> Option<PathBuf> {
    from?
        .ancestors()
        .find(|dir| dir.join(name).is_file())
        .map(Path::to_path_buf)
}

/// A quoted or bare value from a TOML line: `"1.2.3"` → `1.2.3`.
fn quoted_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or(Some(value).filter(|value| !value.is_empty()))
}

fn script_files(from: &Path, text: &str) -> Vec<PathBuf> {
    let typescript = from
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "ts" | "tsx" | "mts" | "cts"));
    let Some(directory) = from.parent() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for specifier in crate::imports::javascript_specifiers(text) {
        for file in script_candidates(directory, &specifier, typescript) {
            push_unique(&mut files, file);
        }
    }
    files
}

/// The files a `from 'specifier'` can mean, most likely first.
fn script_candidates(directory: &Path, specifier: &str, typescript: bool) -> Vec<PathBuf> {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        return probe(&directory.join(specifier), typescript);
    }
    if specifier.starts_with('/') || specifier.is_empty() {
        return Vec::new();
    }
    let (package, subpath) = package_split(specifier);
    let mut candidates = Vec::new();
    for node_modules in node_modules_dirs(directory) {
        let package_dir = node_modules.join(&package);
        if !subpath.is_empty() {
            for file in probe(&package_dir.join(&subpath), typescript) {
                push_unique(&mut candidates, file);
            }
            continue;
        }
        if typescript {
            if let Some(entry) = package_entry(&package_dir, &["types", "typings"]) {
                for file in probe(&entry, typescript) {
                    push_unique(&mut candidates, file);
                }
            }
            // `@types/react` names `@types/scope__name` for a scoped package.
            let types_name = package.replace('/', "__");
            let types_dir = node_modules.join("@types").join(types_name);
            if let Some(entry) = package_entry(&types_dir, &["types", "typings"]) {
                for file in probe(&entry, typescript) {
                    push_unique(&mut candidates, file);
                }
            }
            for file in probe(&types_dir.join("index.d.ts"), typescript) {
                push_unique(&mut candidates, file);
            }
        }
        if let Some(entry) = package_entry(&package_dir, &["main", "module"]) {
            for file in probe(&entry, typescript) {
                push_unique(&mut candidates, file);
            }
        }
        for file in probe(&package_dir, typescript) {
            push_unique(&mut candidates, file);
        }
    }
    candidates
}

/// The package name and the subpath after it: `react` → (`react`, ""), `@scope/pkg/a` →
/// (`@scope/pkg`, "a").
fn package_split(specifier: &str) -> (String, String) {
    let mut segments = specifier.splitn(3, '/');
    let first = segments.next().unwrap_or_default();
    if first.starts_with('@') {
        let second = segments.next().unwrap_or_default();
        (
            format!("{first}/{second}"),
            segments.next().unwrap_or_default().to_string(),
        )
    } else {
        (
            first.to_string(),
            segments.next().unwrap_or_default().to_string(),
        )
    }
}

/// Every `node_modules` above `directory`, nearest first.
fn node_modules_dirs(directory: &Path) -> Vec<PathBuf> {
    directory
        .ancestors()
        .map(|dir| dir.join("node_modules"))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// A `package.json` field naming the package's entry file.
fn package_entry(package_dir: &Path, keys: &[&str]) -> Option<PathBuf> {
    let text = std::fs::read_to_string(package_dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    keys.iter().find_map(|key| {
        json.get(*key)
            .and_then(serde_json::Value::as_str)
            .map(|entry| package_dir.join(entry))
    })
}

/// The existing files `base` can mean: itself, itself with a script extension, or a directory
/// entry file. TypeScript asks for types before code.
fn probe(base: &Path, typescript: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if base.is_file() {
        found.push(base.to_path_buf());
        return found;
    }
    let extensions: &[&str] = if typescript {
        &["ts", "tsx", "d.ts", "js", "jsx", "mjs", "cjs"]
    } else {
        &["js", "jsx", "mjs", "cjs", "ts", "tsx", "d.ts"]
    };
    for extension in extensions {
        let file = base.with_extension(extension);
        if file.is_file() {
            push_unique(&mut found, file);
        }
    }
    for extension in extensions {
        let file = base.join(format!("index.{extension}"));
        if file.is_file() {
            push_unique(&mut found, file);
        }
    }
    found
}

fn push_unique(files: &mut Vec<PathBuf>, file: PathBuf) {
    if !files.contains(&file) {
        files.push(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("zj-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::canonicalize(root).unwrap()
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_python_import_finds_the_workspace_venv() {
        let root = fixture("python");
        write(&root.join("app.py"), "from peewee import Model\n");
        write(&root.join(".venv/pyvenv.cfg"), "home = /usr/bin\n");
        write(
            &root.join(".venv/lib/python3.12/site-packages/peewee.py"),
            "class Model:\n    pass\n",
        );
        let found = module_files(&root.join("app.py"), "python", "from peewee import Model\n");
        assert_eq!(
            found,
            [root.join(".venv/lib/python3.12/site-packages/peewee.py")]
        );
        // A package resolves to its `__init__.py`.
        write(
            &root.join(".venv/lib/python3.12/site-packages/django/db/__init__.py"),
            "",
        );
        let found = module_files(
            &root.join("app.py"),
            "python",
            "from django.db import models\n",
        );
        assert_eq!(
            found.first().map(PathBuf::as_path),
            Some(
                root.join(".venv/lib/python3.12/site-packages/django/db/__init__.py")
                    .as_path()
            )
        );
        // A venv above the file counts too, and a missing module names nothing.
        write(&root.join("src/deep/app.py"), "from peewee import Model\n");
        let found = module_files(
            &root.join("src/deep/app.py"),
            "python",
            "from peewee import Model\nfrom nowhere import x\n",
        );
        assert_eq!(
            found,
            [root.join(".venv/lib/python3.12/site-packages/peewee.py")]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_rust_import_finds_the_locked_crate() {
        let root = fixture("rust");
        let registry = root.join("registry/src");
        write(
            &root.join("project/Cargo.toml"),
            "[package]\nname = \"app\"\n",
        );
        write(
            &root.join("project/Cargo.lock"),
            "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"foo\"\nversion = \"1.2.3\"\n",
        );
        write(
            &registry.join("foo-1.0.0/src/lib.rs"),
            "pub struct Thing;\n",
        );
        write(
            &registry.join("foo-1.2.3/src/net/client.rs"),
            "pub struct Client;\n",
        );
        write(
            &registry.join("foo-1.2.3/src/lib.rs"),
            "pub struct Thing;\n",
        );
        let main = root.join("project/src/main.rs");
        let found = rust_files_in(
            &main,
            "use foo::net::client::Client;\n",
            std::slice::from_ref(&registry),
        );
        assert_eq!(found, [registry.join("foo-1.2.3/src/net/client.rs")]);
        // A path naming an item of the crate root falls back to `lib.rs`.
        let found = rust_files_in(&main, "use foo::Thing;\n", std::slice::from_ref(&registry));
        assert_eq!(found, [registry.join("foo-1.2.3/src/lib.rs")]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_go_import_finds_the_module_cache_and_replacements() {
        let root = fixture("go");
        let project = root.join("project");
        write(
            &project.join("go.mod"),
            "module example.com/app\n\ngo 1.22\n\nrequire (\n\tgithub.com/Some/Dep v1.2.3\n\tgithub.com/old/dep v0.0.1\n\tgithub.com/local/dep v0.0.1\n)\n\nreplace github.com/old/dep => github.com/new/dep v0.1.0\n\nreplace github.com/local/dep => ../localdep\n",
        );
        let cache = root.join("cache");
        write(
            &cache.join("github.com/!some/!dep@v1.2.3/pkg/model.go"),
            "package pkg\n",
        );
        write(
            &cache.join("github.com/!some/!dep@v1.2.3/pkg/model_test.go"),
            "package pkg\n",
        );
        write(
            &cache.join("github.com/new/dep@v0.1.0/util/x.go"),
            "package util\n",
        );
        write(&root.join("localdep/util/y.go"), "package util\n");
        let main = project.join("main.go");
        let text = "import (\n\t\"github.com/Some/Dep/pkg\"\n\t\"github.com/old/dep/util\"\n\t\"github.com/local/dep/util\"\n)\n";
        let found = go_files_in(&main, text, Some(&cache));
        assert_eq!(
            found,
            [
                cache.join("github.com/!some/!dep@v1.2.3/pkg/model.go"),
                cache.join("github.com/new/dep@v0.1.0/util/x.go"),
                root.join("localdep/util/y.go"),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_ruby_require_finds_the_installed_gem() {
        let root = fixture("ruby");
        let project = root.join("project");
        let gems = project.join("vendor/bundle/ruby/3.2.0/gems");
        write(&project.join("app.rb"), "require 'active_record'\n");
        write(
            &project.join("Gemfile.lock"),
            "GEM\n  remote: https://rubygems.org/\n  specs:\n    activerecord (7.0.4)\n      activesupport (= 7.1.0)\n    activesupport (7.1.0)\n\nPLATFORMS\n  ruby\n",
        );
        write(
            &gems.join("activerecord-7.0.4/lib/active_record.rb"),
            "module ActiveRecord\n  class Base\n  end\nend\n",
        );
        write(
            &gems.join("activesupport-7.0.1/lib/active_support/core_ext.rb"),
            "module ActiveSupport\nend\n",
        );
        write(
            &gems.join("activesupport-7.1.0/lib/active_support/core_ext.rb"),
            "module ActiveSupport\nend\n",
        );
        // `require 'rails'` lives in the railties gem.
        write(
            &gems.join("railties-7.0.4/lib/rails.rb"),
            "module Rails\nend\n",
        );
        let app = project.join("app.rb");
        let found = ruby_files_in(
            &app,
            "require 'active_record'\nrequire 'active_support/core_ext'\nrequire 'rails'\n",
            std::slice::from_ref(&gems),
        );
        assert_eq!(
            found,
            [
                gems.join("activerecord-7.0.4/lib/active_record.rb"),
                // The lock pins 7.1.0, though 7.0.1 sorts first.
                gems.join("activesupport-7.1.0/lib/active_support/core_ext.rb"),
                gems.join("railties-7.0.4/lib/rails.rb"),
            ]
        );
        // require_relative and interpolated requires name no installed gem.
        assert!(
            ruby_files_in(
                &app,
                "require_relative 'helper'\nrequire \"#{name}/x\"\n",
                std::slice::from_ref(&gems),
            )
            .is_empty()
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_script_import_finds_node_modules_and_types() {
        let root = fixture("script");
        write(
            &root.join("src/app.tsx"),
            "import { useState } from \"react\";\n",
        );
        write(
            &root.join("node_modules/react/package.json"),
            "{\"main\": \"index.js\"}\n",
        );
        write(
            &root.join("node_modules/react/index.js"),
            "module.exports = {};\n",
        );
        write(
            &root.join("node_modules/@types/react/index.d.ts"),
            "export declare function useState<S>(value: S): [S, (next: S) => void];\n",
        );
        let found = module_files(
            &root.join("src/app.tsx"),
            "tsx",
            "import { useState } from \"react\";\n",
        );
        assert_eq!(
            found.first(),
            Some(&root.join("node_modules/@types/react/index.d.ts"))
        );

        // A package that ships its own types, and a subpath entry.
        write(
            &root.join("node_modules/zod/package.json"),
            "{\"types\": \"lib/index.d.ts\", \"main\": \"lib/index.js\"}\n",
        );
        write(
            &root.join("node_modules/zod/lib/index.d.ts"),
            "export declare const z: 1;\n",
        );
        let found = module_files(
            &root.join("src/deep/app.ts"),
            "typescript",
            "import { z } from 'zod';\nimport { helper } from './util';\n",
        );
        assert_eq!(
            found.first(),
            Some(&root.join("node_modules/zod/lib/index.d.ts"))
        );
        write(
            &root.join("src/deep/util.ts"),
            "export function helper() {}\n",
        );
        let found = module_files(
            &root.join("src/deep/app.ts"),
            "typescript",
            "import { helper } from './util';\n",
        );
        assert_eq!(found, [root.join("src/deep/util.ts")]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
