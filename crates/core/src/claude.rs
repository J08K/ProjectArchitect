//! Claude Code configuration (SPEC § 8, D-3, D-17): what a project or the global `~/.claude` has (instructions,
//! settings, agents, skills, commands, MCP and launch files), whole and part copies between them, and edits.
//! Every write is a [`Plan`] first (the file changes, for a diff preview), then [`apply`]: a backup of each file it
//! replaces (`todo::backup`), then an atomic write after a re-read check (`todo::save`). Copies are plain copies with
//! no link to their source (D-17). Other agents' files are listed, never written.
//!
//! Items are named by their path in a project (`CLAUDE.md`, `.claude/agents/reviewer.md`, `.claude/skills/pdf`). The
//! global scope maps both `x` and `.claude/x` to `~/.claude/x`, so one name works in both scopes.

use crate::db::Store;
use crate::info::skip_dir;
use crate::ops::{self, Error, Result};
use crate::todo;
use serde_json::{Map, Value};
use std::io;
use std::path::{Path, PathBuf};

/// Store setting that lets the MCP server write Claude config files (copy and edit). Off unless it is `"true"`.
pub const ALLOW_WRITE: &str = "mcp.allow_claude_write";
/// Store setting: the folder that holds `.claude` for the global scope. Default: the user's home folder.
pub const HOME: &str = "claude.home";

/// Other agents' files: listed, never written (v1).
const OTHER: &[&str] = &["opencode.json", "GEMINI.md", ".cursorrules", ".windsurfrules", ".github/copilot-instructions.md"];
/// How deep the walk for nested `CLAUDE.md` files goes.
const NESTED_DEPTH: usize = 6;

fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Instructions,
    Settings,
    Agent,
    Skill,
    Command,
    Mcp,
    Launch,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Instructions, Kind::Settings, Kind::Agent, Kind::Skill, Kind::Command, Kind::Mcp, Kind::Launch, Kind::Other];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Instructions => "Instructions",
            Kind::Settings => "Settings",
            Kind::Agent => "Agents",
            Kind::Skill => "Skills",
            Kind::Command => "Commands",
            Kind::Mcp => "MCP",
            Kind::Launch => "Launch",
            Kind::Other => "Other agents",
        }
    }
}

/// One config file or skill folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// Path as in a project, forward slashes (see the module docs).
    pub rel: String,
    /// `CLAUDE.md`, `settings.json`, `reviewer` (an agent), `pdf` (a skill).
    pub name: String,
    /// A skill: a folder of files.
    pub dir: bool,
    /// Another agent's file.
    pub read_only: bool,
}

impl Item {
    fn new(rel: &str, dir: bool) -> Option<Item> {
        let (kind, read_only) = classify(rel)?;
        let file = rel.rsplit('/').next().unwrap_or(rel);
        let name = match kind {
            Kind::Agent | Kind::Command => {
                let rest = rel.splitn(3, '/').nth(2).unwrap_or(file);
                rest.strip_suffix(".md").unwrap_or(rest).to_string()
            }
            Kind::Skill => rel.split('/').nth(2).unwrap_or(file).to_string(),
            Kind::Settings | Kind::Launch | Kind::Mcp => file.to_string(),
            Kind::Instructions | Kind::Other => rel.to_string(),
        };
        Some(Item { kind, rel: rel.to_string(), name, dir, read_only })
    }

    /// The column of the cross-project matrix: nested `CLAUDE.md` files share one.
    pub fn column(&self) -> String {
        match self.kind {
            Kind::Instructions if self.rel.contains('/') => "nested CLAUDE.md".into(),
            Kind::Agent => format!("agent: {}", self.name),
            Kind::Skill => format!("skill: {}", self.name),
            Kind::Command => format!("command: {}", self.name),
            _ => self.name.clone(),
        }
    }

    pub fn is_markdown(&self) -> bool {
        !self.dir && self.rel.ends_with(".md")
    }

    pub fn is_json(&self) -> bool {
        self.rel.ends_with(".json")
    }
}

/// What a path is, and whether it is read-only: `None` for anything that is not Claude (or other agents') config.
/// Paths with `..`, empty segments or a drive are refused here, so a name can never leave its scope.
pub fn classify(rel: &str) -> Option<(Kind, bool)> {
    let rel = rel.trim_start_matches("./");
    if rel.is_empty() || rel.contains(['\\', ':']) || rel.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return None;
    }
    if let Some(rest) = rel.strip_prefix(".claude/") {
        return match rest.split_once('/') {
            None => match rest {
                "settings.json" | "settings.local.json" => Some((Kind::Settings, false)),
                "launch.json" => Some((Kind::Launch, false)),
                _ => None,
            },
            Some(("agents", f)) if f.ends_with(".md") => Some((Kind::Agent, false)),
            Some(("commands", f)) if f.ends_with(".md") => Some((Kind::Command, false)),
            Some(("skills", _)) => Some((Kind::Skill, false)),
            _ => None,
        };
    }
    let file = rel.rsplit('/').next()?;
    match rel {
        ".mcp.json" => Some((Kind::Mcp, false)),
        "CLAUDE.md" | "CLAUDE.local.md" | "AGENTS.md" => Some((Kind::Instructions, false)),
        _ if OTHER.contains(&rel) => Some((Kind::Other, true)),
        // Nested instructions, outside hidden and build folders.
        _ if matches!(file, "CLAUDE.md" | "CLAUDE.local.md") && !rel.split('/').rev().skip(1).any(skip_dir) => Some((Kind::Instructions, false)),
        _ => None,
    }
}

