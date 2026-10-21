//! What the overview shows about one checkout: git state, languages and version. Collected by running `git`
//! and reading manifests; never writes anything. Slow-ish (a few git calls), so the GUI runs it in the background.

use crate::{codetodo, todo};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    /// False when the folder is gone (moved or deleted); everything else is then empty.
    pub exists: bool,
    pub git: Option<GitInfo>,
    /// Bytes per language, largest first.
    pub languages: Vec<(String, u64)>,
    /// From a manifest, else the latest tag.
    pub version: Option<String>,
    /// The manifest's `description`, else the first prose paragraph of README.md. At most 140 characters.
    pub description: Option<String>,
    /// Open items of the TODO file; `None` without one.
    pub todo_open: Option<u32>,
    /// Code TODO comments. Git repositories only, so a refresh of a plain folder stays cheap.
    pub code_todos: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: Option<String>,
    /// Changed + untracked files.
    pub dirty: u32,
    /// Commits ahead of / behind the upstream branch; `None` without an upstream.
    pub ahead_behind: Option<(u32, u32)>,
    /// ISO date-time of the last commit.
    pub last_commit: Option<String>,
    pub remote: Option<String>,
    /// The first parentless commit: the same in every clone and worktree of a repository (D-12).
    pub root_commit: Option<String>,
    pub latest_tag: Option<String>,
    /// `.git` is a file: a worktree or submodule checkout.
    pub linked: bool,
}

pub fn collect(path: &Path) -> Info {
    if !path.is_dir() {
        return Info::default();
    }
    let git = path.join(".git").exists().then(|| git_info(path));
    let list = files(path);
    let version = manifest_version(path).or_else(|| git.as_ref().and_then(|g| g.latest_tag.clone()));
    let code_todos = if git.is_some() { codetodo::scan_files(path, &list).len() as u32 } else { 0 };
    Info {
        exists: true,
        languages: languages(path, &list),
        version,
        description: description(path),
        todo_open: todo::find_file(path).and_then(|f| todo::load(&f).ok()).map(|(_, d)| d.open_count() as u32),
        code_todos,
        git,
    }
}

/// Files of a checkout, relative to it: `git ls-files` in a git repository, else a walk.
pub fn files(root: &Path) -> Vec<String> {
    if root.join(".git").exists() { git_files(root).unwrap_or_else(|| walk_files(root)) } else { walk_files(root) }
}

/// `git -C <path> <args>`, trimmed stdout; `None` when git fails.
pub fn git(path: &Path, args: &[&str]) -> Option<String> {
    let out = crate::cmd("git").arg("-C").arg(path).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn git_info(path: &Path) -> GitInfo {
    let mut g = GitInfo { linked: path.join(".git").is_file(), ..Default::default() };
    if let Some(status) = git(path, &["status", "--porcelain=v2", "--branch"]) {
        parse_status(&status, &mut g);
    }
    g.last_commit = git(path, &["log", "-1", "--format=%cI"]).filter(|s| !s.is_empty());
    (g.remote, g.root_commit) = identity(path);
    g.latest_tag = git(path, &["describe", "--tags", "--abbrev=0"]).filter(|s| !s.is_empty());
    g
}

/// What makes two folders checkouts of the same project (D-12): the `origin` remote and the root commit.
/// Several roots (merged histories): the lexically first one, so every clone picks the same.
pub fn identity(path: &Path) -> (Option<String>, Option<String>) {
    let remote = git(path, &["remote", "get-url", "origin"]).filter(|s| !s.is_empty());
    let root = git(path, &["rev-list", "--max-parents=0", "HEAD"]).and_then(|s| s.lines().min().map(str::to_string));
    (remote, root)
}

fn parse_status(status: &str, g: &mut GitInfo) {
    for line in status.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            g.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split(' ').map(|n| n.trim_start_matches(['+', '-']).parse().unwrap_or(0));
            g.ahead_behind = Some((it.next().unwrap_or(0), it.next().unwrap_or(0)));
        } else if !line.starts_with('#') && !line.is_empty() {
            g.dirty += 1;
        }
    }
}

