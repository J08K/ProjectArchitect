//! Project templates (SPEC § 9, D-10, D-21). A template is a folder with `template.toml` (name, prompts, steps) and
//! usually `files/`, an overlay copied into the new project. Creating a project fills the `{{placeholders}}`, runs the
//! steps in order with their output streamed, then tracks the new folder. A failed step stops the run; the partly
//! created folder is left in place and reported, never deleted. The templates folder is itself a tracked project.

use crate::db::Store;
use crate::info::{self, SKIP_DIRS};
use crate::ops::{self, Error, ProjectEdit, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

/// Store setting: the templates folder. Default `<data dir>\templates`.
pub const DIR: &str = "templates.dir";
/// Store setting: the templates folder the app has tracked as a project once (so removing it from the app sticks).
pub const TRACKED: &str = "templates.tracked";
/// Store setting: the parent folder last chosen in the New Project dialog.
pub const PARENT: &str = "templates.parent";
pub const FILE: &str = "template.toml";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Template {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// A purpose name; ignored when the app has no such purpose.
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "prompt")]
    pub prompts: Vec<Prompt>,
    #[serde(default, rename = "step")]
    pub steps: Vec<Step>,
    /// The template's folder (not in the file).
    #[serde(skip)]
    pub dir: PathBuf,
}

/// A question of the New Project dialog; its answer is the placeholder `{{key}}`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Prompt {
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub default: String,
}

/// A command (`run`, in `cwd`, default the parent folder) or an overlay folder copied into the new project.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Step {
    pub run: Option<String>,
    pub overlay: Option<String>,
    pub cwd: Option<String>,
}

impl Template {
    /// The prompts, `name` first (added when the file has none: every project needs one).
    pub fn questions(&self) -> Vec<Prompt> {
        let mut v: Vec<Prompt> = self.prompts.iter().filter(|p| p.key == "name").cloned().collect();
        if v.is_empty() {
            v.push(Prompt { key: "name".into(), label: "Project name".into(), default: String::new() });
        }
        v.extend(self.prompts.iter().filter(|p| p.key != "name").cloned());
        v
    }

    /// The folder's name, used to find the template.
    pub fn folder(&self) -> String {
        crate::folder_name(&self.dir.to_string_lossy())
    }
}

/// The templates folder: the `templates.dir` setting, else `<data dir>/templates`.
pub fn dir(store: &Store, data_dir: &Path) -> Result<PathBuf> {
    Ok(match store.setting(DIR)?.filter(|d| !d.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => data_dir.join("templates"),
    })
}

/// Read a `template.toml` and check it: a name, unique prompt keys, each step either `run` or `overlay`.
pub fn parse(text: &str, dir: &Path) -> std::result::Result<Template, String> {
    let mut t: Template = toml::from_str(text).map_err(|e| e.to_string())?;
    if t.name.trim().is_empty() {
        return Err("name is empty".into());
    }
    let mut keys = HashSet::new();
    if let Some(p) = t.prompts.iter().find(|p| p.key.trim().is_empty() || !keys.insert(p.key.as_str())) {
        return Err(format!("prompt key {:?} is empty or used twice", p.key));
    }
    if let Some(i) = t.steps.iter().position(|s| s.run.is_some() == s.overlay.is_some()) {
        return Err(format!("step {} needs either run or overlay", i + 1));
    }
    t.dir = dir.to_path_buf();
    Ok(t)
}

/// A template folder, read or not.
pub struct Listed {
    pub dir: PathBuf,
    pub template: std::result::Result<Template, String>,
}

/// Every subfolder with a `template.toml`, by folder name.
pub fn list(dir: &Path) -> Vec<Listed> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join(FILE).is_file()).collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| {
            let template = std::fs::read_to_string(d.join(FILE)).map_err(|e| e.to_string()).and_then(|t| parse(&t, &d));
            Listed { dir: d, template }
        })
        .collect()
}

/// A template by folder name or by name (case-insensitive).
pub fn find(dir: &Path, key: &str) -> Result<Template> {
    let key = key.trim();
    for l in list(dir) {
        let folder = crate::folder_name(&l.dir.to_string_lossy());
        match l.template {
            Ok(t) if folder.eq_ignore_ascii_case(key) || t.name.eq_ignore_ascii_case(key) => return Ok(t),
            Err(e) if folder.eq_ignore_ascii_case(key) => return Err(Error(format!("template {folder}: {e}"))),
            _ => {}
        }
    }
    Err(Error(format!("no template {key:?} in {}", dir.display())))
}