/// `\` as `/`, a leading `./` dropped, and checked with [`classify`].
pub fn norm_rel(rel: &str) -> Result<String> {
    let rel = rel.trim().replace('\\', "/");
    let rel = rel.trim_start_matches("./").trim_end_matches('/').to_string();
    match classify(&rel) {
        Some(_) => Ok(rel),
        None => fail(format!(
            "{rel:?} is not a Claude config file (CLAUDE.md, AGENTS.md, .claude/settings.json, .claude/agents/*.md, .claude/skills/<name>, .mcp.json, …)"
        )),
    }
}

// --- scopes ---------------------------------------------------------------------------------------------------

/// Where items live: a project checkout, or the global `~/.claude`.
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub root: PathBuf,
    pub global: bool,
}

impl Scope {
    pub fn project(root: &Path) -> Scope {
        Scope { root: root.to_path_buf(), global: false }
    }

    /// `<home>/.claude`.
    pub fn global(home: &Path) -> Scope {
        Scope { root: home.join(".claude"), global: true }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        let rel = if self.global { rel.strip_prefix(".claude/").unwrap_or(rel) } else { rel };
        rel.split('/').fold(self.root.clone(), |p, s| p.join(s))
    }

    /// Whether `rel` means something in this scope: the global one has `CLAUDE.md`, `settings.json`, agents, skills
    /// and commands only (its MCP servers live in `~/.claude.json`, which the app does not touch).
    pub fn fits(&self, rel: &str) -> bool {
        !self.global
            || rel == "CLAUDE.md"
            || rel == ".claude/settings.json"
            || classify(rel).is_some_and(|(k, _)| matches!(k, Kind::Agent | Kind::Skill | Kind::Command))
    }
}

/// The folder holding the global `.claude`: the `claude.home` setting, else the user's home.
pub fn home(store: &Store) -> Result<PathBuf> {
    if let Some(h) = store.setting(HOME)?.filter(|h| !h.trim().is_empty()) {
        return Ok(PathBuf::from(h));
    }
    match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Some(h) => Ok(PathBuf::from(h)),
        None => fail("no home folder (USERPROFILE / HOME is not set)"),
    }
}

/// The name Claude Code gives a folder's transcripts folder under `~/.claude/projects`: every character but ASCII
/// letters and digits as '-'.
fn sessions_name(folder: &str) -> String {
    folder.trim_end_matches(['\\', '/']).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The transcripts (`*.jsonl`, one per session) in a folder with their last-write times (seconds since the epoch).
fn transcripts(dir: &Path) -> Vec<(PathBuf, i64)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)))
        .collect()
}

/// When a Claude Code session in `folder` last wrote its transcript, in seconds since the epoch.
pub fn last_session(home: &Path, folder: &str) -> Option<i64> {
    transcripts(&home.join(".claude").join("projects").join(sessions_name(folder))).into_iter().map(|t| t.1).max()
}

/// The transcripts of every Claude Code session in `folder` and in the worktrees Claude Code made under it
/// (`<folder>/.claude/worktrees/*`).
fn folder_transcripts(home: &Path, folder: &str) -> Vec<(PathBuf, i64)> {
    let name = sessions_name(folder);
    let worktrees = format!("{name}--claude-worktrees-");
    let Ok(rd) = std::fs::read_dir(home.join(".claude").join("projects")) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n == name || n.starts_with(&worktrees)))
        .flat_map(|e| transcripts(&e.path()))
        .collect()
}

/// Last-write times of every Claude Code session in `folder` and its Claude worktrees.
pub fn session_times(home: &Path, folder: &str) -> Vec<i64> {
    folder_transcripts(home, folder).into_iter().map(|t| t.1).collect()
}

/// A Claude Code session as the dashboard lists it (read-only).
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// Last write, seconds since the epoch.
    pub secs: i64,
    /// The first line of the first prompt typed (empty if none was found).
    pub prompt: String,
}

/// The `n` most recent sessions in these folders and their Claude worktrees, newest first.
pub fn recent_sessions(home: &Path, folders: &[String], n: usize) -> Vec<Session> {
    let mut all: Vec<(PathBuf, i64)> = folders.iter().flat_map(|f| folder_transcripts(home, f)).collect();
    all.sort_by_key(|t| std::cmp::Reverse(t.1));
    all.truncate(n);
    all.into_iter().map(|(path, secs)| Session { secs, prompt: first_prompt(&path).unwrap_or_default() }).collect()
}

/// The first prompt typed in a transcript: a `user` entry with text content that is not a tool result, a meta
/// entry or a `<command-…>` / reminder wrapper. Reads lines only until it finds one.
fn first_prompt(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let file = io::BufReader::new(std::fs::File::open(path).ok()?);
    for line in file.lines().map_while(io::Result::ok).take(500) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["type"] != "user" || v["isMeta"] == true {
            continue;
        }
        let parts: Vec<&str> = match &v["message"]["content"] {
            Value::String(s) => vec![s.as_str()],
            Value::Array(parts) => parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect(),
            _ => continue,
        };
        if let Some(text) = parts.into_iter().map(strip_wrappers).find(|t| !t.is_empty()) {
            return Some(text.lines().next().unwrap_or_default().chars().take(200).collect());
        }
    }
    None
}

/// `text` without the `<tag>…</tag>` blocks it starts with (system reminders, slash-command wrappers).
fn strip_wrappers(text: &str) -> &str {
    let mut t = text.trim();
    while let Some(rest) = t.strip_prefix('<') {
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        let Some(end) = (!name.is_empty()).then(|| t.find(&format!("</{name}>"))).flatten() else { return "" };
        t = t[end + name.len() + 3..].trim();
    }
    t
}

