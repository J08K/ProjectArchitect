//! A project's tab (SPEC § 7): header with launchers, then sections: Overview (the dashboard), TODOs (the TODO file
//! and TODOs in code), Roadmap, Notes, Git (repository, GitHub, checkouts), Tasks, Deps, Claude and Settings. TODO.md and ROADMAP.md are the truth (D-1): read from disk, edited losslessly through
//! `pa_core::todo` / `roadmap`, written back atomically after a re-read check, reloaded when changed outside.

use crate::overview::{purpose_color, short_path};
use crate::theme::{self, Palette, avatar, caps, card, chip, pal, relative, tag};
use crate::widgets::{ahead_behind, group_combo, purpose_combo, status_combo};
use crate::{App, LAUNCHERS};
use egui::{Align, Color32, Layout, RichText, Sense, Stroke, vec2};
use egui_phosphor::regular as icon;
use pa_core::codetodo::CodeTodo;
use pa_core::roadmap::{self, MilestoneStatus, Roadmap};
use pa_core::todo::{self, SaveError, TodoDoc, TodoItem};
use pa_core::loc::{self, LocRow};
use pa_core::github::{self, Issue};
use pa_core::{Note, Project, info::github_url};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Overview,
    Todos,
    Roadmap,
    CodeTodos,
    Notes,
    Checkouts,
    Settings,
    Git,
    GitHub,
    Tasks,
    Dependencies,
    Claude,
}

/// A TODO or roadmap file as read from disk: the exact text (for the re-read check) and when it was checked.
pub(crate) struct Loaded<T> {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
    pub(crate) doc: T,
    mtime: Option<SystemTime>,
    checked: Instant,
}

/// Lines of code: being counted on a background thread, or counted.
enum Loc {
    Counting(std::sync::mpsc::Receiver<Vec<LocRow>>),
    Done(Vec<LocRow>),
}

#[derive(Default)]
pub struct State {
    section: HashMap<i64, Section>,
    tags: HashMap<i64, String>,
    selected: HashMap<i64, i64>,
    confirm_remove: Option<i64>,
    pub(crate) todos: HashMap<i64, Loaded<TodoDoc>>,
    pub(crate) roadmaps: HashMap<i64, Loaded<Roadmap>>,
    code: HashMap<i64, Vec<CodeTodo>>,
    notes: HashMap<i64, Vec<Note>>,
    note: HashMap<i64, i64>,
    note_preview: bool,
    /// "New item" text per (project, section).
    pub(crate) new_item: HashMap<(i64, String), String>,
    /// Item being edited: (project, item id, text).
    editing: Option<(i64, String, String)>,
    show_done: bool,
    md: egui_commonmark::CommonMarkCache,
    loc: HashMap<i64, Loc>,
    /// Issues of a project's GitHub repository by number, for the items linked to them; `None` while loading.
    issues: HashMap<i64, Option<HashMap<u64, Issue>>>,
    /// A linked item was ticked: offer to close its issue (project, issue number).
    close_offer: Option<(i64, u64)>,
}

impl State {
    /// Drop cached notes (they are re-read from the store on next view), e.g. after the MCP server wrote some.
    pub fn forget_notes(&mut self) {
        self.notes.clear();
    }

    /// Pick a section by name (from `--section`).
    pub fn set_section(&mut self, id: i64, name: &str) {
        let s = match name {
            "todos" => Section::Todos,
            "roadmap" => Section::Roadmap,
            "code" => Section::CodeTodos,
            "notes" => Section::Notes,
            "checkouts" => Section::Checkouts,
            "settings" => Section::Settings,
            "git" => Section::Git,
            "github" => Section::GitHub,
            "tasks" => Section::Tasks,
            "deps" => Section::Dependencies,
            "claude" => Section::Claude,
            _ => Section::Overview,
        };
        self.section.insert(id, s);
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, id: i64) {
    let p = pal(ui.ctx());
    let Some(mut q) = app.projects.iter().find(|q| q.id == id).cloned() else {
        ui.label("This project was removed from the app.");
        return;
    };
    let selected = app.project.selected.get(&id).copied().filter(|s| q.checkouts.iter().any(|c| c.id == *s));
    let Some(checkout) = selected.and_then(|s| q.checkouts.iter().find(|c| c.id == s)).or(q.primary()).cloned() else {
        ui.label("This project has no checkouts.");
        return;
    };
    let root = PathBuf::from(&checkout.path);
    header(app, ui, &mut q, &checkout.path, p);
    ui.add_space(14.0);

    let purpose = app.purpose(q.purpose_id).cloned();
    let track = purpose.as_ref().is_none_or(|u| u.track_todos);
    let scan_code = purpose.as_ref().is_none_or(|u| u.scan_code_todos);
    let info = checkout.info.clone();
    let todo_n = info.as_ref().and_then(|i| i.todo_open);
    let code_n = info.as_ref().map_or(0, |i| i.code_todos);
    // Sections in groups: related ones share a tab and switch with a second bar (TODO file / in code; repository,
    // GitHub, checkouts).
    let dirty = info.as_ref().and_then(|i| i.git.as_ref()).map(|g| g.dirty as usize).filter(|n| *n > 0);
    let one = |s: Section, label: &str, n: Option<usize>| (label.to_string(), n, vec![(s, label.to_string(), n)]);
    let mut groups = vec![one(Section::Overview, "Overview", None)];
    let mut todo_leaves = Vec::new();
    if track {
        todo_leaves.push((Section::Todos, "TODO file".to_string(), todo_n.map(|n| n as usize)));
    }
    if scan_code {
        todo_leaves.push((Section::CodeTodos, "In code".to_string(), Some(code_n as usize)));
    }
    if !todo_leaves.is_empty() {
        let n = if track { todo_n.map(|n| n as usize) } else { Some(code_n as usize) };
        groups.push(("TODOs".into(), n, todo_leaves));
    }
    if track {
        groups.push(one(Section::Roadmap, "Roadmap", None));
    }
    groups.push(one(Section::Notes, "Notes", Some(notes(app, id).len())));
    groups.push((
        "Git".into(),
        dirty,
        vec![
            (Section::Git, "Repository".into(), dirty),
            (Section::GitHub, "GitHub".into(), None),
            (Section::Checkouts, "Checkouts".into(), Some(q.checkouts.len())),
        ],
    ));
    groups.push(one(Section::Tasks, if app.runs.running(id) > 0 { "Tasks ▶" } else { "Tasks" }, None));
    groups.push(one(Section::Dependencies, "Deps", None));
    groups.push(one(Section::Claude, "Claude", None));
    groups.push(one(Section::Settings, "Settings", None));
    let find = |s: Section| groups.iter().position(|g| g.2.iter().any(|l| l.0 == s));
    let current = app.project.section.get(&id).copied().filter(|s| find(*s).is_some()).unwrap_or_default();
    let gi = find(current).unwrap_or(0);
    let items: Vec<(String, Option<usize>)> = groups.iter().map(|g| (g.0.clone(), g.1)).collect();
    if let Some(i) = theme::tabs(ui, &items, gi)
        && i != gi
    {
        app.project.section.insert(id, groups[i].2[0].0);
    }
    ui.add_space(14.0);
    let leaves = &groups[gi].2;
    if leaves.len() > 1 {
        let items: Vec<(String, Option<usize>)> = leaves.iter().map(|l| (l.1.clone(), l.2)).collect();
        if let Some(i) = theme::segmented(ui, &items, leaves.iter().position(|l| l.0 == current).unwrap_or(0)) {
            app.project.section.insert(id, leaves[i].0);
        }
        ui.add_space(12.0);
    }

    if current == Section::Overview {
        overview(app, ui, &q, &root);
        app.save_project(q);
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("project", id)).show(ui, |ui| match current {
        Section::Overview => unreachable!("drawn outside the scroll area"),
        Section::Todos => todos(app, ui, id, &q, &root, p),
        Section::Roadmap => crate::roadmap_ui::show(app, ui, id, &q, &root, p),
        Section::CodeTodos => code_todos(app, ui, id, &q, &root, p),
        Section::Notes => notes_view(app, ui, id, p),
        Section::Checkouts => checkouts(app, ui, id, checkout.id, p),
        Section::Settings => settings(app, ui, &mut q, p),
        Section::Git => crate::git_ui::git(app, ui, &q, &root, p),
        Section::GitHub => crate::git_ui::github(app, ui, &q, &root, p),
        Section::Tasks => crate::tasks_ui::show(app, ui, &q, &root, p),
        Section::Dependencies => crate::deps_ui::show(app, ui, &q, p),
        Section::Claude => crate::claude_ui::section(app, ui, id, &root),
    });
    app.save_project(q);
}

// --- header ---------------------------------------------------------------------------------------------------

fn header(app: &mut App, ui: &mut egui::Ui, q: &mut Project, path: &str, p: Palette) {
    let info = q.info().cloned();
    let git = info.as_ref().and_then(|i| i.git.clone());
    ui.horizontal_wrapped(|ui| {
        let color = info.as_ref().and_then(|i| i.languages.first()).map_or_else(|| theme::hash_color(&q.name), |l| theme::lang_color(&l.0));
        // Compact: name, version and chips on one row, the description (else the folder) under it.
        avatar(ui, &q.name, color, 40.0);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                let w = ui.painter().layout_no_wrap(q.name.clone(), theme::semibold(20.0), p.text).size().x + 16.0;
                ui.add(egui::TextEdit::singleline(&mut q.name).font(theme::semibold(20.0)).frame(egui::Frame::NONE).desired_width(w.min(600.0)));
                if let Some(v) = info.as_ref().and_then(|i| i.version.as_deref()) {
                    ui.label(RichText::new(crate::widgets::version(v)).monospace().size(13.0).color(p.dim));
                }
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                if let Some(u) = app.purpose(q.purpose_id) {
                    chip(ui, &u.name, purpose_color(app, q.purpose_id, p));
                }
                chip(ui, q.status.as_str(), crate::overview::status_color(q.status, p));
                if let Some(g) = app.groups.iter().find(|g| Some(g.id) == q.group_id) {
                    tag(ui, format!("{} {}", icon::FOLDERS, g.name), p.dim);
                }
                for t in &q.tags {
                    tag(ui, t, theme::hash_color(t));
                }
            });
            let sub = info.as_ref().and_then(|i| i.description.clone()).unwrap_or_else(|| short_path(path));
            ui.label(RichText::new(sub).size(12.5).color(p.dim));
        });
        theme::right(ui, "project.rs:233", egui::Align::Min, |ui| {
            for &(key, label, default) in LAUNCHERS[1..].iter().rev() {
                let glyph = match key {
                    "launch.explorer" => icon::FOLDER_OPEN,
                    "launch.terminal" => icon::TERMINAL_WINDOW,
                    _ => icon::ROBOT,
                };
                if theme::secondary(ui, format!("{glyph}  {label}")).on_hover_text(format!("Open in {label}")).clicked() {
                    app.launch(key, default, path);
                }
            }
            let (key, _, default) = LAUNCHERS[0];
            if theme::primary(ui, format!("{}  Open in VS Code", icon::CODE)).clicked() {
                app.launch(key, default, path);
            }
            if let Some(url) = git.as_ref().and_then(|g| g.remote.as_deref()).and_then(github_url) {
                ui.hyperlink_to(RichText::new(format!("{}  GitHub", icon::GITHUB_LOGO)), url);
            }
        });
    });
}