/// Seed the folder with the built-in templates and track it as a project once (D-21). Returns the folder.
pub fn ensure(store: &Store, data_dir: &Path) -> Result<PathBuf> {
    let dir = dir(store, data_dir)?;
    seed(&dir)?;
    let path = crate::norm(&dir);
    if store.setting(TRACKED)?.as_deref() != Some(path.as_str()) {
        if store.checkout_by_path(&path)?.is_none() {
            ops::import_folder(store, &dir, Some("Templates"), None, None)?;
        }
        store.set_setting(TRACKED, &path)?;
    }
    Ok(dir)
}

// --- placeholders ---------------------------------------------------------------------------------------------

/// Placeholder values by key.
pub type Vars = BTreeMap<String, String>;

/// `{{key}}` (spaces inside allowed) replaced by its value; unknown keys stay as they are.
pub fn fill(text: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}").and_then(|j| vars.get(after[..j].trim()).map(|v| (j, v))) {
            Some((j, v)) => {
                out.push_str(v);
                rest = &after[j + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A folder name that works on Windows and Linux.
fn check_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.ends_with(['.', ' ']) || name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']) {
        return Err(format!("{name:?} cannot be a folder name"));
    }
    Ok(())
}

/// The placeholders: `date`, `year`, `author` (git `user.name`), the prompts (answer, else default), `name` and
/// `project_dir`.
pub fn vars(t: &Template, values: &HashMap<String, String>, parent: &Path) -> std::result::Result<Vars, String> {
    let (y, m, d) = civil(crate::today_days());
    let mut v = Vars::new();
    v.insert("date".into(), format!("{y:04}-{m:02}-{d:02}"));
    v.insert("year".into(), y.to_string());
    v.insert("author".into(), info::git(parent, &["config", "user.name"]).unwrap_or_default());
    for p in &t.prompts {
        v.insert(p.key.clone(), p.default.clone());
    }
    for (k, x) in values {
        if !x.trim().is_empty() || !v.contains_key(k) {
            v.insert(k.clone(), x.trim().to_string());
        }
    }
    let name = v.get("name").cloned().unwrap_or_default();
    check_name(&name)?;
    v.insert("project_dir".into(), parent.join(&name).display().to_string());
    Ok(v)
}

/// `(year, month, day)` of days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

// --- creating a project ---------------------------------------------------------------------------------------

/// A step with its placeholders filled.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Run {
        command: String,
        cwd: PathBuf,
    },
    /// Files of `from` (relative, `/`-separated; their names filled too) copied into the project folder.
    Overlay {
        from: PathBuf,
        files: Vec<String>,
    },
}

/// What creating a project will do, for the preview.
#[derive(Clone, Debug)]
pub struct Planned {
    pub project_dir: PathBuf,
    pub vars: Vars,
    pub actions: Vec<Action>,
}

impl Planned {
    pub fn name(&self) -> &str {
        self.vars.get("name").map_or("", String::as_str)
    }
}

/// Fill in a template for `parent`. Refused when the project folder exists already.
pub fn plan(t: &Template, values: &HashMap<String, String>, parent: &Path) -> std::result::Result<Planned, String> {
    if !parent.is_dir() {
        return Err(format!("the folder {} does not exist", parent.display()));
    }
    let vars = vars(t, values, parent)?;
    let project_dir = parent.join(&vars["name"]);
    if project_dir.exists() {
        return Err(format!("{} exists already; choose another name or folder", project_dir.display()));
    }
    let mut actions = Vec::new();
    for s in &t.steps {
        if let Some(run) = &s.run {
            let cwd = s.cwd.as_deref().map_or_else(|| parent.to_path_buf(), |c| parent.join(fill(c, &vars)));
            actions.push(Action::Run { command: fill(run, &vars), cwd });
        } else if let Some(o) = &s.overlay {
            let from = t.dir.join(o);
            if !from.is_dir() {
                return Err(format!("the overlay folder {} is missing", from.display()));
            }
            let mut files = Vec::new();
            walk(&from, "", &mut files);
            actions.push(Action::Overlay { from, files });
        }
    }
    Ok(Planned { project_dir, vars, actions })
}