/// `"global"` (or `"~"`) for `~/.claude`, else a project (id, name or path) at its primary checkout.
pub fn scope_of(store: &Store, key: &str) -> Result<Scope> {
    let key = key.trim();
    if key.eq_ignore_ascii_case("global") || key == "~" {
        return Ok(Scope::global(&home(store)?));
    }
    Ok(Scope::project(&ops::primary_root(&ops::find_project(store, key)?)?))
}

/// Whether the MCP server may write Claude config (the `mcp.allow_claude_write` setting).
pub fn write_allowed(store: &Store) -> Result<bool> {
    Ok(store.setting(ALLOW_WRITE)?.as_deref() == Some("true"))
}

// --- inventory ------------------------------------------------------------------------------------------------

/// The items of a scope, in kind order.
pub fn inventory(scope: &Scope) -> Vec<Item> {
    let mut out = Vec::new();
    let top: &[&str] = if scope.global {
        &["CLAUDE.md", ".claude/settings.json"]
    } else {
        &["CLAUDE.md", "CLAUDE.local.md", "AGENTS.md", ".claude/settings.json", ".claude/settings.local.json", ".mcp.json", ".claude/launch.json"]
    };
    out.extend(top.iter().filter(|r| scope.path(r).is_file()).filter_map(|r| Item::new(r, false)));
    for sub in ["agents", "commands"] {
        let base = format!(".claude/{sub}");
        let mut files = Vec::new();
        walk(&scope.path(&base), "", 3, &mut files);
        out.extend(files.iter().filter(|f| f.ends_with(".md")).filter_map(|f| Item::new(&format!("{base}/{f}"), false)));
    }
    if let Ok(rd) = std::fs::read_dir(scope.path(".claude/skills")) {
        let mut skills: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        skills.sort();
        out.extend(skills.iter().filter_map(|s| Item::new(&format!(".claude/skills/{s}"), true)));
    }
    if !scope.global {
        let mut nested = Vec::new();
        nested_instructions(&scope.root, "", 0, &mut nested);
        out.extend(nested.iter().filter_map(|r| Item::new(r, false)));
        out.extend(OTHER.iter().filter(|r| scope.path(r).is_file()).filter_map(|r| Item::new(r, false)));
    }
    out.sort_by(|a, b| (a.kind, &a.rel).cmp(&(b.kind, &b.rel)));
    out
}

/// Files under `dir`, relative, `/`-separated, sorted; `depth` levels of folders.
fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => walk(&e.path(), &rel, depth - 1, out),
            Ok(t) if t.is_file() => out.push(rel),
            _ => {}
        }
    }
}

fn nested_instructions(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.file_type().is_ok_and(|t| t.is_dir()) || skip_dir(&name) {
            continue;
        }
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        for f in ["CLAUDE.md", "CLAUDE.local.md"] {
            if e.path().join(f).is_file() {
                out.push(format!("{rel}/{f}"));
            }
        }
        if depth + 1 < NESTED_DEPTH {
            nested_instructions(&e.path(), &rel, depth + 1, out);
        }
    }
    out.sort();
}

/// The files of an item, relative to the scope like `rel`: the item itself, or every file of a skill folder.
pub fn files(scope: &Scope, rel: &str) -> Vec<String> {
    let path = scope.path(rel);
    if !path.is_dir() {
        return vec![rel.to_string()];
    }
    let mut out = Vec::new();
    walk(&path, "", 8, &mut out);
    out.into_iter().map(|f| format!("{rel}/{f}")).collect()
}

// --- plans ----------------------------------------------------------------------------------------------------

/// What of a file to copy.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Whole,
    /// A markdown section: its heading's text (without `#`), the heading line and the body up to the next heading of
    /// the same or a higher level.
    Section(String),
    /// A JSON value by its JSON Pointer (`/permissions/allow/2`, `/mcpServers/name`, `/env/KEY`; `""` = all of it).
    Key(String),
}

/// One file write. `before` is the file as read (empty when it did not exist); the write is refused when the file
/// no longer reads that way.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: PathBuf,
    pub existed: bool,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

impl Change {
    pub fn is_text(&self) -> bool {
        std::str::from_utf8(&self.before).is_ok() && std::str::from_utf8(&self.after).is_ok()
    }

    /// A unified diff of the change (3 lines of context), or a note for binary files.
    pub fn diff(&self) -> String {
        if !self.is_text() {
            return format!("(binary file, {} → {} bytes)", self.before.len(), self.after.len());
        }
        unified(&String::from_utf8_lossy(&self.before), &String::from_utf8_lossy(&self.after))
    }
}

/// The writes of a copy or an edit, for a preview before [`apply`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub changes: Vec<Change>,
    /// Where the target has something else (a file, a section heading or a JSON value): left alone, or replaced when
    /// planned with `overwrite`.
    pub conflicts: Vec<String>,
}

fn read_opt(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => fail(format!("{}: {e}", path.display())),
    }
}

fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {e}", path.display())))
}

fn writable(scope: &Scope, rel: &str) -> Result<()> {
    if classify(rel).is_some_and(|(_, ro)| ro) {
        return fail(format!("{rel} belongs to another agent and is read-only here"));
    }
    if !scope.fits(rel) {
        return fail(format!("{rel} has no place in the global ~/.claude (only CLAUDE.md, settings.json, agents, skills and commands)"));
    }
    Ok(())
}