// --- overview -------------------------------------------------------------------------------------------------

/// The Overview section: the project's dashboard (`dashboard_ui`).
fn overview(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path) {
    crate::dashboard_ui::show(app, ui, q, root);
}

/// Branch, upstream, last commit, remote, tag and version.
pub(crate) fn repo_card(ui: &mut egui::Ui, q: &Project, p: Palette) {
    let info = q.info().cloned().unwrap_or_default();
    let row = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(k).color(p.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(v));
        });
    };
    match &info.git {
        Some(g) => {
            row(ui, "Branch", g.branch.clone().unwrap_or("(detached)".into()));
            row(ui, "Changed files", g.dirty.to_string());
            let ab = ahead_behind(g.ahead_behind);
            row(ui, "Upstream", if g.ahead_behind.is_none() { "none".into() } else if ab.is_empty() { "up to date".into() } else { ab });
            row(ui, "Last commit", g.last_commit.as_deref().map(|d| format!("{} ({})", relative(d), &d[..10.min(d.len())])).unwrap_or("—".into()));
            row(ui, "Remote", g.remote.clone().unwrap_or("local only".into()));
            row(ui, "Latest tag", g.latest_tag.clone().unwrap_or("—".into()));
        }
        None => row(ui, "Git", "not a repository".into()),
    }
    row(ui, "Version", info.version.as_deref().map_or("—".into(), crate::widgets::version));
}

/// The first open TODO items.
pub(crate) fn todos_card(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    load_todo(app, q, root);
    match app.project.todos.get(&q.id) {
        Some(l) => {
            let open: Vec<TodoItem> = l.doc.items.iter().filter(|i| !i.done && i.depth == 0).take(6).cloned().collect();
            if open.is_empty() {
                ui.label(RichText::new("Nothing open.").color(p.dim));
            }
            for it in open {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SQUARE).color(p.faint));
                    ui.add(egui::Label::new(theme::plain_md(&first_line(&it.text))).truncate());
                });
            }
            ui.add_space(4.0);
            if theme::ghost(ui, format!("View all  {}", icon::ARROW_RIGHT)).clicked() {
                app.project.section.insert(q.id, Section::Todos);
            }
        }
        None => {
            ui.label(RichText::new("No TODO file.").color(p.dim));
        }
    }
}

/// The last finished roadmap entry and the next ones.
pub(crate) fn roadmap_card(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    load_roadmap(app, q, root);
    let mut open_roadmap = false;
    match app.project.roadmaps.get(&q.id) {
        Some(l) if !l.doc.editable => {
            ui.label(RichText::new("This roadmap uses another layout (a table); see the Roadmap tab.").color(p.dim));
        }
        Some(l) if !l.doc.milestones.is_empty() => {
            // Versions and milestones in file order (plain sections are not entries).
            let all: Vec<&roadmap::Milestone> = l.doc.milestones.iter().collect();
            let finished = |m: &&roadmap::Milestone| matches!(m.status, MilestoneStatus::Done | MilestoneStatus::Released);
            let last_done = all.iter().rev().find(|m| finished(m)).copied();
            let upcoming: Vec<&roadmap::Milestone> = all.iter().filter(|m| !finished(m)).copied().collect();
            let shown: Vec<&roadmap::Milestone> = last_done.into_iter().chain(upcoming.iter().take(TIMELINE_MAX).copied()).collect();
            for (i, m) in shown.iter().enumerate() {
                timeline_entry(ui, m, i + 1 == shown.len(), false, None, p);
            }
            if upcoming.is_empty() {
                ui.label(RichText::new("All milestones done.").color(p.dim));
            }
            ui.add_space(4.0);
            let more = upcoming.len().saturating_sub(TIMELINE_MAX);
            let label = if more > 0 { format!("Show {more} more  {}", icon::ARROW_RIGHT) } else { format!("Open roadmap  {}", icon::ARROW_RIGHT) };
            open_roadmap = theme::ghost(ui, label).clicked();
        }
        _ => {
            ui.label(RichText::new("No roadmap.").color(p.dim));
        }
    }
    if open_roadmap {
        app.project.section.insert(q.id, Section::Roadmap);
    }
}

/// Languages and lines of code in one card. Lines come from tokei, counted once per session on a background
/// thread ("Recount" redoes it); until then the byte shares from the last refresh are shown.
pub(crate) fn languages_card(app: &mut App, ui: &mut egui::Ui, id: i64, root: &Path, bytes: &[(String, u64)], p: Palette) {
    let mut recount = false;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match app.project.loc.get(&id) {
            Some(Loc::Done(_)) => {
                if theme::ghost(ui, format!("{}  Recount", icon::ARROWS_CLOCKWISE)).on_hover_text("Count the lines of code again").clicked() {
                    recount = true;
                }
            }
            _ => {
                ui.label(RichText::new("Counting lines…").size(12.5).color(p.faint));
                ui.spinner();
            }
        });
    });
    ui.add_space(6.0);
    if recount || !app.project.loc.contains_key(&id) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, ctx) = (root.to_path_buf(), ui.ctx().clone());
        std::thread::spawn(move || {
            let _ = tx.send(loc::count(&root));
            ctx.request_repaint();
        });
        app.project.loc.insert(id, Loc::Counting(rx));
    }
    if let Some(Loc::Counting(rx)) = app.project.loc.get(&id)
        && let Ok(rows) = rx.try_recv()
    {
        app.project.loc.insert(id, Loc::Done(rows));
    }
    let rows: Option<Vec<LocRow>> = match app.project.loc.get(&id) {
        Some(Loc::Done(rows)) => Some(rows.clone()),
        _ => None,
    };
    let width = ui.available_width();
    match rows {
        Some(rows) if !rows.is_empty() => {
            let shares: Vec<(String, u64)> = rows.iter().map(|r| (r.language.clone(), r.code as u64)).collect();
            theme::lang_bar(ui, &shares, width);
            ui.add_space(8.0);
            loc_table(ui, &rows, width, p);
        }
        _ => {
            theme::lang_bar(ui, bytes, width);
            ui.add_space(8.0);
            if bytes.is_empty() {
                ui.label(RichText::new("No source files found.").color(p.dim));
            }
            let total: u64 = bytes.iter().map(|l| l.1).sum::<u64>().max(1);
            for (l, n) in bytes.iter().take(6) {
                table_row(ui, width, 26.0, &mut |ui| theme::lang_dot(ui, l), &[format!("{}%", (n * 100 + total / 2) / total)], &[60.0], false, p);
            }
        }
    }
}

