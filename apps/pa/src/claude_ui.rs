//! Claude config in the GUI (SPEC § 7.9, § 8, D-3): the project tab's Claude section and the global Claude page
//! (`~/.claude`, and a matrix of which project has which item). Saves and copies are `pa_core::claude` plans, shown
//! as a diff first; Apply backs up each replaced file, then writes it atomically after a re-read check.

use crate::theme::{self, Palette, caps, card, chip, pal};
use crate::{App, Tab};
use egui::{Align, Color32, Layout, RichText, Sense, vec2};
use egui_phosphor::regular as icon;
use pa_core::claude::{self, Item, Kind, Part, Plan, Scope};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// Hook events offered when adding a hook.
const EVENTS: &[&str] =
    &["PreToolUse", "PostToolUse", "UserPromptSubmit", "Notification", "Stop", "SubagentStop", "PreCompact", "SessionStart", "SessionEnd"];

#[derive(Default)]
pub struct State {
    /// Editors by scope: a project id, or `None` for the global `~/.claude`.
    eds: HashMap<Option<i64>, Ed>,
    matrix: Option<Matrix>,
    /// Global page: 0 = `~/.claude`, 1 = the projects matrix.
    view: usize,
    filter: Option<Kind>,
    copy: Option<CopyDlg>,
    pending: Option<Pending>,
    md: egui_commonmark::CommonMarkCache,
}

impl State {
    /// Show the matrix on the global page (from `--page claude-matrix`).
    pub fn show_matrix(&mut self) {
        self.view = 1;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Text,
    Preview,
    Form,
}

/// The items of one scope and the file being looked at.
struct Ed {
    scope: Scope,
    items: Vec<Item>,
    sel: Option<String>,
    /// The file shown: the item itself, or one file of a skill folder.
    file: String,
    /// The file as read; `None` when it does not exist yet (Save creates it).
    read: Option<String>,
    text: String,
    /// The settings form's working copy (settings files).
    form: Value,
    view: View,
    adds: Adds,
}

/// Text typed into the settings form's "add" fields.
#[derive(Default)]
struct Adds {
    rule: [String; 3],
    env: (String, String),
    hook: (usize, String, String),
}

fn is_settings(file: &str) -> bool {
    file.ends_with("settings.json") || file.ends_with("settings.local.json")
}

/// What a new file starts with.
fn starter(file: &str) -> String {
    if is_settings(file) {
        "{\n  \"permissions\": {\n    \"allow\": [],\n    \"deny\": []\n  }\n}\n".into()
    } else if file.ends_with(".mcp.json") {
        "{\n  \"mcpServers\": {}\n}\n".into()
    } else if file.ends_with(".md") {
        "# CLAUDE.md\n\n".into()
    } else {
        String::new()
    }
}

impl Ed {
    fn new(scope: Scope) -> Ed {
        let mut ed = Ed {
            scope,
            items: Vec::new(),
            sel: None,
            file: String::new(),
            read: None,
            text: String::new(),
            form: Value::Null,
            view: View::Text,
            adds: Adds::default(),
        };
        ed.items = claude::inventory(&ed.scope);
        if let Some(first) = ed.items.first().map(|i| i.rel.clone()) {
            ed.select(&first);
        }
        ed
    }

    fn item(&self) -> Option<&Item> {
        self.items.iter().find(|i| Some(&i.rel) == self.sel.as_ref())
    }

    /// Select an item (also one that does not exist yet); a skill opens its SKILL.md.
    fn select(&mut self, rel: &str) {
        self.sel = Some(rel.to_string());
        let file = if self.scope.path(rel).is_dir() {
            let files = claude::files(&self.scope, rel);
            files.iter().find(|f| f.ends_with("/SKILL.md")).or(files.first()).cloned().unwrap_or_else(|| format!("{rel}/SKILL.md"))
        } else {
            rel.to_string()
        };
        self.open(&file);
    }

    fn open(&mut self, file: &str) {
        self.file = file.to_string();
        self.read = std::fs::read_to_string(self.scope.path(file)).ok();
        self.text = self.read.clone().unwrap_or_else(|| starter(file));
        self.form = serde_json::from_str(&self.text).unwrap_or(Value::Null);
        self.view = if is_settings(file) && self.form.is_object() { View::Form } else { View::Text };
    }

    /// After a write: the list again, and the file unless it has unsaved edits.
    fn reload(&mut self) {
        self.items = claude::inventory(&self.scope);
        if !self.dirty() && !self.file.is_empty() {
            let f = self.file.clone();
            self.open(&f);
        }
    }

    /// The text to save: the form's JSON in the form view, else the editor's text.
    fn current(&self) -> String {
        if self.view == View::Form { claude::pretty(&self.form) } else { self.text.clone() }
    }

    fn dirty(&self) -> bool {
        match &self.read {
            None => !self.file.is_empty(),
            Some(r) if self.view == View::Form => serde_json::from_str::<Value>(r).ok().as_ref() != Some(&self.form),
            Some(r) => *r != self.text,
        }
    }