/// Add a write of `after` to `path`, unless the file already reads so; an existing different file is a conflict,
/// replaced only with `overwrite`.
fn put(plan: &mut Plan, path: PathBuf, label: &str, after: Vec<u8>, overwrite: bool) -> Result<()> {
    let before = read_opt(&path)?;
    if before.as_ref().is_some_and(|b| *b != after) {
        plan.conflicts.push(format!("{label} exists and differs"));
    }
    match before {
        Some(b) if b == after => {}
        Some(_) if !overwrite => {}
        before => plan.changes.push(Change { path, existed: before.is_some(), before: before.unwrap_or_default(), after }),
    }
    Ok(())
}

/// Plan copying `rel` (or a part of it) from one scope to another, as `to_rel` (default: the same name). A whole
/// file or skill folder replaces the target's only with `overwrite`; a markdown section is appended, or replaces the
/// section with the same heading with `overwrite`; a JSON part is merged: objects by key, arrays by adding missing
/// elements, different values kept unless `overwrite`.
pub fn plan_copy(from: &Scope, rel: &str, part: &Part, to: &Scope, to_rel: Option<&str>, overwrite: bool) -> Result<Plan> {
    let rel = norm_rel(rel)?;
    let to_rel = match to_rel {
        Some(r) => norm_rel(r)?,
        None => rel.clone(),
    };
    if from.path(&rel) == to.path(&to_rel) {
        return fail("the source and the target are the same file");
    }
    writable(to, &to_rel)?;
    let src = from.path(&rel);
    if !src.exists() {
        return fail(format!("{} does not exist", src.display()));
    }
    let mut plan = Plan::default();
    match part {
        Part::Whole if src.is_dir() => {
            for f in files(from, &rel) {
                let sub = &f[rel.len()..];
                let target = format!("{to_rel}{sub}");
                put(&mut plan, to.path(&target), &target, read_opt(&from.path(&f))?.unwrap_or_default(), overwrite)?;
            }
        }
        Part::Whole => put(&mut plan, to.path(&to_rel), &to_rel, read_opt(&src)?.unwrap_or_default(), overwrite)?,
        Part::Section(title) => {
            if !(rel.ends_with(".md") && to_rel.ends_with(".md")) {
                return fail("sections are copied between markdown files");
            }
            let text = read_text(&src)?;
            let Some(s) = md_sections(&text).into_iter().find(|s| s.title == title.trim()) else {
                return fail(format!("{rel} has no section {title:?}"));
            };
            let path = to.path(&to_rel);
            let before = read_opt(&path)?;
            let dest = String::from_utf8_lossy(before.as_deref().unwrap_or_default()).into_owned();
            let block = section_text(&text, &s);
            if let Some(after) = insert_section(&dest, &s, &block, overwrite, &mut plan.conflicts, &to_rel) {
                plan.changes.push(Change { path, existed: before.is_some(), before: before.unwrap_or_default(), after: after.into_bytes() });
            }
        }
        Part::Key(pointer) => {
            if !(rel.ends_with(".json") && to_rel.ends_with(".json")) {
                return fail("keys are copied between JSON files");
            }
            let source: Value = parse_json(&read_text(&src)?, &rel)?;
            let Some(frag) = fragment(&source, pointer) else {
                return fail(format!("{rel} has no key {pointer:?} (a JSON Pointer such as /permissions/allow/0)"));
            };
            let path = to.path(&to_rel);
            let before = read_opt(&path)?;
            let mut dest = match &before {
                Some(b) => parse_json(&String::from_utf8_lossy(b), &to_rel)?,
                None => Value::Object(Map::new()),
            };
            let was = dest.clone();
            let mut conflicts = Vec::new();
            merge(&mut dest, &frag, "", overwrite, &mut conflicts);
            plan.conflicts.extend(conflicts.into_iter().map(|c| format!("{to_rel}: {c} differs")));
            if dest != was || before.is_none() {
                plan.changes.push(Change { path, existed: before.is_some(), before: before.unwrap_or_default(), after: pretty(&dest).into_bytes() });
            }
        }
    }
    Ok(plan)
}

/// Plan replacing `rel` with `text`. `read` is the text the edit started from (`None`: the file as it is now); the
/// write is refused if the file changed since. JSON files must parse.
pub fn plan_edit(scope: &Scope, rel: &str, read: Option<&str>, text: &str) -> Result<Plan> {
    let rel = norm_rel(rel)?;
    writable(scope, &rel)?;
    if rel.ends_with(".json") {
        parse_json(text, &rel)?;
    }
    let path = scope.path(&rel);
    if path.is_dir() {
        return fail(format!("{rel} is a folder; edit one of its files"));
    }
    let now = read_opt(&path)?;
    let before = match read {
        Some(r) => r.as_bytes().to_vec(),
        None => now.clone().unwrap_or_default(),
    };
    let mut plan = Plan::default();
    if before != text.as_bytes() || now.is_none() {
        plan.changes.push(Change { path, existed: now.is_some(), before, after: text.as_bytes().to_vec() });
    }
    Ok(plan)
}