/// Run the planned steps in order; `out` gets each output line (commands as `$ …`). Stops at the first failure,
/// leaving what was created.
pub fn run(plan: &Planned, out: &mut dyn FnMut(String)) -> std::result::Result<(), String> {
    for (i, a) in plan.actions.iter().enumerate() {
        let r = match a {
            Action::Run { command, cwd } => {
                out(format!("$ {command}"));
                shell(command, cwd, out)
            }
            Action::Overlay { from, files } => {
                overlay(from, files, &plan.project_dir, &plan.vars).map(|n| out(format!("copied {n} files from {}", from.display())))
            }
        };
        if let Err(e) = r {
            let left = if plan.project_dir.exists() {
                format!(". The partly created folder {} is left in place", plan.project_dir.display())
            } else {
                String::new()
            };
            return Err(format!("step {} failed: {e}{left}", i + 1));
        }
    }
    if !plan.project_dir.is_dir() {
        return Err(format!("the steps ran but {} was not created", plan.project_dir.display()));
    }
    Ok(())
}

/// Run a command line through the shell in `cwd`, streaming stdout and stderr lines.
fn shell(command: &str, cwd: &Path, out: &mut dyn FnMut(String)) -> std::result::Result<(), String> {
    if !cwd.is_dir() {
        return Err(format!("the folder {} does not exist", cwd.display()));
    }
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = crate::cmd("cmd");
        c.raw_arg(format!("/S /C \"{command}\""));
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = crate::cmd("sh");
        c.arg("-c").arg(command);
        c
    };
    let mut child = c
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start `{command}`: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let pipes: Vec<Box<dyn Read + Send>> =
        [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as _)]
            .into_iter()
            .flatten()
            .collect();
    for pipe in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut r = io::BufReader::new(pipe);
            let mut buf = Vec::new();
            while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                let _ = tx.send(String::from_utf8_lossy(&buf).trim_end().to_string());
                buf.clear();
            }
        });
    }
    drop(tx);
    for line in rx {
        out(line);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(format!("`{command}` exited with code {code}")),
        None => Err(format!("`{command}` was stopped")),
    }
}

/// Copy the overlay files, placeholders filled in text files and in names. Returns how many.
fn overlay(from: &Path, files: &[String], to: &Path, vars: &Vars) -> std::result::Result<usize, String> {
    let copy = || -> io::Result<()> {
        std::fs::create_dir_all(to)?;
        for f in files {
            let src = f.split('/').fold(from.to_path_buf(), |p, s| p.join(s));
            let dest = f.split('/').fold(to.to_path_buf(), |p, s| p.join(fill(s, vars)));
            if let Some(dir) = dest.parent() {
                std::fs::create_dir_all(dir)?;
            }
            match String::from_utf8(std::fs::read(&src)?) {
                Ok(t) if !t.contains('\0') => std::fs::write(&dest, fill(&t, vars))?,
                Ok(t) => std::fs::write(&dest, t)?,
                Err(e) => std::fs::write(&dest, e.into_bytes())?,
            }
        }
        Ok(())
    };
    copy().map_err(|e| e.to_string())?;
    Ok(files.len())
}

/// Files under `dir`, relative and `/`-separated, sorted.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if e.path().is_dir() {
            walk(&e.path(), &rel, out);
        } else {
            out.push(rel);
        }
    }
}

/// Track a created project folder: named after the `name` answer, in `group` (default: the group of its parent
/// folder), with the template's purpose and tags.
pub fn finish(store: &Store, t: &Template, plan: &Planned, group: Option<&str>) -> Result<i64> {
    let purpose = t.purpose.as_deref().filter(|p| ops::purpose_named(store, p).is_ok());
    let imported = ops::import_folder(store, &plan.project_dir, Some(plan.name()), group, purpose)?;
    if !t.tags.is_empty() {
        let mut p = ops::find_project(store, &imported.project_id.to_string())?;
        let mut tags = p.tags.clone();
        tags.extend(t.tags.iter().cloned());
        tags.sort();
        tags.dedup();
        ops::edit_project(store, &mut p, ProjectEdit { tags: Some(tags), ..Default::default() })?;
    }
    Ok(imported.project_id)
}