/// Fixed numeric columns (right-aligned) after a flexible name column, so headers, rows and the total line up.
const LOC_COLS: [(&str, f32); 5] = [("Share", 64.0), ("Files", 64.0), ("Code", 84.0), ("Comments", 92.0), ("Blanks", 76.0)];

fn loc_table(ui: &mut egui::Ui, rows: &[LocRow], width: f32, p: Palette) {
    let total = loc::total(rows);
    let widths: Vec<f32> = LOC_COLS.iter().map(|c| c.1).collect();
    let name_w = width - widths.iter().sum::<f32>();
    // Header
    let (rect, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, "LANGUAGE", egui::FontId::monospace(10.5), p.faint);
    let mut x = rect.left() + name_w;
    for (h, w) in LOC_COLS {
        ui.painter().text(egui::pos2(x + w, rect.center().y), egui::Align2::RIGHT_CENTER, h.to_uppercase(), egui::FontId::monospace(10.5), p.faint);
        x += w;
    }
    ui.painter().hline(rect.x_range(), rect.bottom() + 2.0, Stroke::new(1.0, p.border));
    ui.add_space(4.0);
    let shown = 8;
    let pct = |n: usize| if total.code == 0 { "—".to_string() } else { format!("{:.1}%", n as f64 * 100.0 / total.code as f64) };
    for r in rows.iter().take(shown) {
        let cells = [pct(r.code), group_digits(r.files), group_digits(r.code), group_digits(r.comments), group_digits(r.blanks)];
        table_row(ui, width, 26.0, &mut |ui| theme::lang_dot(ui, &r.language), &cells, &widths, false, p);
    }
    if rows.len() > shown {
        let rest = loc::total(&rows[shown..]);
        let cells = [pct(rest.code), group_digits(rest.files), group_digits(rest.code), group_digits(rest.comments), group_digits(rest.blanks)];
        let label = format!("{} more languages", rows.len() - shown);
        table_row(ui, width, 26.0, &mut |ui| { ui.label(RichText::new(&label).color(p.faint)); }, &cells, &widths, false, p);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.border));
    let cells = ["100%".to_string(), group_digits(total.files), group_digits(total.code), group_digits(total.comments), group_digits(total.blanks)];
    table_row(ui, width, 28.0, &mut |ui| { ui.label(RichText::new("Total").font(theme::semibold(14.0))); }, &cells, &widths, true, p);
}

/// One table row: `name` drawn in the flexible first column, then `cells` right-aligned in columns of `widths`
/// (the last columns of the table, so a row with fewer cells lines up with the rightmost columns).
fn table_row(ui: &mut egui::Ui, width: f32, h: f32, name: &mut dyn FnMut(&mut egui::Ui), cells: &[String], widths: &[f32], strong: bool, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let cols: f32 = widths.iter().sum();
    let name_rect = egui::Rect::from_min_size(rect.min, vec2(width - cols, h));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect).layout(Layout::left_to_right(Align::Center)));
    name(&mut child);
    let mut x = rect.right() - cols;
    for (text, w) in cells.iter().zip(widths) {
        let font = egui::FontId::monospace(12.5);
        let color = if strong { p.text } else { p.dim };
        ui.painter().text(egui::pos2(x + w, rect.center().y), egui::Align2::RIGHT_CENTER, text, font, color);
        x += w;
    }
}

/// 63855 → "63 855".
fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Upcoming milestones shown on the overview's timeline (plus the last finished one).
const TIMELINE_MAX: usize = 5;

/// One timeline entry: a dot on a vertical line (green = done, blue = in progress, hollow = planned), the title and
/// status, then progress and target. The finished entry is drawn faded. With `fold`, a caret before the progress
/// unfolds the entry's items (state kept under that id).
pub(crate) fn timeline_entry(ui: &mut egui::Ui, m: &roadmap::Milestone, last: bool, body: bool, fold: Option<egui::Id>, p: Palette) {
    let color = status_color(&m.status, p);
    let done = matches!(m.status, MilestoneStatus::Done | MilestoneStatus::Released);
    let gutter = 22.0;
    let r = ui.horizontal_top(|ui| {
        ui.allocate_exact_size(vec2(gutter, 18.0), Sense::hover());
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                // Chips first, from the right, so a long title truncates instead of pushing the status out.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    status_hover(chip(ui, status_label(&m.status), color), m);
                    if let Some(w) = &m.warning {
                        chip(ui, "check", p.orange).on_hover_text(w);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if let Some(v) = &m.version {
                            tag(ui, v, p.blue);
                        }
                        let title = RichText::new(&m.title).font(theme::semibold(14.5)).color(if done { p.dim } else { p.text });
                        ui.add(egui::Label::new(title).truncate()).on_hover_text(&m.title);
                    });
                });
            });
            let mut open = false;
            if m.total > 0 || m.target.is_some() {
                ui.horizontal(|ui| {
                    if m.total > 0 {
                        if let Some(id) = fold {
                            open = fold_toggle(ui, id);
                        }
                        ui.label(RichText::new(format!("{}/{}", m.done, m.total)).monospace().size(12.0).color(p.faint));
                        let w = (ui.available_width() - if m.target.is_some() { 130.0 } else { 0.0 }).max(60.0);
                        ui.allocate_ui(vec2(w, 12.0), |ui| progress(ui, m.done as f32 / m.total as f32, p));
                    }
                    if let Some(t) = &m.target {
                        ui.label(RichText::new(format!("{} {t}", icon::FLAG)).size(12.5).color(p.faint));
                    }
                });
            }
            if open {
                item_glance(ui, &m.items, p);
            }
            if body && !m.body.trim().is_empty() {
                ui.add(egui::Label::new(RichText::new(m.body.trim()).size(12.5).color(p.dim)).wrap());
            }
            ui.add_space(6.0);
        });
    });
    let rect = r.response.rect;
    let c = egui::pos2(rect.left() + gutter / 2.0 - 2.0, rect.top() + 10.0);
    let painter = ui.painter();
    if !last {
        let bottom = rect.bottom() + ui.spacing().item_spacing.y + 10.0;
        painter.line_segment([c + vec2(0.0, 7.0), egui::pos2(c.x, bottom - 7.0)], Stroke::new(2.0, p.border));
    }
    match m.status {
        MilestoneStatus::Planned => {
            painter.circle(c, 5.5, p.surface, Stroke::new(2.0, p.faint));
        }
        MilestoneStatus::InProgress => {
            painter.circle_filled(c, 8.0, color.gamma_multiply(0.25));
            painter.circle_filled(c, 5.0, color);
        }
        _ => {
            painter.circle_filled(c, 5.5, color);
        }
    }
}

/// A caret that folds and unfolds something; the state is remembered under `id`. Returns whether it is open.
pub(crate) fn fold_toggle(ui: &mut egui::Ui, id: egui::Id) -> bool {
    let mut open = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, false));
    let (glyph, tip) = if open { (icon::CARET_DOWN, "Hide the items") } else { (icon::CARET_RIGHT, "Show the items") };
    if theme::button(ui, glyph, egui::FontId::proportional(13.0), 0.0, theme::Kind::Ghost, vec2(20.0, 18.0)).on_hover_text(tip).clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    }
    open
}

/// An entry's items, read-only: a box or tick, indented by depth (edit them in the List view).
pub(crate) fn item_glance(ui: &mut egui::Ui, items: &[TodoItem], p: Palette) {
    if items.is_empty() {
        ui.label(RichText::new("No items.").size(12.5).color(p.faint));
    }
    for it in items {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(it.depth as f32 * 16.0);
            let (glyph, c) = if it.done { (icon::CHECK_SQUARE, p.green) } else { (icon::SQUARE, p.faint) };
            ui.label(RichText::new(glyph).size(14.0).color(c));
            let mut t = RichText::new(theme::plain_md(&it.text)).size(12.5).color(if it.done { p.faint } else { p.text });
            if it.done {
                t = t.strikethrough();
            }
            ui.add(egui::Label::new(t).wrap());
        });
    }
}

pub(crate) fn progress(ui: &mut egui::Ui, f: f32, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 2, p.border);
    let mut done = rect;
    done.set_width(rect.width() * f.clamp(0.0, 1.0));
    ui.painter().rect_filled(done, 2, p.green);
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