/// Write a plan: first a backup of every file it replaces, then each file atomically after its re-read check.
/// Returns the backups.
// ponytail: a failure part way through a skill copy leaves the files written so far; each one is whole.
pub fn apply(data_dir: &Path, plan: &Plan) -> Result<Vec<PathBuf>> {
    let mut backups = Vec::new();
    for c in &plan.changes {
        if c.path.is_file() {
            backups.push(todo::backup(data_dir, &c.path)?);
        }
    }
    for c in &plan.changes {
        if let Some(dir) = c.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        match (std::str::from_utf8(&c.before), std::str::from_utf8(&c.after)) {
            (Ok(before), Ok(after)) => todo::save(&c.path, before, after)?,
            _ => {
                if read_opt(&c.path)?.unwrap_or_default() != c.before {
                    return fail(format!("{} changed outside the app; reload it", c.path.display()));
                }
                let tmp = PathBuf::from(format!("{}.tmp", c.path.display()));
                std::fs::write(&tmp, &c.after)?;
                std::fs::rename(&tmp, &c.path)?;
            }
        }
    }
    Ok(backups)
}

/// The text of a part of a file: all of it, one markdown section, or one JSON value (pretty-printed).
pub fn read_part(text: &str, part: &Part) -> Result<String> {
    match part {
        Part::Whole => Ok(text.to_string()),
        Part::Section(title) => match md_sections(text).into_iter().find(|s| s.title == title.trim()) {
            Some(s) => Ok(section_text(text, &s)),
            None => fail(format!("no section {title:?}")),
        },
        Part::Key(pointer) => match parse_json(text, "the file")?.pointer(pointer) {
            Some(v) => Ok(serde_json::to_string_pretty(v).unwrap_or_default()),
            None => fail(format!("no key {pointer:?}")),
        },
    }
}

// --- markdown sections ----------------------------------------------------------------------------------------

/// A `#` heading and its body: lines `start..end` (0-based, heading included).
#[derive(Clone, Debug, PartialEq)]
pub struct MdSection {
    pub title: String,
    pub level: usize,
    pub start: usize,
    pub end: usize,
}

/// The headings of a markdown text (not those inside code fences), each with its extent.
pub fn md_sections(text: &str) -> Vec<MdSection> {
    let lines: Vec<&str> = text.lines().collect();
    let mut heads: Vec<(usize, usize, String)> = Vec::new();
    let mut fence = false;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        let level = t.chars().take_while(|c| *c == '#').count();
        if !fence && (1..=6).contains(&level) && t[level..].starts_with(' ') {
            heads.push((i, level, t[level..].trim().trim_end_matches('#').trim().to_string()));
        }
    }
    heads
        .iter()
        .enumerate()
        .map(|(k, (start, level, title))| {
            let end = heads[k + 1..].iter().find(|h| h.1 <= *level).map_or(lines.len(), |h| h.0);
            MdSection { title: title.clone(), level: *level, start: *start, end }
        })
        .collect()
}

/// A section's lines, trailing blank lines dropped, ending in one newline.
fn section_text(text: &str, s: &MdSection) -> String {
    let lines: Vec<&str> = text.lines().collect();
    format!("{}\n", lines[s.start..s.end].join("\n").trim_end())
}

/// `dest` with `block` (the section `s`) appended, or replacing the section of the same heading when `overwrite`.
/// `None` when nothing changes.
fn insert_section(dest: &str, s: &MdSection, block: &str, overwrite: bool, conflicts: &mut Vec<String>, label: &str) -> Option<String> {
    let crlf = dest.contains("\r\n");
    let lines: Vec<&str> = dest.lines().collect();
    let out = match md_sections(dest).into_iter().find(|d| d.title == s.title && d.level == s.level) {
        Some(d) if section_text(dest, &d) == block => return None,
        Some(_) if !overwrite => {
            conflicts.push(format!("{label}: section {:?} exists and differs", s.title));
            return None;
        }
        Some(d) => {
            conflicts.push(format!("{label}: section {:?} exists and differs", s.title));
            let mut v: Vec<String> = lines[..d.start].iter().map(|l| l.to_string()).collect();
            v.extend(block.lines().map(str::to_string));
            if d.end < lines.len() {
                v.push(String::new());
                v.extend(lines[d.end..].iter().map(|l| l.to_string()));
            }
            format!("{}\n", v.join("\n").trim_end())
        }
        None if dest.trim().is_empty() => block.to_string(),
        None => format!("{}\n\n{block}", dest.trim_end()),
    };
    Some(if crlf { out.replace('\n', "\r\n") } else { out })
}

// --- JSON parts -----------------------------------------------------------------------------------------------

fn parse_json(text: &str, what: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|e| Error(format!("{what} is not valid JSON: {e}")))
}

/// Two-space JSON with a final newline, keys in their order.
pub fn pretty(v: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(v).unwrap_or_default())
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The value at `pointer` wrapped in its parents (objects keep the key, arrays become one-element arrays), ready to
/// [`merge`] into another document.
fn fragment(src: &Value, pointer: &str) -> Option<Value> {
    let mut value = src.pointer(pointer)?.clone();
    let tokens: Vec<&str> = if pointer.is_empty() { Vec::new() } else { pointer.strip_prefix('/')?.split('/').collect() };
    for i in (0..tokens.len()).rev() {
        let parent: String = tokens[..i].iter().map(|t| format!("/{t}")).collect();
        value = match src.pointer(&parent)? {
            Value::Array(_) => Value::Array(vec![value]),
            _ => Value::Object(Map::from_iter([(tokens[i].replace("~1", "/").replace("~0", "~"), value)])),
        };
    }
    Some(value)
}