/// Plan, run and track in one go (MCP `create_project`). Returns the new project's id.
pub fn create(
    store: &Store,
    t: &Template,
    values: &HashMap<String, String>,
    parent: &Path,
    group: Option<&str>,
    out: &mut dyn FnMut(String),
) -> Result<(i64, PathBuf)> {
    let plan = plan(t, values, parent).map_err(Error)?;
    run(&plan, out).map_err(Error)?;
    Ok((finish(store, t, &plan, group)?, plan.project_dir))
}

// --- managing templates ---------------------------------------------------------------------------------------

/// Copy a folder, `.git` left out.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        if e.path().is_dir() {
            copy_dir(&e.path(), &to.join(&name))?;
        } else {
            std::fs::copy(e.path(), to.join(&name))?;
        }
    }
    Ok(())
}

/// The `name = …` line set to `name` (added on top when missing); everything else kept.
fn set_name(text: &str, name: &str) -> String {
    let line = format!("name = {}", toml::Value::String(name.to_string()));
    let mut done = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let is_name = l.trim_start().strip_prefix("name").is_some_and(|r| r.trim_start().starts_with('='));
            if is_name && !done {
                done = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !done {
        out.insert(0, line);
    }
    format!("{}\n", out.join("\n"))
}

/// A free folder name next to the others: `base`, else `base-2`, `base-3`…
fn free_folder(dir: &Path, base: &str) -> PathBuf {
    (1..).map(|i| if i == 1 { dir.join(base) } else { dir.join(format!("{base}-{i}")) }).find(|p| !p.exists()).expect("a free name")
}

/// Copy a template's folder as `<folder>-copy`, named "<name> (copy)". Returns the new folder.
pub fn duplicate(t: &Template) -> Result<PathBuf> {
    let parent = t.dir.parent().ok_or_else(|| Error("the template has no parent folder".into()))?;
    let dest = free_folder(parent, &format!("{}-copy", t.folder()));
    copy_dir(&t.dir, &dest)?;
    let text = std::fs::read_to_string(dest.join(FILE))?;
    std::fs::write(dest.join(FILE), set_name(&text, &format!("{} (copy)", t.name)))?;
    Ok(dest)
}

/// Top-level entries of a project offered for "make template from this project": `(name, is folder)`, without
/// `.git` and build or dependency folders.
pub fn project_entries(root: &Path) -> Vec<(String, bool)> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut v: Vec<(String, bool)> = rd
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().is_dir()))
        .filter(|(n, _)| n != ".git" && !SKIP_DIRS.contains(&n.as_str()))
        .collect();
    v.sort_by_key(|(n, d)| (!d, n.to_lowercase()));
    v
}

/// Make a template from chosen files and folders of a project: they become the overlay (copied as they are), with
/// steps overlay → `git init` + first commit. Returns the new template folder.
pub fn from_project(templates_dir: &Path, name: &str, source: &Path, picks: &[String]) -> Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error("a template needs a name".into()));
    }
    let slug: String = name.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.trim_matches('-');
    let dest = free_folder(templates_dir, if slug.is_empty() { "template" } else { slug });
    for rel in picks {
        if !Path::new(rel).components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(Error(format!("{rel:?} is not a path inside the project")));
        }
    }
    let files = dest.join("files");
    std::fs::create_dir_all(&files)?;
    for rel in picks {
        let src = source.join(rel);
        if src.is_dir() {
            copy_dir(&src, &files.join(rel))?;
        } else {
            if let Some(dir) = files.join(rel).parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::copy(&src, files.join(rel))?;
        }
    }
    let toml = format!(
        "name = {}\ndescription = {}\n\n{PROMPTS}\n[[step]]\noverlay = \"files\"\n\n{COMMIT}",
        toml::Value::String(name.to_string()),
        toml::Value::String(format!("Made from {}", crate::folder_name(&source.to_string_lossy()))),
    );
    std::fs::write(dest.join(FILE), toml)?;
    Ok(dest)
}

// --- seed templates -------------------------------------------------------------------------------------------

const PROMPTS: &str = "[[prompt]]\nkey = \"name\"\nlabel = \"Project name\"\n\n[[prompt]]\nkey = \"description\"\nlabel = \"One-line description\"\n";
const COMMIT: &str = "[[step]]\nrun = 'git init -q && git add -A && git commit -q -m \"Initial commit\"'\ncwd = \"{{project_dir}}\"\n";