// --- file loading ---------------------------------------------------------------------------------------------

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The project's TODO file: its own setting, else the first known name.
pub(crate) fn todo_path(q: &Project, root: &Path) -> Option<PathBuf> {
    pa_core::ops::todo_path_in(q, root)
}

pub(crate) fn roadmap_path(q: &Project, root: &Path) -> Option<PathBuf> {
    pa_core::ops::roadmap_path_in(q, root)
}

/// Load (or reload when the file changed on disk; checked at most once a second) the TODO file.
pub(crate) fn load_todo(app: &mut App, q: &Project, root: &Path) {
    let Some(path) = todo_path(q, root) else {
        app.project.todos.remove(&q.id);
        return;
    };
    if let Some(l) = app.project.todos.get_mut(&q.id)
        && l.path == path
    {
        if l.checked.elapsed().as_secs_f32() < 1.0 {
            return;
        }
        l.checked = Instant::now();
        if mtime(&path) == l.mtime {
            return;
        }
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let style = pa_core::ops::style_of(q, &text);
            let doc = todo::parse_with(&text, &style);
            app.project.todos.insert(q.id, Loaded { mtime: mtime(&path), path, doc, text, checked: Instant::now() });
        }
        Err(_) => {
            app.project.todos.remove(&q.id);
        }
    }
}

pub(crate) fn load_roadmap(app: &mut App, q: &Project, root: &Path) {
    let Some(path) = roadmap_path(q, root) else {
        app.project.roadmaps.remove(&q.id);
        return;
    };
    if let Some(l) = app.project.roadmaps.get_mut(&q.id)
        && l.path == path
    {
        if l.checked.elapsed().as_secs_f32() < 1.0 {
            return;
        }
        l.checked = Instant::now();
        if mtime(&path) == l.mtime {
            return;
        }
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let doc = roadmap::parse_roadmap(&text);
            app.project.roadmaps.insert(q.id, Loaded { mtime: mtime(&path), path, doc, text, checked: Instant::now() });
        }
        Err(_) => {
            app.project.roadmaps.remove(&q.id);
        }
    }
}

/// Write `new_text` over the file read as `old`; on a conflict reload and say so. Returns whether it was written.
fn write_file(app: &mut App, path: &Path, old: &str, new_text: &str) -> bool {
    match todo::save(path, old, new_text) {
        Ok(()) => true,
        Err(SaveError::Changed) => {
            app.message = Some(format!("{} changed outside the app; reloaded, please redo the edit", path.display()));
            false
        }
        Err(e) => {
            app.message = Some(format!("Could not write {}: {e}", path.display()));
            false
        }
    }
}

/// Apply an edit to the loaded TODO doc and write it back.
pub(crate) fn edit_todo(app: &mut App, id: i64, f: impl FnOnce(&mut TodoDoc) -> Result<(), todo::TodoError>) {
    let Some(l) = app.project.todos.get_mut(&id) else { return };
    let mut doc = todo::parse_with(&l.text, &l.doc.style);
    if let Err(e) = f(&mut doc) {
        app.message = Some(format!("TODO edit refused: {e}"));
        return;
    }
    let (path, old, new_text) = (l.path.clone(), l.text.clone(), doc.render());
    if write_file(app, &path, &old, &new_text) {
        let open = doc.open_count() as u32;
        let l = app.project.todos.get_mut(&id).expect("loaded");
        (l.text, l.doc, l.mtime) = (new_text, doc, mtime(&path));
        set_info(app, id, |i| i.todo_open = Some(open));
    } else {
        app.project.todos.remove(&id);
    }
}

pub(crate) fn edit_roadmap(app: &mut App, id: i64, f: impl FnOnce(&mut Roadmap) -> Result<(), todo::TodoError>) {
    let Some(l) = app.project.roadmaps.get_mut(&id) else { return };
    let mut doc = roadmap::parse_roadmap(&l.text);
    if let Err(e) = f(&mut doc) {
        app.message = Some(format!("Roadmap edit refused: {e}"));
        return;
    }
    doc.reparse();
    let (path, old, new_text) = (l.path.clone(), l.text.clone(), doc.render());
    if write_file(app, &path, &old, &new_text) {
        let l = app.project.roadmaps.get_mut(&id).expect("loaded");
        (l.text, l.doc, l.mtime) = (new_text, doc, mtime(&path));
    } else {
        app.project.roadmaps.remove(&id);
    }
}

/// Keep the overview's cached numbers in step with an edit (the next refresh recomputes them anyway).
fn set_info(app: &mut App, id: i64, f: impl Fn(&mut pa_core::Info)) {
    if let Some(q) = app.projects.iter_mut().find(|q| q.id == id) {
        for c in &mut q.checkouts {
            if let Some(i) = c.info.as_mut() {
                f(i);
            }
        }
    }
}

// --- TODOs ----------------------------------------------------------------------------------------------------

pub(crate) enum ItemAction {
    Toggle(String, bool),
    StartEdit(String, String),
    SaveEdit(String, String),
    CancelEdit,
    Remove(String),
    Move(String, i32),
    MoveTo(String, String),
    /// Drag and drop: (item, the item it goes before).
    MoveBefore(String, String),
    Add(String, String),
    AddChild(String, String),
    /// Open a GitHub issue from the item (id, text); handled by the TODOs section.
    CreateIssue(String, String),
}

fn todos(app: &mut App, ui: &mut egui::Ui, id: i64, q: &Project, root: &Path, p: Palette) {
    load_todo(app, q, root);
    let Some(l) = app.project.todos.get(&id) else {
        empty_file(app, ui, id, root, "TODO.md", "# TODO\n\n## Open\n\n", p);
        return;
    };
    let (path, editable, text, style) = (l.path.clone(), l.doc.editable, l.text.clone(), l.doc.style.clone());
    let all_items = l.doc.items.clone();
    let sections: Vec<String> = section_names(&l.doc);
    let open = all_items.iter().filter(|i| !i.done).count();
    let done = all_items.len() - open;
    let slug = github_slug(q, root);
    let mut import_to: Option<String> = None;
    file_bar(app, ui, &path, root, &format!("{open} open · {done} done"), p, |app, ui| {
        if slug.is_some() && editable {
            ui.menu_button(RichText::new(format!("{}  Import issues", icon::GITHUB_LOGO)).size(13.0), |ui| {
                ui.label(caps("Open issues not linked yet, into", p.faint));
                for s in sections.iter().chain(std::iter::once(&"Issues".to_string())).collect::<std::collections::BTreeSet<_>>() {
                    if ui.button(if s.is_empty() { "General" } else { s }).clicked() {
                        import_to = Some(s.clone());
                    }
                }
            });
        }
        if theme::secondary(ui, format!("{}  Change style…", icon::SWAP)).on_hover_text("Rewrite the file in another TODO style, here or on a branch").clicked() {
            crate::restyle::open(app, id, root);
        }
        tag(ui, style.label(), p.dim).on_hover_text("How this file marks items and done items (set in Settings or with Change style)");
        if editable {
            theme::checkbox(ui, &mut app.project.show_done, "Show done");
            if done > 0 && style.completion == todo::Completion::Tick && theme::secondary(ui, format!("{}  Clear done", icon::BROOM)).on_hover_text("Move done items to a Done section").clicked() {
                edit_todo(app, id, |d| {
                    d.clear_done();
                    Ok(())
                });
            }
        }
    });
    if !editable {
        read_only(app, ui, id, root, &text, false, p);
        return;
    }
    if let Some(section) = import_to {
        import_issues(app, ui.ctx(), id, root, section);
    }
    if let Some(slug) = &slug {
        issue_banners(app, ui, id, root, slug, &all_items, p);
    }
    let mut acts = Vec::new();
    // A "move to section" style keeps done items in that section: shown last, and only with "Show done".
    let mut sections = sections;
    if let todo::Completion::MoveTo(done_section) = &style.completion {
        sections.retain(|s| s != done_section);
        if app.project.show_done {
            sections.push(done_section.clone());
        }
    }
    for s in &sections {
        let items: Vec<TodoItem> = all_items.iter().filter(|i| &i.section == s).cloned().collect();
        let title = if s.is_empty() { "General".to_string() } else { s.clone() };
        let n = items.iter().filter(|i| !i.done).count();
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(caps(format!("{title}   {n}"), p.dim));
            ui.add_space(4.0);
            item_list(&app.project, ui, id, &items, &sections, app.project.show_done, slug.as_deref(), p, &mut acts);
            add_box(&mut app.project.new_item, ui, id, s, "Add a TODO…", &mut acts);
        });
        ui.add_space(12.0);
    }
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("New section", p.faint));
        let key = (id, "\0section".to_string());
        let buf = app.project.new_item.entry(key.clone()).or_default();
        let r = ui.add(theme::field(buf, ui.available_width()).hint_text("Section name, Enter to add"));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !buf.trim().is_empty() {
            let name = buf.trim().to_string();
            buf.clear();
            // A section exists in the file once it has an item; the first item is a placeholder to edit.
            acts.push(ItemAction::Add(name, "New item".into()));
        }
    });
    let mut rest = Vec::new();
    for a in acts {
        match a {
            ItemAction::CreateIssue(item, text) => create_issue(app, ui.ctx(), id, root, item, text),
            ItemAction::Toggle(item, true) => {
                // Ticking an item linked to an open issue offers to close the issue.
                let n = all_items.iter().find(|i| i.id == item).and_then(|i| github::issue_ref(&i.text));
                let closed = n.and_then(|n| app.project.issues.get(&id)?.as_ref()?.get(&n)).is_some_and(|i| !i.open());
                if let Some(n) = n.filter(|_| !closed && slug.is_some()) {
                    app.project.close_offer = Some((id, n));
                }
                rest.push(ItemAction::Toggle(item, true));
            }
            a => rest.push(a),
        }
    }
    apply_items(app, id, rest, false);
}