/// Merge `src` into `dst`: objects by key, arrays by appending elements they lack, other values replaced only with
/// `overwrite`. Differences are listed in `conflicts` as JSON Pointers either way.
fn merge(dst: &mut Value, src: &Value, at: &str, overwrite: bool, conflicts: &mut Vec<String>) {
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(k) {
                    Some(dv) => merge(dv, v, &format!("{at}/{}", escape(k)), overwrite, conflicts),
                    None => {
                        d.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (Value::Array(d), Value::Array(s)) => {
            for v in s {
                if !d.contains(v) {
                    d.push(v.clone());
                }
            }
        }
        (d, s) if *d == *s => {}
        (d, s) => {
            conflicts.push(if at.is_empty() { "/".into() } else { at.to_string() });
            if overwrite {
                *d = s.clone();
            }
        }
    }
}

/// Copyable parts of a JSON document: each top-level key; inside objects each key; inside arrays each element
/// (one permission rule, one hook group, one MCP server). `(pointer, label)`.
pub fn json_parts(text: &str) -> Vec<(String, String)> {
    let Ok(Value::Object(top)) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let short = |v: &Value| {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if s.chars().count() > 70 { format!("{}…", s.chars().take(70).collect::<String>()) } else { s }
    };
    let mut out = Vec::new();
    for (k, v) in &top {
        let p1 = format!("/{}", escape(k));
        out.push((p1.clone(), k.clone()));
        match v {
            Value::Object(m) => {
                for (k2, v2) in m {
                    let p2 = format!("{p1}/{}", escape(k2));
                    out.push((p2.clone(), format!("{k} › {k2}")));
                    if let Value::Array(a) = v2 {
                        out.extend(a.iter().enumerate().map(|(i, e)| (format!("{p2}/{i}"), format!("{k} › {k2} › {}", short(e)))));
                    }
                }
            }
            Value::Array(a) => out.extend(a.iter().enumerate().map(|(i, e)| (format!("{p1}/{i}"), format!("{k} › {}", short(e))))),
            _ => {}
        }
    }
    out
}

// --- diff -----------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diff {
    Same,
    Added,
    Removed,
}

/// Line diff (longest common subsequence between the common head and tail).
// ponytail: O(n·m) table; above 4M cells the middle is shown as all removed then all added.
pub fn diff_lines<'a>(a: &'a str, b: &'a str) -> Vec<(Diff, &'a str)> {
    let (x, y): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let pre = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
    let suf = x[pre..].iter().rev().zip(y[pre..].iter().rev()).take_while(|(p, q)| p == q).count();
    let (xm, ym) = (&x[pre..x.len() - suf], &y[pre..y.len() - suf]);
    let mut out: Vec<(Diff, &str)> = x[..pre].iter().map(|l| (Diff::Same, *l)).collect();
    let (n, m) = (xm.len(), ym.len());
    if n * m > 4_000_000 {
        out.extend(xm.iter().map(|l| (Diff::Removed, *l)));
        out.extend(ym.iter().map(|l| (Diff::Added, *l)));
    } else {
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if xm[i] == ym[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if xm[i] == ym[j] {
                out.push((Diff::Same, xm[i]));
                (i, j) = (i + 1, j + 1);
            } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
                out.push((Diff::Removed, xm[i]));
                i += 1;
            } else {
                out.push((Diff::Added, ym[j]));
                j += 1;
            }
        }
        out.extend(xm[i..].iter().map(|l| (Diff::Removed, *l)));
        out.extend(ym[j..].iter().map(|l| (Diff::Added, *l)));
    }
    out.extend(x[x.len() - suf..].iter().map(|l| (Diff::Same, *l)));
    out
}