    fn read_only(&self) -> bool {
        claude::classify(&self.file).is_some_and(|(_, ro)| ro)
    }
}

enum Act {
    Select(String),
    Open(String),
    Save,
    Copy,
    OpenFile(PathBuf),
    Rescan,
    View(View),
}

/// The project tab's Claude section, for the selected checkout.
pub fn section(app: &mut App, ui: &mut egui::Ui, id: i64, root: &Path) {
    editor(app, ui, Some(id), Scope::project(root));
}

/// The sidebar's Claude page: the global `~/.claude`, or the matrix of every project's items.
pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    let scope = match claude::home(&app.store) {
        Ok(h) => Scope::global(&h),
        Err(e) => {
            ui.label(RichText::new(e.to_string()).color(p.orange));
            return;
        }
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Claude").font(theme::semibold(22.0)));
        ui.add_space(10.0);
        // Segments read left to right, so they stay out of right-to-left layouts.
        let items = [("Global".to_string(), None), ("All projects".to_string(), None)];
        if let Some(i) = theme::segmented(ui, &items, app.claude.view) {
            app.claude.view = i;
        }
        ui.add_space(6.0);
        let sub = if app.claude.view == 0 {
            scope.root.display().to_string()
        } else {
            "Which project has which instructions, settings, agents, skills and commands".into()
        };
        ui.label(RichText::new(sub).color(p.dim));
    });
    ui.add_space(14.0);
    if app.claude.view == 0 {
        egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("claude_global").show(ui, |ui| editor(app, ui, None, scope));
    } else {
        matrix(app, ui, p);
    }
}

// --- editor ---------------------------------------------------------------------------------------------------

fn editor(app: &mut App, ui: &mut egui::Ui, key: Option<i64>, scope: Scope) {
    let p = pal(ui.ctx());
    let ed = app.claude.eds.entry(key).or_insert_with(|| Ed::new(scope.clone()));
    if ed.scope != scope {
        *ed = Ed::new(scope);
    }
    let mut acts = Vec::new();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(250.0);
            item_list(ui, ed, p, &mut acts);
        });
        ui.add_space(14.0);
        ui.vertical(|ui| item_view(ui, ed, &mut app.claude.md, p, &mut acts));
    });
    for a in acts {
        let Some(ed) = app.claude.eds.get_mut(&key) else { return };
        match a {
            Act::Select(rel) => ed.select(&rel),
            Act::Open(file) => ed.open(&file),
            Act::Rescan => {
                ed.reload();
                app.claude.matrix = None;
            }
            Act::View(v) => match (ed.view, v) {
                (View::Form, _) => {
                    ed.text = claude::pretty(&ed.form);
                    ed.view = v;
                }
                (_, View::Form) => match serde_json::from_str::<Value>(&ed.text) {
                    Ok(f) if f.is_object() => {
                        ed.form = f;
                        ed.view = v;
                    }
                    _ => app.message = Some("The form needs valid JSON with an object at the top; fix it in Raw first".into()),
                },
                _ => ed.view = v,
            },
            Act::Save => match claude::plan_edit(&ed.scope, &ed.file, Some(ed.read.as_deref().unwrap_or("")), &ed.current()) {
                Ok(plan) => app.claude.pending = Some(Pending { title: format!("Save {}", ed.file), plan, copy: None, overwrite: false }),
                Err(e) => app.message = Some(format!("Not saved: {e}")),
            },
            Act::Copy => {
                let Some(it) = ed.item().cloned() else { continue };
                let source = std::fs::read_to_string(ed.scope.path(&it.rel)).unwrap_or_default();
                let mut parts = vec![(Part::Whole, if it.dir { "The whole skill folder".to_string() } else { "The whole file".to_string() })];
                if it.is_markdown() {
                    parts.extend(claude::md_sections(&source).into_iter().map(|s| {
                        let label = format!("{}{} {}", "  ".repeat(s.level - 1), "#".repeat(s.level), s.title);
                        (Part::Section(s.title), label)
                    }));
                } else if it.is_json() {
                    parts.extend(claude::json_parts(&source).into_iter().map(|(k, label)| (Part::Key(k), label)));
                }
                let target = if key.is_none() { app.projects.first().map(|q| Some(q.id)) } else { Some(None) };
                app.claude.copy =
                    Some(CopyDlg { from: ed.scope.clone(), from_key: key, rel: it.rel.clone(), to_rel: it.rel, parts, part: 0, target });
            }
            Act::OpenFile(path) => crate::project::open_at(app, &path, None),
        }
    }
}

fn glyph(k: Kind) -> &'static str {
    match k {
        Kind::Instructions => icon::FILE_TEXT,
        Kind::Settings => icon::SLIDERS,
        Kind::Agent => icon::ROBOT,
        Kind::Skill => icon::PUZZLE_PIECE,
        Kind::Command => icon::TERMINAL,
        Kind::Mcp => icon::PLUGS,
        Kind::Launch => icon::ROCKET_LAUNCH,
        Kind::Other => icon::LOCK_SIMPLE,
    }
}