/// `owner/name` when the checkout's `origin` is on GitHub (from its cached info, so no git call per frame).
fn github_slug(q: &Project, root: &Path) -> Option<String> {
    let remote = q.checkouts.iter().find(|c| Path::new(&c.path) == root)?.info.as_ref()?.git.as_ref()?.remote.clone()?;
    github_url(&remote)?.strip_prefix("https://github.com/").map(str::to_string)
}

/// The issue offers above the TODO sections: close the issue of an item just ticked, and tick items whose issue
/// was closed on GitHub. Issue states are read once per session (and after a change here).
fn issue_banners(app: &mut App, ui: &mut egui::Ui, id: i64, root: &Path, slug: &str, items: &[TodoItem], p: Palette) {
    let linked: Vec<(&TodoItem, u64)> = items.iter().filter_map(|i| github::issue_ref(&i.text).map(|n| (i, n))).collect();
    let banner = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        egui::Frame::new().fill(p.blue.gamma_multiply(0.08)).stroke(Stroke::new(1.0, p.blue.gamma_multiply(0.4))).corner_radius(8).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::GITHUB_LOGO).size(16.0).color(p.blue));
                add(ui);
            });
        });
        ui.add_space(10.0);
    };
    if let Some((pid, n)) = app.project.close_offer.filter(|o| o.0 == id) {
        let mut answer: Option<bool> = None;
        banner(ui, &mut |ui| {
            ui.label(format!("Issue #{n} is linked to the item you ticked. Close it on GitHub too?"));
            if theme::primary(ui, format!("Close issue #{n}")).clicked() {
                answer = Some(true);
            }
            if theme::ghost(ui, "Keep it open").clicked() {
                answer = Some(false);
            }
        });
        match answer {
            Some(true) => {
                app.project.close_offer = None;
                let root = root.to_path_buf();
                app.background(ui.ctx(), move || {
                    let r = github::close_issue(&root, n);
                    Box::new(move |app: &mut App| {
                        app.message = Some(match r {
                            Ok(()) => format!("Closed issue #{n}"),
                            Err(e) => format!("Issue #{n} not closed: {e}"),
                        });
                        app.project.issues.remove(&pid);
                    })
                });
            }
            Some(false) => app.project.close_offer = None,
            None => {}
        }
    }
    if linked.is_empty() {
        return;
    }
    if let std::collections::hash_map::Entry::Vacant(e) = app.project.issues.entry(id) {
        e.insert(None);
        let root = root.to_path_buf();
        app.background(ui.ctx(), move || {
            let list = github::issues(&root, "all");
            Box::new(move |app: &mut App| match list {
                Ok(list) => {
                    app.project.issues.insert(id, Some(list.into_iter().map(|i| (i.number, i)).collect()));
                }
                Err(e) => app.message = Some(format!("Could not read the GitHub issues: {e}")),
            })
        });
    }
    let Some(Some(states)) = app.project.issues.get(&id) else { return };
    let closed: Vec<(String, u64)> = linked.iter().filter(|(i, n)| !i.done && states.get(n).is_some_and(|s| !s.open())).map(|(i, n)| (i.id.clone(), *n)).collect();
    if closed.is_empty() {
        return;
    }
    let mut tick: Vec<String> = Vec::new();
    banner(ui, &mut |ui| {
        let list: Vec<String> = closed.iter().map(|(_, n)| format!("#{n}")).collect();
        ui.label(format!("Closed on GitHub ({}), still open here: {}.", slug, list.join(", ")));
        if theme::primary(ui, if closed.len() == 1 { "Tick it".to_string() } else { format!("Tick all {}", closed.len()) }).clicked() {
            tick = closed.iter().map(|(i, _)| i.clone()).collect();
        }
    });
    if !tick.is_empty() {
        edit_todo(app, id, move |d| {
            for i in &tick {
                d.set_done(i, true)?;
            }
            Ok(())
        });
    }
}

/// Open an issue from a TODO item, then write `(#n)` on its line.
fn create_issue(app: &mut App, ctx: &egui::Context, id: i64, root: &Path, item: String, text: String) {
    let root = root.to_path_buf();
    app.message = Some("Creating the issue…".into());
    app.background(ctx, move || {
        let (title, body) = github::issue_parts(&text);
        let r = github::create_issue(&root, &title, &body, &[]);
        Box::new(move |app: &mut App| match r {
            Ok((n, url)) => {
                edit_todo(app, id, |d| d.set_text(&item, &github::with_issue(&text, n)));
                app.message = Some(format!("Created issue #{n}: {url}"));
                app.project.issues.remove(&id);
            }
            Err(e) => app.message = Some(format!("Issue not created: {e}")),
        })
    });
}

/// Add the repository's open issues that no item links to yet to `section`.
fn import_issues(app: &mut App, ctx: &egui::Context, id: i64, root: &Path, section: String) {
    let root = root.to_path_buf();
    app.message = Some("Reading the open issues…".into());
    app.background(ctx, move || {
        let r = github::issues(&root, "open");
        Box::new(move |app: &mut App| match r {
            Ok(issues) => {
                let Some(l) = app.project.todos.get(&id) else { return };
                let texts: Vec<String> = github::unlinked(&l.doc, &issues).into_iter().map(github::import_text).collect();
                let n = texts.len();
                if n > 0 {
                    edit_todo(app, id, move |d| {
                        for t in &texts {
                            d.add(&section, t)?;
                        }
                        Ok(())
                    });
                }
                app.message = Some(format!("Imported {n} open issue(s); {} were linked already", issues.len() - n));
            }
            Err(e) => app.message = Some(format!("Could not read the issues: {e}")),
        })
    });
}

fn section_names(doc: &TodoDoc) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if doc.items.iter().any(|i| i.section.is_empty()) {
        v.push(String::new());
    }
    v.extend(doc.sections.iter().map(|s| s.title.clone()));
    v.dedup();
    v
}

/// Drag payload of an item row: (project, item id).
#[derive(Clone)]
struct DragItem(i64, String);