const CLAUDE_MD: &str = "# CLAUDE.md\n\n{{name}}: {{description}}\n\nOpen work is in [TODO.md](TODO.md), milestones in [ROADMAP.md](ROADMAP.md).\n";
const TODO_MD: &str = "# TODO\n\n## Open\n\n- [ ] First feature\n";
const ROADMAP_MD: &str = "# Roadmap\n\n## v0.1.0 — First version\nStatus: planned\n\n- [ ] Something that works end to end\n";
const README_MD: &str = "# {{name}}\n\n{{description}}\n";

const EMPTY_TOML: &str =
    "name = \"Empty folder\"\ndescription = \"A folder with git, README.md, TODO.md, ROADMAP.md and CLAUDE.md\"\npurpose = \"Developing\"\n\n";
const RUST_TOML: &str = "name = \"Rust egui app\"\ndescription = \"An eframe/egui 0.36 desktop app: cargo new, then an egui main.rs\"\npurpose = \"Developing\"\ntags = [\"rust\"]\n\n[[prompt]]\nkey = \"name\"\nlabel = \"Project name (a crate name: lowercase, dashes)\"\n\n[[prompt]]\nkey = \"description\"\nlabel = \"One-line description\"\n\n[[step]]\nrun = \"cargo new {{name}} --vcs git\"\n\n[[step]]\noverlay = \"files\"\n\n[[step]]\nrun = 'git add -A && git commit -q -m \"Initial commit\"'\ncwd = \"{{project_dir}}\"\n";
const RUST_CARGO: &str = "[package]\nname = \"{{name}}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\neframe = \"0.36\"\n";
const RUST_MAIN: &str = "use eframe::egui;\n\nfn main() -> eframe::Result {\n    eframe::run_native(\"{{name}}\", eframe::NativeOptions::default(), Box::new(|_cc| Ok(Box::new(App))))\n}\n\nstruct App;\n\nimpl eframe::App for App {\n    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {\n        egui::CentralPanel::default().show(ui, |ui| {\n            ui.heading(\"{{name}}\");\n        });\n    }\n}\n";
const PYTHON_TOML: &str =
    "name = \"Python (uv)\"\ndescription = \"A Python project managed by uv (uv init)\"\npurpose = \"Developing\"\ntags = [\"python\"]\n\n";
const PYTHON_STEPS: &str = "[[step]]\nrun = \"uv init {{name}}\"\n\n[[step]]\noverlay = \"files\"\n\n[[step]]\nrun = 'git add -A && git commit -q -m \"Initial commit\"'\ncwd = \"{{project_dir}}\"\n";
const SVELTE_TOML: &str = "name = \"SvelteKit\"\ndescription = \"A SvelteKit app (npx sv create: minimal, TypeScript, npm)\"\npurpose = \"Developing\"\ntags = [\"svelte\", \"typescript\"]\n\n";
const SVELTE_STEPS: &str =
    "[[step]]\nrun = \"npx --yes sv create {{name}} --template minimal --types ts --no-add-ons --install npm\"\n\n[[step]]\noverlay = \"files\"\n\n";

/// The built-in templates: folder and files.
fn seeds() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let docs = |extra: Vec<(&'static str, String)>| {
        let mut v =
            vec![("files/CLAUDE.md", CLAUDE_MD.to_string()), ("files/TODO.md", TODO_MD.to_string()), ("files/ROADMAP.md", ROADMAP_MD.to_string())];
        v.extend(extra);
        v
    };
    vec![
        (
            "empty",
            docs(vec![(FILE, format!("{EMPTY_TOML}{PROMPTS}\n[[step]]\noverlay = \"files\"\n\n{COMMIT}")), ("files/README.md", README_MD.into())]),
        ),
        (
            "rust-egui",
            docs(vec![
                (FILE, RUST_TOML.into()),
                ("files/Cargo.toml", RUST_CARGO.into()),
                ("files/src/main.rs", RUST_MAIN.into()),
                ("files/.gitignore", "/target\n".into()),
            ]),
        ),
        ("python-uv", docs(vec![(FILE, format!("{PYTHON_TOML}{PROMPTS}\n{PYTHON_STEPS}"))])),
        ("sveltekit", docs(vec![(FILE, format!("{SVELTE_TOML}{PROMPTS}\n{SVELTE_STEPS}{COMMIT}"))])),
    ]
}