/// A unified-style diff: changed lines with `+` / `-`, three lines of context as `  `, `…` between hunks.
pub fn unified(before: &str, after: &str) -> String {
    let d = diff_lines(before, after);
    let near: Vec<bool> = (0..d.len()).map(|i| d[i.saturating_sub(3)..(i + 4).min(d.len())].iter().any(|(k, _)| *k != Diff::Same)).collect();
    let mut out = String::new();
    let mut gap = false;
    for (i, (k, line)) in d.iter().enumerate() {
        if !near[i] {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push_str("…\n");
        }
        gap = false;
        let mark = match k {
            Diff::Same => "  ",
            Diff::Added => "+ ",
            Diff::Removed => "- ",
        };
        out.push_str(mark);
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-claude-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn last_session_reads_the_newest_transcript_of_the_folder() {
        let home = temp("sessions");
        assert_eq!(last_session(&home, r"C:\x\my.repo"), None);
        write(&home, ".claude/projects/C--x-my-repo/a.jsonl", "{}\n");
        write(&home, ".claude/projects/C--x-my-repo/notes.txt", "");
        let t = last_session(&home, r"C:\x\my.repo\").expect("found by the encoded folder name");
        assert!((crate::now_secs() - t).abs() < 60);
        write(&home, ".claude/projects/C--x-my-repo--claude-worktrees-feat-1a2b3c/b.jsonl", "{}\n");
        write(&home, ".claude/projects/C--x-my-repository/c.jsonl", "{}\n");
        assert_eq!(session_times(&home, r"C:\x\my.repo").len(), 2, "the folder and its Claude worktrees, not a longer name");
    }

    #[test]
    fn recent_sessions_show_the_first_typed_prompt() {
        let home = temp("recent");
        let lines = [
            r#"{"type":"summary","summary":"x"}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Caveat: meta"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>\nx\n</system-reminder>"},{"type":"text","text":"<system-reminder>y</system-reminder>\n\nLet's work on M6.\nMore."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"later"}}"#,
        ];
        write(&home, ".claude/projects/C--x-r/a.jsonl", &lines.join("\n"));
        write(&home, ".claude/projects/C--x-r--claude-worktrees-w/b.jsonl", "not json\n");
        let folders = [r"C:\x\r".to_string()];
        let mut prompts: Vec<String> = recent_sessions(&home, &folders, 10).into_iter().map(|s| s.prompt).collect();
        prompts.sort();
        assert_eq!(prompts, ["", "Let's work on M6."]);
        assert_eq!(recent_sessions(&home, &folders, 1).len(), 1);
    }

    /// A project, a home folder with `.claude`, and a data folder for backups.
    fn fixture(name: &str) -> (Scope, Scope, PathBuf) {
        let base = temp(name);
        let (proj, home) = (base.join("proj"), base.join("home"));
        write(&proj, "CLAUDE.md", "# Proj\n\n## Rules\n\n- be nice\n\n## Checks\n\n```\n# not a heading\ncargo test\n```\n");
        write(&proj, ".claude/settings.json", r#"{"permissions":{"allow":["Bash(cargo test:*)"]},"env":{"A":"1"}}"#);
        write(&proj, ".claude/agents/reviewer.md", "---\nname: reviewer\n---\nReview.\n");
        write(&proj, ".claude/skills/pdf/SKILL.md", "# PDF\n");
        write(&proj, ".claude/skills/pdf/scripts/run.py", "print(1)\n");
        write(&proj, "docs/CLAUDE.md", "# Docs\n");
        write(&proj, "node_modules/x/CLAUDE.md", "# no\n");
        write(&proj, "opencode.json", "{}");
        write(&home, ".claude/settings.json", "{\n  \"env\": {\n    \"A\": \"2\"\n  },\n  \"model\": \"opus\"\n}\n");
        write(&home, ".claude/agents/global-one.md", "Global.\n");
        (Scope::project(&proj), Scope::global(&home), base.join("data"))
    }

    #[test]
    fn classify_names_and_refuses_the_rest() {
        assert_eq!(classify("CLAUDE.md"), Some((Kind::Instructions, false)));
        assert_eq!(classify("src/ui/CLAUDE.md"), Some((Kind::Instructions, false)));
        assert_eq!(classify(".claude/agents/x.md"), Some((Kind::Agent, false)));
        assert_eq!(classify(".claude/skills/pdf/SKILL.md"), Some((Kind::Skill, false)));
        assert_eq!(classify("opencode.json"), Some((Kind::Other, true)));
        for bad in
            ["src/main.rs", "../CLAUDE.md", "a/../CLAUDE.md", ".claude/worktrees/x/CLAUDE.md", "C:/x/CLAUDE.md", "node_modules/a/CLAUDE.md", ""]
        {
            assert_eq!(classify(bad), None, "{bad}");
        }
        assert_eq!(norm_rel(r".\.claude\agents\x.md").unwrap(), ".claude/agents/x.md");
    }

    #[test]
    fn inventory_of_a_project_and_of_global() {
        let (proj, global, _) = fixture("inventory");
        let items = inventory(&proj);
        let rels: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        assert_eq!(
            rels,
            ["CLAUDE.md", "docs/CLAUDE.md", ".claude/settings.json", ".claude/agents/reviewer.md", ".claude/skills/pdf", "opencode.json"]
        );
        assert_eq!(items[3].name, "reviewer");
        assert!(items[4].dir && items[4].name == "pdf");
        assert!(items[5].read_only);
        assert_eq!(items[1].column(), "nested CLAUDE.md");
        let g: Vec<String> = inventory(&global).into_iter().map(|i| i.column()).collect();
        assert_eq!(g, ["settings.json", "agent: global-one"]);
        assert_eq!(global.path(".claude/agents/x.md"), global.root.join("agents").join("x.md"));
        assert_eq!(global.path("CLAUDE.md"), global.root.join("CLAUDE.md"));
    }

    #[test]
    fn whole_files_and_skills_copy_with_conflicts_and_backups() {
        let (proj, global, data) = fixture("whole");
        let plan = plan_copy(&proj, ".claude/agents/reviewer.md", &Part::Whole, &global, None, false).unwrap();
        assert_eq!(plan.changes.len(), 1);
        assert!(!plan.changes[0].existed && plan.conflicts.is_empty());
        assert!(apply(&data, &plan).unwrap().is_empty(), "nothing to back up for a new file");
        assert_eq!(fs::read_to_string(global.root.join("agents/reviewer.md")).unwrap(), "---\nname: reviewer\n---\nReview.\n");
        assert_eq!(
            plan_copy(&proj, ".claude/agents/reviewer.md", &Part::Whole, &global, None, false).unwrap(),
            Plan::default(),
            "same file: nothing to do"
        );

        fs::write(global.root.join("agents/reviewer.md"), "changed\n").unwrap();
        let kept = plan_copy(&proj, ".claude/agents/reviewer.md", &Part::Whole, &global, None, false).unwrap();
        assert!(kept.changes.is_empty() && kept.conflicts.len() == 1);
        let plan = plan_copy(&proj, ".claude/agents/reviewer.md", &Part::Whole, &global, None, true).unwrap();
        assert!(plan.changes[0].diff().contains("- changed\n"), "{}", plan.changes[0].diff());
        let backups = apply(&data, &plan).unwrap();
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "changed\n");

        let skill = plan_copy(&proj, ".claude/skills/pdf", &Part::Whole, &global, None, false).unwrap();
        apply(&data, &skill).unwrap();
        assert!(global.root.join("skills/pdf/scripts/run.py").is_file());
        let err = plan_copy(&proj, ".mcp.json", &Part::Whole, &global, None, false).unwrap_err().to_string();
        assert!(err.contains("does not exist") || err.contains("no place"), "{err}");
        let err = plan_copy(&global, ".claude/agents/global-one.md", &Part::Whole, &proj, Some("opencode.json"), true).unwrap_err().to_string();
        assert!(err.contains("not a Claude config") || err.contains("read-only"), "{err}");
    }

    #[test]
    fn markdown_sections_append_or_replace() {
        let (proj, global, data) = fixture("sections");
        let titles: Vec<String> = md_sections(&fs::read_to_string(proj.path("CLAUDE.md")).unwrap()).into_iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Proj", "Rules", "Checks"], "headings in code fences are not sections");
        let plan = plan_copy(&proj, "CLAUDE.md", &Part::Section("Rules".into()), &global, None, false).unwrap();
        apply(&data, &plan).unwrap();
        assert_eq!(fs::read_to_string(global.path("CLAUDE.md")).unwrap(), "## Rules\n\n- be nice\n");

        fs::write(global.path("CLAUDE.md"), "# Me\n\n## Rules\n\n- old\n\n## Tail\n\nkeep\n").unwrap();
        let kept = plan_copy(&proj, "CLAUDE.md", &Part::Section("Rules".into()), &global, None, false).unwrap();
        assert!(kept.changes.is_empty() && kept.conflicts[0].contains("Rules"));
        let plan = plan_copy(&proj, "CLAUDE.md", &Part::Section("Rules".into()), &global, None, true).unwrap();
        apply(&data, &plan).unwrap();
        assert_eq!(fs::read_to_string(global.path("CLAUDE.md")).unwrap(), "# Me\n\n## Rules\n\n- be nice\n\n## Tail\n\nkeep\n");
        let plan = plan_copy(&proj, "CLAUDE.md", &Part::Section("Checks".into()), &global, None, false).unwrap();
        apply(&data, &plan).unwrap();
        assert!(fs::read_to_string(global.path("CLAUDE.md")).unwrap().ends_with("keep\n\n## Checks\n\n```\n# not a heading\ncargo test\n```\n"));
    }

    #[test]
    fn json_keys_merge_and_report_conflicts() {
        let (proj, global, data) = fixture("json");
        let parts: Vec<String> = json_parts(&fs::read_to_string(proj.path(".claude/settings.json")).unwrap()).into_iter().map(|p| p.0).collect();
        assert_eq!(parts, ["/permissions", "/permissions/allow", "/permissions/allow/0", "/env", "/env/A"]);
        let plan = plan_copy(&proj, ".claude/settings.json", &Part::Key("/permissions/allow/0".into()), &global, None, false).unwrap();
        apply(&data, &plan).unwrap();
        let text = fs::read_to_string(global.path(".claude/settings.json")).unwrap();
        assert_eq!(
            text,
            "{\n  \"env\": {\n    \"A\": \"2\"\n  },\n  \"model\": \"opus\",\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(cargo test:*)\"\n    ]\n  }\n}\n",
            "keys keep their order"
        );
        assert_eq!(
            plan_copy(&proj, ".claude/settings.json", &Part::Key("/permissions/allow/0".into()), &global, None, false).unwrap(),
            Plan::default()
        );

        let kept = plan_copy(&proj, ".claude/settings.json", &Part::Key("".into()), &global, None, false).unwrap();
        assert_eq!(kept.conflicts, [".claude/settings.json: /env/A differs"]);
        assert!(kept.changes.is_empty());
        let plan = plan_copy(&proj, ".claude/settings.json", &Part::Key("/env".into()), &global, None, true).unwrap();
        apply(&data, &plan).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(global.path(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(v["env"]["A"], "1");
        assert_eq!(read_part(&fs::read_to_string(proj.path(".claude/settings.json")).unwrap(), &Part::Key("/env/A".into())).unwrap(), "\"1\"");
    }

    #[test]
    fn edits_validate_json_and_refuse_changed_files() {
        let (proj, _, data) = fixture("edit");
        let err = plan_edit(&proj, ".claude/settings.json", None, "{ nope").unwrap_err().to_string();
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(plan_edit(&proj, "opencode.json", None, "{}").is_err(), "read-only");
        assert!(plan_edit(&proj, "src/lib.rs", None, "x").is_err(), "not Claude config");
        let read = fs::read_to_string(proj.path("CLAUDE.md")).unwrap();
        let plan = plan_edit(&proj, "CLAUDE.md", Some(&read), "# New\n").unwrap();
        fs::write(proj.path("CLAUDE.md"), "# Someone else\n").unwrap();
        let err = apply(&data, &plan).unwrap_err().to_string();
        assert!(err.contains("changed outside"), "{err}");
        assert_eq!(fs::read_to_string(proj.path("CLAUDE.md")).unwrap(), "# Someone else\n");
        let plan = plan_edit(&proj, ".claude/commands/go.md", None, "Go.\n").unwrap();
        assert!(!plan.changes[0].existed);
        apply(&data, &plan).unwrap();
        assert!(inventory(&proj).iter().any(|i| i.kind == Kind::Command && i.name == "go"));
    }

    #[test]
    fn unified_diff_marks_lines_with_context() {
        let a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
        let b = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n10\n11\n12\n13\n";
        assert_eq!(unified(a, b), "  2\n  3\n  4\n- 5\n+ five\n  6\n  7\n  8\n…\n  10\n  11\n  12\n+ 13\n");
        assert_eq!(unified("same\n", "same\n"), "");
    }
}