/// Items with a drag handle, checkboxes, inline editing, and a menu plus delete at the row's right edge (shown on
/// hover). Dropping a dragged item on a row puts it before that row. Done items hidden unless `show_done`.
/// `github`: the repository's `owner/name` when items can be linked to issues (TODO files only).
pub(crate) fn item_list(st: &State, ui: &mut egui::Ui, id: i64, items: &[TodoItem], sections: &[String], show_done: bool, github: Option<&str>, p: Palette, acts: &mut Vec<ItemAction>) {
    if items.is_empty() {
        ui.label(RichText::new("No items yet.").color(p.faint));
    } else if items.iter().all(|i| i.done && !show_done) {
        ui.label(RichText::new("Nothing open here.").color(p.faint));
    }
    // The row whose menu is open keeps its buttons while the pointer is in the menu.
    let menu_key = egui::Id::new(("item-menu", id));
    let menu_row: Option<String> = ui.ctx().data(|d| d.get_temp(menu_key));
    for it in items {
        if it.done && !show_done {
            continue;
        }
        let editing = st.editing.as_ref().filter(|e| e.0 == id && e.1 == it.id).map(|e| e.2.clone());
        let drag_id = egui::Id::new(("item-drag", id, &it.id));
        let dragged = ui.ctx().is_being_dragged(drag_id);
        let r = ui.horizontal_top(|ui| {
            ui.add_space(it.depth as f32 * 24.0);
            ui.dnd_drag_source(drag_id, DragItem(id, it.id.clone()), |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 26.0), Sense::hover());
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::DOTS_SIX_VERTICAL, egui::FontId::proportional(16.0), if dragged { p.text } else { p.faint });
            })
            .response
            .on_hover_text("Drag to move");
            let glyph = if it.done { icon::CHECK_SQUARE } else { icon::SQUARE };
            if theme::button(ui, glyph, egui::FontId::proportional(18.0), 0.0, theme::Kind::Ghost, vec2(26.0, 26.0)).on_hover_text(if it.done { "Mark as open" } else { "Mark as done" }).clicked() {
                acts.push(ItemAction::Toggle(it.id.clone(), !it.done));
            }
            // Room for the menu and delete buttons on the right, so they never cover the text.
            let w = (ui.available_width() - 64.0).max(80.0);
            match editing {
                Some(mut text) => {
                    let r = ui.add(egui::TextEdit::multiline(&mut text).desired_rows(1).desired_width(w));
                    r.request_focus();
                    let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter) && !i.modifiers.shift, i.key_pressed(egui::Key::Escape)));
                    if esc {
                        acts.push(ItemAction::CancelEdit);
                    } else if enter || r.lost_focus() {
                        acts.push(ItemAction::SaveEdit(it.id.clone(), text.trim_end_matches('\n').to_string()));
                    } else {
                        acts.push(ItemAction::StartEdit(it.id.clone(), text));
                    }
                }
                None => {
                    let mut rt = RichText::new(theme::plain_md(&it.text));
                    if it.done || dragged {
                        rt = rt.color(p.faint);
                    }
                    if it.done {
                        rt = rt.strikethrough();
                    }
                    let r = ui
                        .allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                            ui.set_width(w);
                            ui.add_space(4.0);
                            ui.add(egui::Label::new(rt).wrap().sense(Sense::click()))
                        })
                        .inner;
                    if r.clicked() {
                        acts.push(ItemAction::StartEdit(it.id.clone(), it.text.clone()));
                    }
                    r.on_hover_text("Click to edit");
                }
            }
        });
        let rect = r.response.rect;
        // Drop target: a line above the row while an item hovers it; on release the item goes before this one.
        let hit = ui.interact(rect, egui::Id::new(("item-drop", id, &it.id)), Sense::hover());
        if let Some(src) = hit.dnd_hover_payload::<DragItem>()
            && src.0 == id
            && src.1 != it.id
        {
            let x = rect.left() + it.depth as f32 * 24.0;
            ui.painter().hline(x..=rect.right(), rect.top() - 1.0, Stroke::new(2.0, p.blue));
        }
        if let Some(src) = hit.dnd_release_payload::<DragItem>()
            && src.0 == id
            && src.1 != it.id
        {
            acts.push(ItemAction::MoveBefore(src.1.clone(), it.id.clone()));
        }
        let menu_open = menu_row.as_deref() == Some(it.id.as_str());
        if (ui.rect_contains_pointer(rect) || menu_open) && st.editing.is_none() && !dragged {
            let area = egui::Rect::from_min_max(egui::pos2(rect.right() - 60.0, rect.top()), rect.right_top() + vec2(0.0, 28.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Min)));
            child.spacing_mut().item_spacing.x = 2.0;
            if theme::icon_button(&mut child, icon::TRASH, "Delete (with sub-items)").clicked() {
                acts.push(ItemAction::Remove(it.id.clone()));
            }
            let more = theme::icon_button(&mut child, icon::DOTS_THREE, "More actions");
            let shown = egui::Popup::menu(&more).show(|ui| {
                if ui.button(format!("{}  Add a sub-item", icon::PLUS)).clicked() {
                    acts.push(ItemAction::AddChild(it.id.clone(), "New sub-item".into()));
                }
                if ui.button(format!("{}  Move up", icon::ARROW_UP)).clicked() {
                    acts.push(ItemAction::Move(it.id.clone(), -1));
                }
                if ui.button(format!("{}  Move down", icon::ARROW_DOWN)).clicked() {
                    acts.push(ItemAction::Move(it.id.clone(), 1));
                }
                let others: Vec<&String> = sections.iter().filter(|s| **s != it.section).collect();
                if !others.is_empty() {
                    ui.menu_button(format!("{}  Move to section", icon::ARROW_RIGHT), |ui| {
                        for s in others {
                            if ui.button(if s.is_empty() { "General" } else { s }).clicked() {
                                acts.push(ItemAction::MoveTo(it.id.clone(), s.clone()));
                            }
                        }
                    });
                }
                if let Some(slug) = github {
                    match github::issue_ref(&it.text) {
                        Some(n) => {
                            if ui.button(format!("{}  Open issue #{n}", icon::GITHUB_LOGO)).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://github.com/{slug}/issues/{n}")));
                            }
                        }
                        None => {
                            if ui.button(format!("{}  Create a GitHub issue", icon::GITHUB_LOGO)).on_hover_text("Publishes the item as an issue").clicked() {
                                acts.push(ItemAction::CreateIssue(it.id.clone(), it.text.clone()));
                            }
                        }
                    }
                }
            });
            match (shown.is_some(), menu_open) {
                (true, false) => ui.ctx().data_mut(|d| { d.insert_temp(menu_key, it.id.clone()); }),
                (false, true) => ui.ctx().data_mut(|d| d.remove::<String>(menu_key)),
                _ => {}
            }
        }
    }
}

pub(crate) fn add_box(bufs: &mut HashMap<(i64, String), String>, ui: &mut egui::Ui, id: i64, section: &str, hint: &str, acts: &mut Vec<ItemAction>) {
    let buf = bufs.entry((id, section.to_string())).or_default();
    let row = ui.horizontal(|ui| {
        ui.label(RichText::new(icon::PLUS).color(pal(ui.ctx()).faint));
        let r = ui.add(egui::TextEdit::singleline(buf).hint_text(hint).frame(egui::Frame::NONE).desired_width(f32::INFINITY));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !buf.trim().is_empty() {
            acts.push(ItemAction::Add(section.to_string(), buf.trim().to_string()));
            buf.clear();
            r.request_focus();
        }
    });
    // Dropping an item on the "Add" row moves it to the end of this section.
    let rect = row.response.rect;
    let hit = ui.interact(rect, egui::Id::new(("add-drop", id, section)), Sense::hover());
    if hit.dnd_hover_payload::<DragItem>().is_some_and(|s| s.0 == id) {
        ui.painter().hline(rect.x_range(), rect.top() - 1.0, Stroke::new(2.0, pal(ui.ctx()).blue));
    }
    if let Some(src) = hit.dnd_release_payload::<DragItem>()
        && src.0 == id
    {
        acts.push(ItemAction::MoveTo(src.1.clone(), section.to_string()));
    }
}

type DocEdit = Box<dyn FnOnce(&mut TodoDoc) -> Result<(), todo::TodoError>>;

/// Run item actions against the TODO file (`roadmap` = false) or the roadmap's document.
pub(crate) fn apply_items(app: &mut App, id: i64, acts: Vec<ItemAction>, roadmap: bool) {
    for a in acts {
        let op = |f: DocEdit, app: &mut App| {
            if roadmap {
                edit_roadmap(app, id, |r| f(&mut r.doc));
            } else {
                edit_todo(app, id, f);
            }
        };
        match a {
            ItemAction::Toggle(i, done) => op(Box::new(move |d| d.set_done(&i, done)), app),
            ItemAction::StartEdit(i, text) => app.project.editing = Some((id, i, text)),
            ItemAction::CancelEdit => app.project.editing = None,
            ItemAction::SaveEdit(i, text) => {
                app.project.editing = None;
                if !text.trim().is_empty() {
                    op(Box::new(move |d| d.set_text(&i, &text)), app);
                }
            }
            ItemAction::Remove(i) => op(Box::new(move |d| d.remove(&i)), app),
            ItemAction::Move(i, delta) => op(Box::new(move |d| d.move_by(&i, delta)), app),
            ItemAction::MoveTo(i, s) => op(Box::new(move |d| d.move_to(&i, &s)), app),
            ItemAction::MoveBefore(i, t) => op(Box::new(move |d| d.move_before(&i, &t)), app),
            ItemAction::Add(s, text) => op(Box::new(move |d| d.add(&s, &text).map(|_| ())), app),
            ItemAction::AddChild(i, text) => op(Box::new(move |d| d.add_child(&i, &text).map(|_| ())), app),
            // The TODOs section takes these out before (they need the checkout).
            ItemAction::CreateIssue(..) => {}
        }
    }
}