/// Tracked files, relative to the checkout.
fn git_files(path: &Path) -> Option<Vec<String>> {
    let out = git(path, &["ls-files", "-z"])?;
    Some(out.split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect())
}

/// Folders a walk never enters (here, in the scanner and for language counts of non-git folders).
pub const SKIP_DIRS: &[&str] = &["node_modules", "target", "venv", "build", "dist", "__pycache__", "obj"];

pub fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

/// Files of a folder that is not a git repository (relative paths), skipping build and dependency folders.
/// ponytail: capped at 20,000 files so a huge plain folder does not stall the refresh.
fn walk_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !skip_dir(&name) {
                    stack.push(e.path());
                }
            } else if let Ok(rel) = e.path().strip_prefix(root) {
                out.push(rel.to_string_lossy().into_owned());
                if out.len() >= 20_000 {
                    return out;
                }
            }
        }
    }
    out
}

/// Programming languages by extension. Data and docs (json, md, csv, txt, toml, ...) are deliberately left out.
pub fn language(file: &str) -> Option<&'static str> {
    let ext = Path::new(file).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "Rust",
        "py" | "pyw" => "Python",
        "js" | "mjs" | "cjs" | "jsx" => "JavaScript",
        "ts" | "mts" | "cts" | "tsx" => "TypeScript",
        "svelte" => "Svelte",
        "vue" => "Vue",
        "html" | "htm" => "HTML",
        "css" | "scss" | "sass" | "less" => "CSS",
        "go" => "Go",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "C++",
        "cs" => "C#",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "lua" => "Lua",
        "sh" | "bash" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "sql" => "SQL",
        "gs" => "Apps Script",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "dart" => "Dart",
        "zig" => "Zig",
        _ => return None,
    })
}