fn kind_color(k: Kind, p: Palette) -> Color32 {
    match k {
        Kind::Instructions => p.blue,
        Kind::Settings => p.orange,
        Kind::Agent => p.purple,
        Kind::Skill => p.green,
        Kind::Command => p.link,
        Kind::Mcp => p.red,
        Kind::Launch => p.dim,
        Kind::Other => p.faint,
    }
}

/// Files a scope can start without copying: offered under "Create" while missing.
fn standard(global: bool) -> &'static [&'static str] {
    if global { &["CLAUDE.md", ".claude/settings.json"] } else { &["CLAUDE.md", ".claude/settings.json", ".claude/settings.local.json", ".mcp.json"] }
}

fn item_list(ui: &mut egui::Ui, ed: &Ed, p: Palette, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.label(caps(format!("{} items", ed.items.len()), p.faint));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Read the folder again").clicked() {
                acts.push(Act::Rescan);
            }
        });
    });
    for kind in Kind::ALL {
        let items: Vec<&Item> = ed.items.iter().filter(|i| i.kind == kind).collect();
        if items.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(caps(kind.label(), p.faint));
        for it in items {
            let on = ed.sel.as_deref() == Some(it.rel.as_str());
            let lock = if it.read_only { format!("\t{}", icon::LOCK_SIMPLE) } else { String::new() };
            let label = format!("{}  {}{lock}", glyph(kind), it.name);
            if theme::button(ui, &label, egui::FontId::proportional(13.5), 0.0, theme::Kind::Nav(on), vec2(w, 30.0)).on_hover_text(&it.rel).clicked()
            {
                acts.push(Act::Select(it.rel.clone()));
            }
        }
    }
    let missing: Vec<&str> = standard(ed.scope.global).iter().copied().filter(|r| !ed.items.iter().any(|i| i.rel == *r)).collect();
    if !missing.is_empty() {
        ui.add_space(12.0);
        ui.label(caps("Create", p.faint));
        for rel in missing {
            let on = ed.sel.as_deref() == Some(rel);
            if theme::button(ui, &format!("{}  {rel}", icon::PLUS), egui::FontId::proportional(13.0), 0.0, theme::Kind::Nav(on), vec2(w, 28.0))
                .clicked()
            {
                acts.push(Act::Select(rel.to_string()));
            }
        }
    }
}

fn item_view(ui: &mut egui::Ui, ed: &mut Ed, md: &mut egui_commonmark::CommonMarkCache, p: Palette, acts: &mut Vec<Act>) {
    if ed.file.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(icon::ROBOT).size(40.0).color(p.faint));
            ui.label(
                RichText::new("No Claude config here yet. Create a file on the left, or copy items here from another project or from ~/.claude.")
                    .color(p.dim),
            );
        });
        return;
    }
    let ro = ed.read_only();
    let dirty = ed.dirty() && !ro;
    let item = ed.item().cloned();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{}  {}", icon::FILE_TEXT, ed.file)).monospace().color(p.dim));
        if ed.read.is_none() {
            chip(ui, "new file", p.blue);
        } else if dirty {
            chip(ui, "unsaved", p.orange);
        }
        if ro {
            chip(ui, "read-only", p.orange).on_hover_text("Another agent's file: listed here, not edited");
        }
        let views: &[(View, &str)] = if ed.file.ends_with(".md") {
            &[(View::Text, "Edit"), (View::Preview, "Preview")]
        } else if is_settings(&ed.file) {
            &[(View::Form, "Form"), (View::Text, "Raw")]
        } else {
            &[]
        };
        if !views.is_empty() {
            let items: Vec<(String, Option<usize>)> = views.iter().map(|v| (v.1.to_string(), None)).collect();
            if let Some(i) = theme::segmented(ui, &items, views.iter().position(|v| v.0 == ed.view).unwrap_or(0)) {
                acts.push(Act::View(views[i].0));
            }
        }
        theme::right(ui, "claude_ui.rs:383", egui::Align::Center, |ui| {
            if !ro
                && ui
                    .add_enabled_ui(dirty, |ui| theme::primary(ui, format!("{}  Save…", icon::FLOPPY_DISK)))
                    .inner
                    .on_hover_text("Shows the change first")
                    .clicked()
            {
                acts.push(Act::Save);
            }
            if item.as_ref().is_some_and(|i| !i.read_only)
                && theme::secondary(ui, format!("{}  Copy to…", icon::COPY))
                    .on_hover_text("Copy this item, or a section or key of it, to another project or ~/.claude")
                    .clicked()
            {
                acts.push(Act::Copy);
            }
            if ed.read.is_some() && theme::secondary(ui, format!("{}  Open file", icon::ARROW_SQUARE_OUT)).clicked() {
                acts.push(Act::OpenFile(ed.scope.path(&ed.file)));
            }
        });
    });
    if let Some(it) = item.as_ref().filter(|i| i.dir) {
        ui.horizontal_wrapped(|ui| {
            ui.label(caps("Files", p.faint));
            for f in claude::files(&ed.scope, &it.rel) {
                let short = f[it.rel.len() + 1..].to_string();
                if theme::button(ui, &short, egui::FontId::monospace(12.0), 0.0, theme::Kind::Segment(f == ed.file), vec2(0.0, 24.0)).clicked() {
                    acts.push(Act::Open(f));
                }
            }
        });
    }
    ui.add_space(6.0);
    match ed.view {
        View::Preview => {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                egui_commonmark::CommonMarkViewer::new().show(ui, md, &ed.text);
            });
        }
        View::Form => settings_form(ui, ed, p),
        View::Text => {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::TextEdit::multiline(&mut ed.text)
                        .code_editor()
                        .frame(egui::Frame::NONE)
                        .desired_rows(26)
                        .desired_width(f32::INFINITY)
                        .interactive(!ro),
                );
            });
            if ed.file.ends_with(".json") {
                match serde_json::from_str::<Value>(&ed.text) {
                    Ok(_) => ui.label(RichText::new(format!("{}  valid JSON", icon::CHECK_CIRCLE)).size(12.0).color(p.faint)),
                    Err(e) => ui.label(RichText::new(format!("{}  {e}", icon::WARNING)).size(12.0).color(p.red)),
                };
            }
        }
    }
}