/// File path, a summary, "Open file", and extra controls on the right.
pub(crate) fn file_bar(app: &mut App, ui: &mut egui::Ui, path: &Path, root: &Path, summary: &str, p: Palette, extra: impl FnOnce(&mut App, &mut egui::Ui)) {
    ui.horizontal_wrapped(|ui| {
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        ui.label(RichText::new(format!("{}  {rel}", icon::FILE_TEXT)).monospace().color(p.dim));
        ui.label(caps(summary, p.faint));
        theme::right(ui, "project.rs:1187", egui::Align::Center, |ui| {
            if theme::secondary(ui, format!("{}  Open file", icon::ARROW_SQUARE_OUT)).clicked() {
                open_at(app, path, None);
            }
            extra(app, ui);
        });
    });
    ui.add_space(8.0);
}

/// A file in another style: rendered read-only with a banner offering conversion (D-5).
pub(crate) fn read_only(app: &mut App, ui: &mut egui::Ui, id: i64, root: &Path, text: &str, roadmap: bool, p: Palette) {
    egui::Frame::new().fill(p.orange.gamma_multiply(0.08)).stroke(Stroke::new(1.0, p.orange.gamma_multiply(0.4))).corner_radius(8).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::WARNING).size(18.0).color(p.orange));
            ui.label("This file uses another style, so it is shown read-only. Convert it to the app's format to edit it here.");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::primary(ui, "Convert…").clicked() {
                    if roadmap {
                        crate::restyle::open_roadmap(app, id, root);
                    } else {
                        crate::restyle::open(app, id, root);
                    }
                }
            });
        });
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        egui_commonmark::CommonMarkViewer::new().show(ui, &mut app.project.md, text);
    });
}

/// No file yet: offer to create one.
pub(crate) fn empty_file(app: &mut App, ui: &mut egui::Ui, id: i64, root: &Path, name: &str, content: &str, p: Palette) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon::FILE_TEXT).size(40.0).color(p.faint));
        ui.label(RichText::new(format!("This project has no {name} yet.")).color(p.dim));
        ui.add_space(6.0);
        if theme::primary(ui, format!("Create {name}")).clicked() {
            let path = root.join(name);
            if write_file(app, &path, "", content) {
                app.project.todos.remove(&id);
                app.project.roadmaps.remove(&id);
            }
        }
    });
}

// --- roadmap --------------------------------------------------------------------------------------------------

pub(crate) fn status_label(s: &MilestoneStatus) -> String {
    match s {
        MilestoneStatus::Planned => "planned".into(),
        MilestoneStatus::InProgress => "in progress".into(),
        MilestoneStatus::Done => "done".into(),
        MilestoneStatus::Released => "released".into(),
        // ponytail: the full text shows on hover (status_hover), the chip stays short.
        MilestoneStatus::Other(_) => "unknown".into(),
    }
}

/// Hover text of a status chip: the whole `Status:` line when it says more than the status word.
pub(crate) fn status_hover(r: egui::Response, m: &roadmap::Milestone) -> egui::Response {
    match &m.status_note {
        Some(n) => r.on_hover_text(format!("Status: {n}")),
        None => r,
    }
}

pub(crate) fn status_color(s: &MilestoneStatus, p: Palette) -> Color32 {
    match s {
        MilestoneStatus::Planned => p.dim,
        MilestoneStatus::InProgress => p.blue,
        MilestoneStatus::Done | MilestoneStatus::Released => p.green,
        MilestoneStatus::Other(_) => p.purple,
    }
}

// --- code TODOs -----------------------------------------------------------------------------------------------

fn code_todos(app: &mut App, ui: &mut egui::Ui, id: i64, q: &Project, root: &Path, p: Palette) {
    app.project.code.entry(id).or_insert_with(|| pa_core::codetodo::scan(root));
    let list = app.project.code[&id].clone();
    ui.horizontal(|ui| {
        ui.label(caps(format!("{} comments in {} files", list.len(), count_files(&list)), p.faint));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::secondary(ui, format!("{}  Rescan", icon::ARROWS_CLOCKWISE)).clicked() {
                app.project.code.remove(&id);
            }
        });
    });
    ui.add_space(8.0);
    if list.is_empty() {
        ui.label(RichText::new("No TODO, FIXME, HACK or XXX comments found.").color(p.dim));
        return;
    }
    load_todo(app, q, root);
    let sections: Vec<String> = app.project.todos.get(&id).map(|l| section_names(&l.doc)).unwrap_or_default();
    let mut promote: Option<(CodeTodo, String)> = None;
    let mut open: Option<(PathBuf, usize)> = None;
    let mut file = String::new();
    for t in &list {
        if t.file != file {
            file = t.file.clone();
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}  {file}", icon::FILE_CODE)).monospace().size(12.5).color(p.dim));
        }
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            let r = ui.add(egui::Label::new(RichText::new(format!("{:>5}", t.line)).monospace().size(12.0).color(p.faint)).sense(Sense::click()));
            if r.on_hover_text("Open in VS Code at this line").clicked() {
                open = Some((root.join(&t.file), t.line));
            }
            let color = match t.keyword.as_str() {
                "FIXME" => p.red,
                "HACK" => p.orange,
                "XXX" => p.purple,
                _ => p.blue,
            };
            chip(ui, &t.keyword, color);
            ui.add(egui::Label::new(&t.text).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button(RichText::new(format!("{} Promote", icon::ARROW_UP)).size(12.5), |ui| {
                    ui.label(caps("Add to TODO section", p.faint));
                    let mut names = sections.clone();
                    if !names.iter().any(|s| s == "Code TODOs") {
                        names.push("Code TODOs".into());
                    }
                    for s in names {
                        if ui.button(if s.is_empty() { "General".to_string() } else { s.clone() }).clicked() {
                            promote = Some((t.clone(), s));
                        }
                    }
                });
            });
        });
    }
    if let Some((path, line)) = open {
        open_at(app, &path, Some(line));
    }
    if let Some((t, section)) = promote {
        if !app.project.todos.contains_key(&id) {
            let path = root.join("TODO.md");
            write_file(app, &path, "", "# TODO\n");
            load_todo(app, q, root);
        }
        let text = format!("{} ({}:{})", t.text, t.file, t.line);
        edit_todo(app, id, |d| d.add(&section, &text).map(|_| ()));
        app.message = Some(format!("Added to TODO section \"{section}\""));
    }
}

fn count_files(list: &[CodeTodo]) -> usize {
    let mut f: Vec<&str> = list.iter().map(|t| t.file.as_str()).collect();
    f.dedup();
    f.len()
}

/// Open a file (at a line) in VS Code.
pub(crate) fn open_at(app: &mut App, path: &Path, line: Option<usize>) {
    let target = match line {
        Some(l) => format!("{}:{l}", path.display()),
        None => path.display().to_string(),
    };
    let dir = path.parent().unwrap_or(path).display().to_string();
    let template = format!(r#"code -g "{target}""#);
    if let Err(e) = pa_core::launch(&template, &dir) {
        app.message = Some(format!("Could not open {target}: {e}"));
    }
}

// --- notes ----------------------------------------------------------------------------------------------------

fn notes(app: &mut App, id: i64) -> &Vec<Note> {
    if !app.project.notes.contains_key(&id) {
        let list = app.store.notes(Some(id)).unwrap_or_default();
        app.project.notes.insert(id, list);
    }
    &app.project.notes[&id]
}

fn notes_view(app: &mut App, ui: &mut egui::Ui, id: i64, p: Palette) {
    let list = notes(app, id).clone();
    let selected = app.project.note.get(&id).copied().filter(|n| list.iter().any(|x| x.id == *n)).or(list.first().map(|n| n.id));
    let mut reload = false;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(240.0);
            if theme::primary(ui, format!("{}  New note", icon::PLUS)).clicked() {
                let r = app.store.add_note(Some(id), "Untitled");
                if let Some(n) = app.ok(r) {
                    app.project.note.insert(id, n);
                    reload = true;
                }
            }
            ui.add_space(6.0);
            for n in &list {
                let on = Some(n.id) == selected;
                let text = format!("{}\t{}", n.title, relative_secs(n.updated_at));
                if theme::button(ui, &text, theme::semibold(14.0), 0.0, theme::Kind::Nav(on), vec2(240.0, 36.0)).clicked() {
                    app.project.note.insert(id, n.id);
                }
            }
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            let Some(mut n) = list.iter().find(|n| Some(n.id) == selected).cloned() else {
                ui.label(RichText::new("No notes yet. Notes are kept in the app, not in the repository.").color(p.dim));
                return;
            };
            let before = n.clone();
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut n.title).font(theme::semibold(20.0)).frame(egui::Frame::NONE).desired_width(ui.available_width() - 200.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(ui, icon::TRASH, "Delete note").clicked() {
                        let r = app.store.delete_note(n.id);
                        app.ok(r);
                        reload = true;
                    }
                    let mut preview = app.project.note_preview;
                    let items = [("Edit".to_string(), None), ("Preview".to_string(), None)];
                    if let Some(i) = theme::segmented(ui, &items, preview as usize) {
                        preview = i == 1;
                    }
                    app.project.note_preview = preview;
                });
            });
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                if app.project.note_preview {
                    egui_commonmark::CommonMarkViewer::new().show(ui, &mut app.project.md, &n.body);
                } else {
                    ui.add(egui::TextEdit::multiline(&mut n.body).frame(egui::Frame::NONE).desired_rows(18).desired_width(f32::INFINITY).hint_text("Markdown…"));
                }
            });
            if n != before && !reload {
                let r = app.store.update_note(&n);
                app.ok(r);
                if let Some(x) = app.project.notes.get_mut(&id).and_then(|v| v.iter_mut().find(|x| x.id == n.id)) {
                    *x = n;
                }
            }
        });
    });
    if reload {
        app.project.notes.remove(&id);
    }
}