/// Write the built-in templates whose folders are missing. Returns the folders written.
pub fn seed(dir: &Path) -> io::Result<Vec<String>> {
    let mut made = Vec::new();
    for (folder, files) in seeds() {
        let d = dir.join(folder);
        if d.exists() {
            continue;
        }
        for (rel, text) in files {
            let p = rel.split('/').fold(d.clone(), |p, s| p.join(s));
            std::fs::create_dir_all(p.parent().unwrap_or(&d))?;
            std::fs::write(p, text)?;
        }
        made.push(folder.to_string());
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-template-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A template folder `demo` with an overlay (a placeholder in a text and in a file name) and `steps`.
    fn demo(base: &Path, steps: &str) -> Template {
        let dir = base.join("templates").join("demo");
        fs::create_dir_all(dir.join("files/docs")).unwrap();
        fs::write(dir.join("files/README.md"), "# {{name}}\n{{description}} by {{ author }} in {{year}} {{unknown}}\n").unwrap();
        fs::write(dir.join("files/docs/{{name}}.txt"), "x").unwrap();
        let text = format!(
            "name = \"Demo\"\ntags = [\"demo\"]\npurpose = \"Developing\"\n{PROMPTS}\n[[prompt]]\nkey = \"extra\"\ndefault = \"dflt\"\n\n{steps}"
        );
        fs::write(dir.join(FILE), &text).unwrap();
        parse(&text, &dir).unwrap()
    }

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn parse_checks_steps_prompts_and_name() {
        let d = Path::new("x");
        assert!(parse("name = \"\"", d).unwrap_err().contains("name"));
        assert!(parse("name = \"a\"\n[[step]]\nrun = \"x\"\noverlay = \"files\"", d).unwrap_err().contains("step 1"));
        assert!(parse("name = \"a\"\n[[step]]\ncwd = \"x\"", d).unwrap_err().contains("step 1"));
        assert!(parse("name = \"a\"\n[[prompt]]\nkey = \"k\"\n[[prompt]]\nkey = \"k\"", d).unwrap_err().contains("twice"));
        let t = parse("name = \"a\"\n[[prompt]]\nkey = \"description\"", d).unwrap();
        let keys: Vec<String> = t.questions().into_iter().map(|p| p.key).collect();
        assert_eq!(keys, ["name", "description"], "name is always asked, first");
    }

    #[test]
    fn placeholders_and_dates() {
        let v: Vars = [("name".to_string(), "app".to_string())].into();
        assert_eq!(fill("{{name}} {{ name }} {{nope}} {{ {{name}}", &v), "app app {{nope}} {{ app");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(crate::days_from_civil(2026, 10, 17)), (2026, 10, 17));
        assert_eq!(civil(crate::days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn creates_a_project_with_overlay_and_commands_and_tracks_it() {
        let base = temp("create");
        let t = demo(
            &base,
            "[[step]]\noverlay = \"files\"\n\n[[step]]\nrun = \"echo made {{name}}\"\ncwd = \"{{project_dir}}\"\n\n[[step]]\nrun = \"git init -q\"\ncwd = \"{{project_dir}}\"\n",
        );
        let parent = base.join("code");
        fs::create_dir_all(&parent).unwrap();
        let store = Store::open_in_memory().unwrap();
        let mut lines = Vec::new();
        let (id, dir) = create(&store, &t, &values(&[("name", "hello"), ("description", "Says hi")]), &parent, None, &mut |l| lines.push(l)).unwrap();
        assert_eq!(dir, parent.join("hello"));
        let readme = fs::read_to_string(dir.join("README.md")).unwrap();
        assert!(readme.starts_with("# hello\nSays hi by "), "{readme}");
        assert!(readme.ends_with("{{unknown}}\n"), "{readme}");
        assert!(dir.join("docs/hello.txt").is_file(), "placeholders in names");
        assert!(dir.join(".git").is_dir());
        assert!(lines.iter().any(|l| l == "$ echo made hello") && lines.iter().any(|l| l.trim() == "made hello"), "{lines:?}");
        let p = ops::find_project(&store, &id.to_string()).unwrap();
        assert_eq!((p.name.as_str(), p.tags.clone()), ("hello", vec!["demo".to_string()]));
        assert!(p.purpose_id.is_some());
        let again = plan(&t, &values(&[("name", "hello")]), &parent).unwrap_err();
        assert!(again.contains("exists already"), "{again}");
        assert!(plan(&t, &values(&[("name", "a/b")]), &parent).unwrap_err().contains("folder name"));
        let planned = plan(&t, &values(&[("name", "next")]), &parent).unwrap();
        assert_eq!(planned.vars["extra"], "dflt", "defaults fill unanswered prompts");
        assert_eq!(planned.actions.len(), 3);
    }

    #[test]
    fn a_failed_step_stops_and_leaves_the_folder() {
        let base = temp("fail");
        let t = demo(&base, "[[step]]\noverlay = \"files\"\n\n[[step]]\nrun = \"exit 3\"\n\n[[step]]\nrun = \"echo never\"\n");
        let planned = plan(&t, &values(&[("name", "broken")]), &base).unwrap();
        let mut lines = Vec::new();
        let err = run(&planned, &mut |l| lines.push(l)).unwrap_err();
        assert!(err.contains("step 2 failed") && err.contains("code 3") && err.contains("left in place"), "{err}");
        assert!(base.join("broken/README.md").is_file());
        assert!(!lines.iter().any(|l| l.contains("never")));
    }

    #[test]
    fn seeds_once_all_valid_and_tracked() {
        let data = temp("seed");
        let store = Store::open_in_memory().unwrap();
        let dir = ensure(&store, &data).unwrap();
        assert_eq!(dir, data.join("templates"));
        let listed = list(&dir);
        assert_eq!(listed.len(), 4);
        for l in &listed {
            let t = l.template.as_ref().unwrap_or_else(|e| panic!("{}: {e}", l.dir.display()));
            assert!(plan(t, &values(&[("name", "x")]), &data).is_ok(), "{}", t.name);
        }
        fs::write(dir.join("empty/files/TODO.md"), "mine").unwrap();
        assert!(seed(&dir).unwrap().is_empty());
        assert_eq!(fs::read_to_string(dir.join("empty/files/TODO.md")).unwrap(), "mine", "existing templates are never rewritten");
        let projects = store.projects().unwrap();
        assert_eq!(projects.iter().filter(|p| p.name == "Templates").count(), 1);
        store.remove_project(projects[0].id).unwrap();
        ensure(&store, &data).unwrap();
        assert!(store.projects().unwrap().is_empty(), "removing the Templates project from the app sticks");
        assert_eq!(find(&dir, "rust egui app").unwrap().folder(), "rust-egui");
    }

    #[test]
    fn duplicate_and_from_project() {
        let base = temp("manage");
        let t = demo(&base, "[[step]]\noverlay = \"files\"\n");
        let copy = duplicate(&t).unwrap();
        assert_eq!(copy, base.join("templates/demo-copy"));
        let c = parse(&fs::read_to_string(copy.join(FILE)).unwrap(), &copy).unwrap();
        assert_eq!(c.name, "Demo (copy)");
        assert_eq!(c.prompts, t.prompts);
        assert_eq!(duplicate(&t).unwrap(), base.join("templates/demo-copy-2"));

        let src = base.join("src-project");
        fs::create_dir_all(src.join(".claude/agents")).unwrap();
        fs::create_dir_all(src.join("target")).unwrap();
        fs::write(src.join("CLAUDE.md"), "# Mine\n").unwrap();
        fs::write(src.join(".claude/agents/a.md"), "A").unwrap();
        let names: Vec<String> = project_entries(&src).into_iter().map(|e| e.0).collect();
        assert_eq!(names, [".claude", "CLAUDE.md"]);
        let made = from_project(&base.join("templates"), "My Kit!", &src, &[".claude".into(), "CLAUDE.md".into()]).unwrap();
        assert_eq!(made, base.join("templates/my-kit"));
        let t2 = find(&base.join("templates"), "My Kit!").unwrap();
        assert!(made.join("files/.claude/agents/a.md").is_file());
        let planned = plan(&t2, &values(&[("name", "new")]), &base).unwrap();
        assert!(matches!(&planned.actions[0], Action::Overlay { files, .. } if files.len() == 2));
        assert!(from_project(&base.join("templates"), "x", &src, &["../etc".into()]).is_err());
    }
}