// --- settings form --------------------------------------------------------------------------------------------

/// The value at `path` (objects created on the way through nulls); `None` when something on the way is not an object.
fn slot<'a>(v: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut cur = v;
    for k in path {
        if cur.is_null() {
            *cur = Value::Object(Map::new());
        }
        cur = cur.as_object_mut()?.entry(k.to_string()).or_insert(Value::Null);
    }
    Some(cur)
}

fn push_at(v: &mut Value, path: &[&str], item: Value) {
    if let Some(s) = slot(v, path) {
        if s.is_null() {
            *s = json!([]);
        }
        if let Some(a) = s.as_array_mut() {
            a.push(item);
        }
    }
}

/// A left-aligned monospace label in a fixed-width cell.
fn key_cell(ui: &mut egui::Ui, text: &str, w: f32) {
    ui.allocate_ui_with_layout(vec2(w, 28.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_width(w);
        ui.add(egui::Label::new(RichText::new(text).monospace()).truncate());
    });
}

/// Permissions (allow, deny, ask), env and hooks as rows; other keys are left to the raw view.
fn settings_form(ui: &mut egui::Ui, ed: &mut Ed, p: Palette) {
    let (form, adds) = (&mut ed.form, &mut ed.adds);
    let fw = (ui.available_width() - 120.0).clamp(240.0, 700.0);
    let enter = |ui: &egui::Ui, r: &egui::Response| r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Permissions", p.faint));
        for (i, list) in ["allow", "deny", "ask"].into_iter().enumerate() {
            ui.add_space(4.0);
            ui.label(RichText::new(list).font(theme::semibold(13.5)));
            let mut remove = None;
            if let Some(arr) = form.pointer_mut(&format!("/permissions/{list}")).and_then(Value::as_array_mut) {
                for (k, v) in arr.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        match v {
                            Value::String(s) => ui.add(theme::field(s, fw).font(egui::TextStyle::Monospace)),
                            other => ui.label(other.to_string()),
                        };
                        if theme::icon_button(ui, icon::TRASH, "Remove this rule").clicked() {
                            remove = Some(k);
                        }
                    });
                }
                if let Some(k) = remove {
                    arr.remove(k);
                }
            }
            ui.horizontal(|ui| {
                let r = ui.add(
                    theme::field(&mut adds.rule[i], fw).hint_text(format!("Add a rule to {list}: Bash(cargo test:*), Read(./docs/**), mcp__server")),
                );
                let add = theme::icon_button(ui, icon::PLUS, "Add").clicked() || enter(ui, &r);
                if add && !adds.rule[i].trim().is_empty() {
                    push_at(form, &["permissions", list], json!(adds.rule[i].trim()));
                    adds.rule[i].clear();
                }
            });
        }
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Environment", p.faint));
        let mut remove = None;
        if let Some(env) = form.get_mut("env").and_then(Value::as_object_mut) {
            for (k, v) in env.iter_mut() {
                ui.horizontal(|ui| {
                    key_cell(ui, k, 220.0);
                    match v {
                        Value::String(s) => ui.add(theme::field(s, fw - 228.0)),
                        other => ui.label(other.to_string()),
                    };
                    if theme::icon_button(ui, icon::TRASH, "Remove this variable").clicked() {
                        remove = Some(k.clone());
                    }
                });
            }
            if let Some(k) = remove {
                env.shift_remove(&k);
            }
        }
        ui.horizontal(|ui| {
            ui.add(theme::field(&mut adds.env.0, 220.0).hint_text("NAME").font(egui::TextStyle::Monospace));
            let r = ui.add(theme::field(&mut adds.env.1, fw - 228.0).hint_text("value"));
            let add = theme::icon_button(ui, icon::PLUS, "Add").clicked() || enter(ui, &r);
            if add && !adds.env.0.trim().is_empty() {
                if let Some(s) = slot(form, &["env", adds.env.0.trim()]) {
                    *s = json!(adds.env.1);
                }
                adds.env = Default::default();
            }
        });
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Hooks", p.faint));
        let cw = (fw - 290.0).max(200.0);
        let mut remove: Option<(String, usize, usize)> = None;
        if let Some(hooks) = form.get_mut("hooks").and_then(Value::as_object_mut) {
            for (event, groups) in hooks.iter_mut() {
                let Some(groups) = groups.as_array_mut() else { continue };
                for (gi, g) in groups.iter_mut().enumerate() {
                    let before = g.get("matcher").and_then(Value::as_str).unwrap_or("").to_string();
                    let mut matcher = before.clone();
                    if let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                        for (hi, h) in list.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                key_cell(ui, if hi == 0 { event } else { "" }, 140.0);
                                if hi == 0 {
                                    ui.add(theme::field(&mut matcher, 140.0).hint_text("any tool"));
                                } else {
                                    ui.add_space(148.0);
                                }
                                match h.get_mut("command") {
                                    Some(Value::String(c)) => ui.add(theme::field(c, cw).font(egui::TextStyle::Monospace)),
                                    _ => ui.label(h.to_string()),
                                };
                                if theme::icon_button(ui, icon::TRASH, "Remove this hook").clicked() {
                                    remove = Some((event.clone(), gi, hi));
                                }
                            });
                        }
                    }
                    if matcher != before
                        && let Some(o) = g.as_object_mut()
                    {
                        if matcher.is_empty() {
                            o.shift_remove("matcher");
                        } else {
                            o.insert("matcher".into(), json!(matcher));
                        }
                    }
                }
            }
            if let Some((ev, gi, hi)) = remove
                && let Some(groups) = hooks.get_mut(&ev).and_then(Value::as_array_mut)
            {
                if let Some(list) = groups.get_mut(gi).and_then(|g| g.get_mut("hooks")).and_then(Value::as_array_mut) {
                    list.remove(hi);
                    if list.is_empty() {
                        groups.remove(gi);
                    }
                }
                if groups.is_empty() {
                    hooks.shift_remove(&ev);
                }
            }
        }
        ui.horizontal(|ui| {
            theme::select(ui, "hook_event", 140.0, EVENTS[adds.hook.0], |ui| {
                for (i, e) in EVENTS.iter().enumerate() {
                    ui.selectable_value(&mut adds.hook.0, i, *e);
                }
            });
            ui.add(theme::field(&mut adds.hook.1, 140.0).hint_text("matcher"));
            let r = ui.add(theme::field(&mut adds.hook.2, cw).hint_text("command").font(egui::TextStyle::Monospace));
            let add = theme::icon_button(ui, icon::PLUS, "Add").clicked() || enter(ui, &r);
            if add && !adds.hook.2.trim().is_empty() {
                let mut group = json!({"hooks": [{"type": "command", "command": adds.hook.2.trim()}]});
                if !adds.hook.1.trim().is_empty() {
                    group = json!({"matcher": adds.hook.1.trim(), "hooks": group["hooks"].clone()});
                }
                push_at(form, &["hooks", EVENTS[adds.hook.0]], group);
                (adds.hook.1, adds.hook.2) = Default::default();
            }
        });
    });
    let others: Vec<String> =
        form.as_object().map(|o| o.keys().filter(|k| !matches!(k.as_str(), "permissions" | "env" | "hooks")).cloned().collect()).unwrap_or_default();
    if !others.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Also in this file: {}. Edit those in Raw.", others.join(", "))).size(12.5).color(p.dim));
    }
}