fn relative_secs(t: i64) -> String {
    let d = pa_core::now_secs() - t;
    match d {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86_400 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

// --- checkouts and settings -----------------------------------------------------------------------------------

/// A change to run after the grid is drawn (the grid borrows a copy of the project).
type Deferred = Box<dyn FnOnce(&mut App, &egui::Context)>;

fn checkouts(app: &mut App, ui: &mut egui::Ui, id: i64, selected: i64, p: Palette) {
    let Some(q) = app.projects.iter().find(|q| q.id == id).cloned() else { return };
    let mut action: Option<Deferred> = None;
    ui.label(RichText::new("Clones and worktrees of this project. Launchers open the selected one.").color(p.dim));
    ui.add_space(8.0);
    for c in &q.checkouts {
        let info = c.info.as_ref();
        let git = info.and_then(|i| i.git.as_ref());
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if ui.radio(c.id == selected, "").on_hover_text("Launchers open this checkout").clicked() {
                    app.project.selected.insert(id, c.id);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(short_path(&c.path)).monospace());
                        if c.primary {
                            chip(ui, "primary", p.blue);
                        }
                        if git.is_some_and(|g| g.linked) {
                            chip(ui, "worktree", p.purple);
                        }
                        if info.is_some_and(|i| !i.exists) {
                            chip(ui, "missing", p.red);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        match git {
                            Some(g) => {
                                ui.label(RichText::new(format!("{} {}", icon::GIT_BRANCH, g.branch.as_deref().unwrap_or("(detached)"))).color(p.dim));
                                ui.label(RichText::new(format!("{} {} changed {}", icon::GIT_DIFF, g.dirty, ahead_behind(g.ahead_behind))).color(p.dim));
                                if let Some(d) = g.last_commit.as_deref() {
                                    ui.label(RichText::new(format!("{} {}", icon::CLOCK, relative(d))).color(p.dim));
                                }
                                if let Some(r) = g.remote.as_deref() {
                                    ui.label(RichText::new(r).color(p.faint));
                                }
                            }
                            None => {
                                ui.label(RichText::new("no git").color(p.faint));
                            }
                        }
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (cid, path) = (c.id, c.path.clone());
                    if q.checkouts.len() > 1 && theme::secondary(ui, "Remove").on_hover_text(format!("Forget {path}; the folder is not touched")).clicked() {
                        action = Some(Box::new(move |app, _| {
                            let r = app.store.remove_checkout(cid);
                            app.ok(r);
                            app.reload();
                        }));
                    }
                    if theme::secondary(ui, "Relocate…").on_hover_text("The folder moved: pick its new place").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        let new = pa_core::norm(&dir);
                        action = Some(Box::new(move |app, ctx| {
                            let r = app.store.relocate_checkout(cid, &new);
                            if app.ok(r).is_some() {
                                app.reload();
                                app.refresh(ctx, vec![(cid, new)]);
                            }
                        }));
                    }
                    if !c.primary && theme::secondary(ui, "Make primary").clicked() {
                        action = Some(Box::new(move |app, _| {
                            let r = app.store.set_primary(cid);
                            app.ok(r);
                            app.reload();
                        }));
                    }
                });
            });
        });
        ui.add_space(8.0);
    }
    ui.horizontal(|ui| {
        if theme::secondary(ui, format!("{}  Add checkout…", icon::PLUS)).on_hover_text("Another clone or worktree of this project").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            let path = pa_core::norm(&dir);
            action = Some(Box::new(move |app, ctx| match app.store.checkout_by_path(&path) {
                Ok(Some(_)) => app.message = Some(format!("{path} is already tracked")),
                r => {
                    if app.ok(r).is_some() {
                        let r = app.store.add_checkout(id, &path);
                        if let Some(cid) = app.ok(r) {
                            app.reload();
                            app.refresh(ctx, vec![(cid, path)]);
                        }
                    }
                }
            }));
        }
        if theme::secondary(ui, format!("{}  Refresh", icon::ARROWS_CLOCKWISE)).clicked() {
            action = Some(Box::new(move |app, ctx| app.refresh_project(ctx, id)));
        }
    });
    if let Some(f) = action {
        f(app, &ui.ctx().clone());
    }
}

fn settings(app: &mut App, ui: &mut egui::Ui, q: &mut Project, p: Palette) {
    let id = q.id;
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Organise", p.faint));
        ui.add_space(4.0);
        egui::Grid::new(("meta", id)).num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label(RichText::new("Group").color(p.dim));
            group_combo(ui, ("group", id), &app.groups, &mut q.group_id);
            ui.end_row();
            ui.label(RichText::new("Purpose").color(p.dim));
            purpose_combo(ui, ("purpose", id), &app.purposes, &mut q.purpose_id);
            ui.end_row();
            ui.label(RichText::new("Status").color(p.dim));
            status_combo(ui, ("status", id), &mut q.status);
            ui.end_row();
            ui.label(RichText::new("Tags").color(p.dim));
            let text = app.project.tags.entry(id).or_insert_with(|| q.tags.join(", "));
            if ui.add(theme::field(text, 360.0).hint_text("comma-separated")).lost_focus() {
                let mut tags: Vec<String> = text.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                tags.sort_by_key(|t| t.to_lowercase());
                tags.dedup();
                *text = tags.join(", ");
                q.tags = tags;
            }
            ui.end_row();
        });
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Files", p.faint));
        ui.label(RichText::new("Relative to the checkout. Empty = found automatically (TODO.md, TODOS.md, TODO.txt…; ROADMAP.md, docs/roadmap.md…).").color(p.dim));
        ui.add_space(4.0);
        egui::Grid::new(("files", id)).num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            for (label, value) in [("TODO file", &mut q.todo_path), ("Roadmap file", &mut q.roadmap_path)] {
                ui.label(RichText::new(label).color(p.dim));
                let mut s = value.clone().unwrap_or_default();
                if ui.add(theme::field(&mut s, 360.0).hint_text("auto")).changed() {
                    *value = (!s.trim().is_empty()).then(|| s.trim().to_string());
                }
                ui.end_row();
            }
            ui.label(RichText::new("TODO style").color(p.dim));
            let detected = app.project.todos.get(&id).map(|l| todo::Style::detect(&l.text)).unwrap_or(todo::Style::CHECKLIST);
            let chosen = q.todo_style.as_deref().and_then(todo::Style::decode);
            let text = match &chosen {
                Some(s) => s.label(),
                None => format!("Detect ({})", detected.label()),
            };
            let mut pick: Option<Option<todo::Style>> = None;
            theme::select(ui, ("todo_style", id), 360.0, text, |ui| {
                if ui.selectable_label(chosen.is_none(), format!("Detect ({})", detected.label())).clicked() {
                    pick = Some(None);
                }
                for s in crate::restyle::presets(&detected) {
                    if ui.selectable_label(chosen.as_ref() == Some(&s), s.label()).clicked() {
                        pick = Some(Some(s));
                    }
                }
            });
            if let Some(s) = pick {
                q.todo_style = s.map(|s| s.encode());
                app.project.todos.remove(&id);
            }
            ui.end_row();
        });
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Remove", p.faint));
        if app.project.confirm_remove == Some(id) {
            ui.horizontal(|ui| {
                ui.label("Forget this project? Its folders are not touched.");
                if theme::primary(ui, "Remove").clicked() {
                    let r = app.store.remove_project(id);
                    app.ok(r);
                    app.reload();
                }
                if theme::secondary(ui, "Cancel").clicked() {
                    app.project.confirm_remove = None;
                }
            });
        } else if theme::secondary(ui, format!("{}  Remove from app…", icon::TRASH)).clicked() {
            app.project.confirm_remove = Some(id);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn first_lines() {
        assert_eq!(super::first_line("a\nb"), "a");
    }
}