fn languages(root: &Path, files: &[String]) -> Vec<(String, u64)> {
    let mut by: std::collections::HashMap<&'static str, u64> = Default::default();
    for f in files {
        if let Some(lang) = language(f) {
            let size = std::fs::metadata(root.join(f)).map_or(0, |m| m.len());
            *by.entry(lang).or_default() += size;
        }
    }
    let mut v: Vec<(String, u64)> = by.into_iter().filter(|(_, n)| *n > 0).map(|(l, n)| (l.to_string(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Version from `Cargo.toml` (workspace package, then package), `package.json` or `pyproject.toml`.
pub fn manifest_version(root: &Path) -> Option<String> {
    let read = |f: &str| std::fs::read_to_string(root.join(f)).ok();
    if let Some(t) = read("Cargo.toml").and_then(|s| s.parse::<toml::Table>().ok()) {
        let ws = t.get("workspace").and_then(|w| w.get("package")).and_then(|p| p.get("version")).and_then(|v| v.as_str());
        let pkg = t.get("package").and_then(|p| p.get("version")).and_then(|v| v.as_str());
        if let Some(v) = pkg.or(ws) {
            return Some(v.to_string());
        }
    }
    if let Some(j) = read("package.json").and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        && let Some(v) = j.get("version").and_then(|v| v.as_str())
    {
        return Some(v.to_string());
    }
    if let Some(t) = read("pyproject.toml").and_then(|s| s.parse::<toml::Table>().ok()) {
        let project = t.get("project").and_then(|p| p.get("version"));
        let poetry = t.get("tool").and_then(|p| p.get("poetry")).and_then(|p| p.get("version"));
        if let Some(v) = project.or(poetry).and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    None
}

/// The manifest's description (`Cargo.toml`, `package.json`, `pyproject.toml`), else the README summary.
fn description(root: &Path) -> Option<String> {
    manifest_description(root).or_else(|| readme_summary(root)).map(|d| cut(&d, 140))
}

fn manifest_description(root: &Path) -> Option<String> {
    let read = |f: &str| std::fs::read_to_string(root.join(f)).ok();
    if let Some(t) = read("Cargo.toml").and_then(|s| s.parse::<toml::Table>().ok()) {
        let ws = t.get("workspace").and_then(|w| w.get("package")).and_then(|p| p.get("description")).and_then(|v| v.as_str());
        let pkg = t.get("package").and_then(|p| p.get("description")).and_then(|v| v.as_str());
        if let Some(d) = pkg.or(ws).map(str::trim).filter(|d| !d.is_empty()) {
            return Some(d.to_string());
        }
    }
    if let Some(d) = read("package.json")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|j| j.get("description").and_then(|v| v.as_str()).map(str::trim).filter(|d| !d.is_empty()).map(str::to_string))
    {
        return Some(d);
    }
    let t = read("pyproject.toml")?.parse::<toml::Table>().ok()?;
    t.get("project")?.get("description")?.as_str().map(str::trim).filter(|d| !d.is_empty()).map(str::to_string)
}

/// The first prose paragraph of README.md: headings, badges, images, HTML, tables and lists are skipped.
fn readme_summary(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("README.md")).ok()?;
    let mut para: Vec<&str> = Vec::new();
    let mut fence = false;
    let lines: Vec<&str> = text.lines().collect();
    // Setext headings: a line underlined with `===` or `---` (the underline itself is skipped below).
    let underline = |l: &str| {
        let l = l.trim();
        l.len() >= 2 && (l.chars().all(|c| c == '=') || l.chars().all(|c| c == '-'))
    };
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if underline(t) || lines.get(i + 1).is_some_and(|n| underline(n)) {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            if !para.is_empty() {
                break;
            }
            continue;
        }
        let skip = fence || t.starts_with(['#', '<', '|', '-', '*', '>']) || t.starts_with(|c: char| c.is_ascii_digit()) || t.contains("![");
        if t.is_empty() || skip {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        para.push(t);
    }
    let s = plain(&para.join(" "));
    (!s.is_empty()).then_some(s)
}

/// Markdown to text: `[label](url)` becomes its label, emphasis and code marks go.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) if after[close + 1..].starts_with('(') => {
                out.push_str(&after[..close]);
                rest = after[close..].find(')').map_or("", |p| &after[close + p + 1..]);
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.retain(|c| c != '*' && c != '`');
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, cut at a word boundary with "…".
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max - 1).collect();
    match head.rfind(' ') {
        Some(i) => format!("{}…", &head[..i]),
        None => format!("{head}…"),
    }
}

/// `https://github.com/<owner>/<repo>` for a GitHub remote (https, ssh or scp-style), else `None`.
pub fn github_url(remote: &str) -> Option<String> {
    let r = remote.trim().trim_end_matches('/');
    let r = r.strip_suffix(".git").unwrap_or(r);
    let rest = ["https://github.com/", "http://github.com/", "ssh://git@github.com/", "git@github.com:", "git://github.com/"]
        .iter()
        .find_map(|p| r.strip_prefix(p))?;
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (!owner.is_empty() && !repo.is_empty() && parts.next().is_none()).then(|| format!("https://github.com/{owner}/{repo}"))
}

/// Build and environment folders the disk widget shows apart from the rest.
pub const BULKY: [&str; 3] = ["target", "node_modules", ".venv"];

/// Bytes on disk under `root` (symlinks not followed): the total, and per name in [`BULKY`] the folders of that
/// name anywhere in the tree (a nested one counted once, with its outermost). Walks everything: call it off the UI
/// thread.
pub fn disk_usage(root: &Path) -> (u64, [u64; 3]) {
    fn walk(dir: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        rd.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
                _ => 0,
            })
            .sum()
    }
    fn split(dir: &Path, bulky: &mut [u64; 3]) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        let mut rest = 0;
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => match BULKY.iter().position(|b| e.file_name() == *b) {
                    Some(k) => bulky[k] += walk(&e.path()),
                    None => rest += split(&e.path(), bulky),
                },
                Ok(t) if t.is_file() => rest += e.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
        rest
    }
    let mut bulky = [0; 3];
    let rest = split(root, &mut bulky);
    (rest + bulky.iter().sum::<u64>(), bulky)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_usage_splits_out_build_folders() {
        let dir = std::env::temp_dir().join(format!("pa-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, n) in [("src/a.rs", 10), ("target/debug/x", 100), ("web/node_modules/p/i.js", 30), ("web/node_modules/p/node_modules/q.js", 5), (".venv/l", 7)] {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; n]).unwrap();
        }
        assert_eq!(disk_usage(&dir), (152, [100, 35, 7]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn github_urls() {
        for r in ["https://github.com/example/sudoku.git", "git@github.com:example/sudoku.git", "ssh://git@github.com/example/sudoku", "https://github.com/example/sudoku/"] {
            assert_eq!(github_url(r).as_deref(), Some("https://github.com/example/sudoku"), "{r}");
        }
        assert_eq!(github_url("https://gitlab.com/a/b.git"), None);
        assert_eq!(github_url("https://github.com/a"), None);
    }

    #[test]
    fn descriptions_and_todo_counts() {
        let dir = std::env::temp_dir().join(format!("pa-info-desc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("README.md"),
            "# Title\n\n[![CI](x.svg)](y)\n\n<p align=center>hi</p>\n\nA [fast](https://x.io) **tool** for `things`.\nSecond line.\n\nMore.\n",
        )
        .unwrap();
        assert_eq!(description(&dir).as_deref(), Some("A fast tool for things. Second line."));
        std::fs::write(dir.join("README.md"), "grove.py
========

Grove sensors for Python.
").unwrap();
        assert_eq!(description(&dir).as_deref(), Some("Grove sensors for Python."), "setext heading skipped");
        std::fs::write(
            dir.join("README.md"),
            "# Title

[![CI](x.svg)](y)

<p align=center>hi</p>

A [fast](https://x.io) **tool** for `things`.
Second line.

More.
",
        )
        .unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\ndescription = \"From the manifest\"\n").unwrap();
        assert_eq!(description(&dir).as_deref(), Some("From the manifest"));
        std::fs::write(dir.join("README.md"), "word ".repeat(60)).unwrap();
        let _ = std::fs::remove_file(dir.join("Cargo.toml"));
        let long = description(&dir).unwrap();
        assert!(long.chars().count() <= 140 && long.ends_with('…') && long.starts_with("word word"), "{long}");

        std::fs::write(dir.join("TODO.md"), "## A\n- [ ] one\n- [x] two\n- [ ] three\n").unwrap();
        let info = collect(&dir);
        assert_eq!(info.todo_open, Some(2));
        assert_eq!(info.code_todos, 0, "code TODOs are counted in git repositories only");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_parsing() {
        let mut g = GitInfo::default();
        parse_status("# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -5\n1 .M N... a\n? new.txt\n", &mut g);
        assert_eq!((g.branch.as_deref(), g.ahead_behind, g.dirty), (Some("main"), Some((2, 5)), 2));
        let mut g = GitInfo::default();
        parse_status("# branch.head (detached)\n", &mut g);
        assert_eq!((g.branch, g.ahead_behind), (None, None));
    }

    #[test]
    fn versions_and_languages() {
        let dir = std::env::temp_dir().join(format!("pa-info-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[workspace.package]\nversion = \"0.7.1\"\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("src/app.py"), "x").unwrap();
        std::fs::write(dir.join("node_modules/big.js"), "x".repeat(1000)).unwrap();
        std::fs::write(dir.join("data.json"), "{}").unwrap();
        let info = collect(&dir);
        assert_eq!(info.version.as_deref(), Some("0.7.1"));
        assert_eq!(info.languages, [("Rust".to_string(), 13), ("Python".to_string(), 1)]);
        assert!(info.git.is_none() && info.exists);

        std::fs::write(dir.join("Cargo.toml"), "[package]\nversion.workspace = true\n").unwrap();
        assert_eq!(manifest_version(&dir), None);
        std::fs::remove_file(dir.join("Cargo.toml")).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"name":"x","version":"2.0.0"}"#).unwrap();
        assert_eq!(manifest_version(&dir).as_deref(), Some("2.0.0"));
        std::fs::remove_file(dir.join("package.json")).unwrap();
        std::fs::write(dir.join("pyproject.toml"), "[project]\nversion = \"0.3\"\n").unwrap();
        assert_eq!(manifest_version(&dir).as_deref(), Some("0.3"));
        assert!(!collect(&dir.join("missing")).exists);
        let _ = std::fs::remove_dir_all(dir);
    }
}