// --- copy and review windows ----------------------------------------------------------------------------------

struct CopyDlg {
    from: Scope,
    from_key: Option<i64>,
    rel: String,
    to_rel: String,
    parts: Vec<(Part, String)>,
    part: usize,
    /// `Some(None)` = the global `~/.claude`.
    target: Option<Option<i64>>,
}

/// A copy, kept so the review can plan it again with overwrite.
struct CopyReq {
    from: Scope,
    rel: String,
    part: Part,
    to: Scope,
    to_rel: String,
}

struct Pending {
    title: String,
    plan: Plan,
    copy: Option<CopyReq>,
    overwrite: bool,
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    copy_window(app, ctx);
    review_window(app, ctx);
}

fn copy_window(app: &mut App, ctx: &egui::Context) {
    let p = pal(ctx);
    let projects: Vec<(i64, String)> = app.projects.iter().map(|q| (q.id, q.name.clone())).collect();
    let Some(c) = app.claude.copy.as_mut() else { return };
    let (mut open, mut go) = (true, false);
    let from_name = match c.from_key {
        None => "the global ~/.claude".to_string(),
        Some(id) => projects.iter().find(|q| q.0 == id).map_or("?".into(), |q| q.1.clone()),
    };
    egui::Window::new(format!("Copy · {}", c.rel)).id(egui::Id::new("claude_copy")).open(&mut open).collapsible(false).default_width(620.0).show(
        ctx,
        |ui| {
            ui.label(RichText::new(format!("From {from_name}: {}", c.rel)).color(p.dim));
            ui.add_space(8.0);
            egui::Grid::new("claude_copy_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("To").color(p.dim));
                let name = |t: Option<i64>| {
                    t.map_or("Global ~/.claude".to_string(), |id| projects.iter().find(|q| q.0 == id).map_or("?".into(), |q| q.1.clone()))
                };
                theme::select(ui, "claude_copy_to", 420.0, c.target.map_or("Choose…".to_string(), name), |ui| {
                    if c.from_key.is_some() {
                        ui.selectable_value(&mut c.target, Some(None), "Global ~/.claude");
                    }
                    for (id, n) in projects.iter().filter(|q| Some(q.0) != c.from_key) {
                        ui.selectable_value(&mut c.target, Some(Some(*id)), n);
                    }
                });
                ui.end_row();
                ui.label(RichText::new("What").color(p.dim));
                theme::select(ui, "claude_copy_part", 420.0, c.parts[c.part].1.clone(), |ui| {
                    for (i, (_, label)) in c.parts.iter().enumerate() {
                        ui.selectable_value(&mut c.part, i, label);
                    }
                });
                ui.end_row();
                ui.label(RichText::new("As").color(p.dim));
                ui.horizontal(|ui| {
                    ui.add(theme::field(&mut c.to_rel, 300.0).font(egui::TextStyle::Monospace));
                    if c.rel.contains("settings") {
                        for alt in [".claude/settings.json", ".claude/settings.local.json"] {
                            if alt != c.to_rel && theme::ghost(ui, alt.trim_start_matches(".claude/")).clicked() {
                                c.to_rel = alt.to_string();
                            }
                        }
                    }
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            let how = match &c.parts[c.part].0 {
                Part::Whole => "A file that exists in the target is only replaced if you say so in the review.",
                Part::Section(_) => "The section is added at the end, or replaces the section with the same heading if you say so.",
                Part::Key(_) => "The key is merged in: objects by key, lists gain what they lack; different values are kept unless you say so.",
            };
            ui.label(RichText::new(format!("{how} Copies the saved file; a plain copy, not linked to its source.")).size(12.5).color(p.dim));
            ui.add_space(8.0);
            go = ui.add_enabled_ui(c.target.is_some(), |ui| theme::primary(ui, "Review…")).inner.clicked();
        },
    );
    if !open {
        app.claude.copy = None;
        return;
    }
    if !go {
        return;
    }
    let Some(c) = app.claude.copy.take() else { return };
    let to_name = match c.target.flatten() {
        None => "Global ~/.claude".to_string(),
        Some(id) => projects.iter().find(|q| q.0 == id).map_or("?".into(), |q| q.1.clone()),
    };
    let to = match c.target.flatten() {
        None => claude::home(&app.store).map(|h| Scope::global(&h)),
        Some(id) => app
            .projects
            .iter()
            .find(|q| q.id == id)
            .ok_or_else(|| pa_core::ops::Error("no such project".into()))
            .and_then(pa_core::ops::primary_root)
            .map(|r| Scope::project(&r)),
    };
    let req = match to {
        Ok(to) => CopyReq { from: c.from, rel: c.rel, part: c.parts[c.part].0.clone(), to, to_rel: c.to_rel.trim().to_string() },
        Err(e) => {
            app.message = Some(format!("Not copied: {e}"));
            return;
        }
    };
    match claude::plan_copy(&req.from, &req.rel, &req.part, &req.to, Some(&req.to_rel), false) {
        Ok(plan) => app.claude.pending = Some(Pending { title: format!("Copy {} to {to_name}", req.rel), plan, copy: Some(req), overwrite: false }),
        Err(e) => app.message = Some(format!("Not copied: {e}")),
    }
}

/// A unified diff as one coloured monospace text.
fn diff_text(text: &str, p: Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::monospace(12.0);
    for line in text.lines() {
        let color = match line.get(..2) {
            Some("+ ") => p.green,
            Some("- ") => p.red,
            _ if line == "…" => p.faint,
            _ => p.dim,
        };
        job.append(&format!("{line}\n"), 0.0, egui::TextFormat::simple(font.clone(), color));
    }
    job
}

fn review_window(app: &mut App, ctx: &egui::Context) {
    let p = pal(ctx);
    let backups = app.data_dir.join("backups");
    let Some(pd) = app.claude.pending.as_mut() else { return };
    let (mut open, mut apply, mut replan, mut cancel) = (true, false, false, false);
    egui::Window::new(format!("Review · {}", pd.title))
        .id(egui::Id::new("claude_review"))
        .open(&mut open)
        .collapsible(false)
        .default_size([940.0, 660.0])
        .max_width(1200.0)
        .show(ctx, |ui| {
            if pd.plan.changes.is_empty() {
                let why = if pd.plan.conflicts.is_empty() {
                    "Nothing to change: the target already reads like this."
                } else {
                    "Nothing to write unless you replace what differs (below)."
                };
                ui.label(RichText::new(why).color(p.dim));
            }
            egui::ScrollArea::both().max_height(480.0).show(ui, |ui| {
                for c in &pd.plan.changes {
                    ui.horizontal(|ui| {
                        if c.existed {
                            chip(ui, "changed", p.orange)
                        } else {
                            chip(ui, "new file", p.blue)
                        };
                        ui.add(egui::Label::new(RichText::new(c.path.display().to_string()).monospace()).truncate());
                    });
                    card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(diff_text(&c.diff(), p)).extend());
                    });
                    ui.add_space(8.0);
                }
            });
            if !pd.plan.conflicts.is_empty() {
                egui::Frame::new()
                    .fill(p.orange.gamma_multiply(0.08))
                    .stroke(egui::Stroke::new(1.0, p.orange.gamma_multiply(0.4)))
                    .corner_radius(8)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(caps(if pd.overwrite { "Replaced in the target" } else { "Kept as they are in the target" }, p.orange));
                        for x in &pd.plan.conflicts {
                            ui.label(RichText::new(x).monospace().size(12.0));
                        }
                        if pd.copy.is_some() && theme::checkbox(ui, &mut pd.overwrite, "Replace them with the source's").changed() {
                            replan = true;
                        }
                    });
            }
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Each replaced file is backed up to {} first.", backups.display())).size(12.5).color(p.dim));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                apply = ui.add_enabled_ui(!pd.plan.changes.is_empty(), |ui| theme::primary(ui, "Apply")).inner.clicked();
                cancel = theme::ghost(ui, "Cancel").clicked();
            });
        });
    if replan && let Some(r) = &pd.copy {
        match claude::plan_copy(&r.from, &r.rel, &r.part, &r.to, Some(&r.to_rel), pd.overwrite) {
            Ok(plan) => pd.plan = plan,
            Err(e) => app.message = Some(e.to_string()),
        }
    }
    if !open || cancel {
        app.claude.pending = None;
        return;
    }
    if !apply {
        return;
    }
    let Some(pd) = app.claude.pending.take() else { return };
    match claude::apply(&app.data_dir, &pd.plan) {
        Ok(b) => {
            let changed: Vec<&PathBuf> = pd.plan.changes.iter().map(|c| &c.path).collect();
            for ed in app.claude.eds.values_mut() {
                if changed.contains(&&ed.scope.path(&ed.file)) {
                    let f = ed.file.clone();
                    ed.open(&f);
                }
                ed.reload();
            }
            app.claude.matrix = None;
            let backed = if b.is_empty() { String::new() } else { format!("; {} backup(s) in the app's backups folder", b.len()) };
            app.message = Some(format!("Wrote {} file(s){backed}", pd.plan.changes.len()));
        }
        Err(e) => app.message = Some(format!("Not written: {e}")),
    }
}

// --- matrix ---------------------------------------------------------------------------------------------------

struct Matrix {
    rx: Option<Receiver<Vec<Row>>>,
    rows: Vec<Row>,
}

#[derive(Clone)]
struct Row {
    /// `None` = the global `~/.claude`.
    id: Option<i64>,
    name: String,
    items: Vec<Item>,
}

fn matrix(app: &mut App, ui: &mut egui::Ui, p: Palette) {
    if app.claude.matrix.is_none() {
        let mut jobs: Vec<(Option<i64>, String, Scope)> = Vec::new();
        if let Ok(h) = claude::home(&app.store) {
            jobs.push((None, "Global ~/.claude".into(), Scope::global(&h)));
        }
        jobs.extend(app.projects.iter().filter_map(|q| q.primary().map(|c| (Some(q.id), q.name.clone(), Scope::project(Path::new(&c.path))))));
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let rows = jobs.into_iter().map(|(id, name, s)| Row { id, name, items: claude::inventory(&s) }).collect();
            let _ = tx.send(rows);
            ctx.request_repaint();
        });
        app.claude.matrix = Some(Matrix { rx: Some(rx), rows: Vec::new() });
    }
    let Some(m) = app.claude.matrix.as_mut() else { return };
    if let Some(rx) = &m.rx
        && let Ok(rows) = rx.try_recv()
    {
        m.rows = rows;
        m.rx = None;
    }
    let loading = m.rx.is_some();
    let rows = m.rows.clone();
    let mut cols: BTreeSet<(Kind, String)> = BTreeSet::new();
    for r in &rows {
        cols.extend(r.items.iter().filter(|i| app.claude.filter.is_none_or(|k| k == i.kind)).map(|i| (i.kind, i.column())));
    }
    let cols: Vec<(Kind, String)> = cols.into_iter().collect();
    let mut rescan = false;
    ui.horizontal_wrapped(|ui| {
        let kinds: Vec<Kind> = Kind::ALL.into_iter().filter(|k| rows.iter().any(|r| r.items.iter().any(|i| i.kind == *k))).collect();
        let mut items = vec![("All".to_string(), None)];
        items.extend(kinds.iter().map(|k| (k.label().to_string(), None)));
        let sel = app.claude.filter.and_then(|f| kinds.iter().position(|k| *k == f)).map_or(0, |i| i + 1);
        if let Some(i) = theme::segmented(ui, &items, sel) {
            app.claude.filter = if i == 0 { None } else { Some(kinds[i - 1]) };
        }
        theme::right(ui, "claude_ui.rs:932", egui::Align::Center, |ui| {
            if loading {
                ui.spinner();
                ui.label(RichText::new("Reading every project's Claude config…").color(p.dim));
            } else {
                rescan = theme::secondary(ui, format!("{}  Rescan", icon::ARROWS_CLOCKWISE)).clicked();
                ui.label(caps(format!("{} rows · {} columns", rows.len(), cols.len()), p.faint));
            }
        });
    });
    if rescan {
        app.claude.matrix = None;
        return;
    }
    ui.add_space(10.0);
    let (name_w, cw, rh) = (230.0, 26.0, 26.0);
    let font = egui::FontId::proportional(12.5);
    let galleys: Vec<_> = cols.iter().map(|(_, c)| ui.painter().layout_no_wrap(c.clone(), font.clone(), p.dim)).collect();
    let head_h = galleys.iter().map(|g| g.size().x).fold(40.0, f32::max).min(220.0) + 12.0;
    let width = name_w + cw * cols.len() as f32 + 8.0;
    let mut open: Option<Option<i64>> = None;
    egui::ScrollArea::both().auto_shrink([false, false]).id_salt("claude_matrix").show(ui, |ui| {
        let (head, _) = ui.allocate_exact_size(vec2(width, head_h), Sense::hover());
        for (i, g) in galleys.into_iter().enumerate() {
            let x = head.left() + name_w + cw * i as f32 + cw / 2.0 - g.size().y / 2.0;
            ui.painter().add(egui::epaint::TextShape::new(egui::pos2(x, head.bottom() - 6.0), g, p.dim).with_angle(-std::f32::consts::FRAC_PI_2));
        }
        ui.painter().hline(head.left()..=head.right(), head.bottom(), egui::Stroke::new(1.0, p.border));
        for (ri, row) in rows.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(width, rh), Sense::click());
            let painter = ui.painter();
            let hover = resp.hovered();
            if hover {
                painter.rect_filled(r, 4, p.hover);
            } else if ri % 2 == 1 {
                painter.rect_filled(r, 0, p.surface);
            }
            let name_rect = egui::Rect::from_min_size(r.min, vec2(name_w - 8.0, rh));
            let color = if row.id.is_none() { p.blue } else { p.text };
            ui.painter_at(name_rect).text(
                name_rect.left_center() + vec2(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::proportional(13.5),
                color,
            );
            let mut tip = None;
            for (ci, (kind, col)) in cols.iter().enumerate() {
                let cell = egui::Rect::from_min_size(egui::pos2(r.left() + name_w + cw * ci as f32, r.top()), vec2(cw, rh));
                let n = row.items.iter().filter(|i| i.kind == *kind && i.column() == *col).count();
                if n > 0 {
                    ui.painter().circle_filled(cell.center(), 5.0, kind_color(*kind, p));
                    if n > 1 {
                        ui.painter().text(
                            cell.center() + vec2(7.0, -6.0),
                            egui::Align2::LEFT_CENTER,
                            n.to_string(),
                            egui::FontId::monospace(9.5),
                            p.dim,
                        );
                    }
                }
                if hover && resp.hover_pos().is_some_and(|q| cell.contains(q)) {
                    tip = Some(format!("{} · {col}: {}", row.name, if n > 0 { "yes" } else { "no" }));
                }
            }
            let resp = match tip {
                Some(t) => resp.on_hover_text(t),
                None => resp.on_hover_text(format!("{}: open its Claude config", row.name)),
            };
            if resp.clicked() {
                open = Some(row.id);
            }
        }
        if rows.is_empty() && !loading {
            ui.label(RichText::new("No projects yet.").color(p.dim));
        }
    });
    match open {
        Some(Some(id)) => {
            app.project.set_section(id, "claude");
            app.open.push(Tab::Project(id));
        }
        Some(None) => app.claude.view = 0,
        None => {}
    }
}
