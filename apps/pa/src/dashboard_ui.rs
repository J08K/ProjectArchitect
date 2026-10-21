//! A project's Overview as a dashboard (SPEC M6, D-20, D-46): widgets in an `egui_dock` layout the user arranges (drag a
//! widget's tab to split, close it, add it back). The layout is saved per project (`dashboard.<project id>`), or for
//! every project of a purpose (`dashboard.purpose.<purpose id>`), which a project without its own layout uses.
//! Commit activity, Claude sessions and disk size are read on background threads and kept for the session.

use crate::theme::{self, Palette, caps, pal};
use crate::{App, Job};
use egui::{Align, Id, Layout, RichText, WidgetText};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer, tab_viewer::OnCloseResponse};
use egui_phosphor::regular as icon;
use pa_core::Project;
use pa_core::claude::Session;
use pa_core::info::BULKY;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};

/// Weeks of activity (a year).
const WEEKS: usize = 52;
/// Claude sessions listed.
const SESSIONS: usize = 12;

/// Commits and Claude sessions per week, oldest first.
type Weekly = (Vec<usize>, Vec<usize>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Widget {
    Repository,
    Todos,
    Roadmap,
    Languages,
    Commits,
    Sessions,
    Disk,
    TodoHistory,
    RoadmapHistory,
}

const ALL: [Widget; 9] = [
    Widget::Repository,
    Widget::Todos,
    Widget::Roadmap,
    Widget::Languages,
    Widget::Commits,
    Widget::TodoHistory,
    Widget::RoadmapHistory,
    Widget::Sessions,
    Widget::Disk,
];

impl Widget {
    fn title(self) -> String {
        let (glyph, name) = match self {
            Widget::Repository => (icon::GIT_BRANCH, "Repository"),
            Widget::Todos => (icon::LIST_CHECKS, "Open TODOs"),
            Widget::Roadmap => (icon::FLAG, "Roadmap"),
            Widget::Languages => (icon::CODE, "Languages"),
            Widget::Commits => (icon::CHART_BAR, "Activity"),
            Widget::TodoHistory => (icon::CHART_LINE, "TODOs over time"),
            Widget::RoadmapHistory => (icon::CHART_LINE_UP, "Roadmap progress"),
            Widget::Sessions => (icon::ROBOT, "Claude sessions"),
            Widget::Disk => (icon::HARD_DRIVES, "Disk size"),
        };
        format!("{glyph}  {name}")
    }
}

/// Repository, TODOs and Roadmap on top; activity and history charts, languages, sessions and disk under them.
fn default_layout() -> DockState<Widget> {
    let mut d = DockState::new(vec![Widget::Repository]);
    let s = d.main_surface_mut();
    let [top, bottom] = s.split_below(NodeIndex::root(), 0.5, vec![Widget::Commits, Widget::TodoHistory, Widget::RoadmapHistory, Widget::Languages]);
    let [_, mid] = s.split_right(top, 0.33, vec![Widget::Todos]);
    s.split_right(mid, 0.5, vec![Widget::Roadmap]);
    let [_, right] = s.split_right(bottom, 0.5, vec![Widget::Sessions]);
    s.split_right(right, 0.5, vec![Widget::Disk]);
    d
}

#[derive(Default)]
pub struct State {
    /// Per project: the layout and its JSON as last saved or loaded.
    docks: HashMap<i64, (DockState<Widget>, String)>,
    /// Per project; `None` while reading.
    /// Commits and Claude sessions per week.
    commits: HashMap<i64, Option<Weekly>>,
    sessions: HashMap<i64, Option<Vec<Session>>>,
    disk: HashMap<i64, Option<(u64, [u64; 3])>>,
}

fn key(id: i64) -> String {
    format!("dashboard.{id}")
}

fn purpose_key(id: i64) -> String {
    format!("dashboard.purpose.{id}")
}

/// The project's layout, else its purpose's, else the default.
fn load(app: &App, q: &Project) -> DockState<Widget> {
    let mut keys = vec![key(q.id)];
    keys.extend(q.purpose_id.map(purpose_key));
    keys.iter().find_map(|k| serde_json::from_str(&app.setting(k, "")).ok()).unwrap_or_else(default_layout)
}

fn save(app: &mut App, key: &str, json: &str) {
    let r = app.store.set_setting(key, json);
    app.ok(r);
}

pub fn show(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path) {
    let p = pal(ui.ctx());
    if !app.dashboard.docks.contains_key(&q.id) {
        let d = load(app, q);
        let json = serde_json::to_string(&d).unwrap_or_default();
        app.dashboard.docks.insert(q.id, (d, json));
    }
    toolbar(app, ui, q, p);
    ui.add_space(8.0);
    let Some((mut dock, saved)) = app.dashboard.docks.remove(&q.id) else { return };
    if dock.iter_all_tabs().next().is_none() {
        ui.label(RichText::new("No widgets. Add some with \"Add widget\".").color(p.dim));
    }
    let mut widgets = Widgets { app, q, root, p, remove: None };
    let mut style = crate::dock_style(ui, p);
    style.tab_bar.height = 30.0;
    style.tab.minimum_width = None;
    style.tab.tab_body.bg_fill = p.surface;
    style.tab.tab_body.stroke = egui::Stroke::new(1.0, p.border);
    style.tab.tab_body.inner_margin = egui::Margin::same(14);
    DockArea::new(&mut dock)
        .id(Id::new(("dashboard", q.id)))
        .style(style)
        .show_add_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show_leaf_close_all_buttons(false)
        // No close button on the tabs (too easy to hit): right-click a tab to remove its widget.
        .show_close_buttons(false)
        .show_inside(ui, &mut widgets);
    if let Some(w) = widgets.remove
        && let Some(at) = dock.find_tab(&w)
    {
        dock.remove_tab(at);
    }
    // ponytail: serialised every frame to notice changes; a handful of nodes, cheap.
    let json = serde_json::to_string(&dock).unwrap_or_default();
    if json != saved {
        save(app, &key(q.id), &json);
    }
    app.dashboard.docks.insert(q.id, (dock, json));
}

fn toolbar(app: &mut App, ui: &mut egui::Ui, q: &Project, p: Palette) {
    let purpose = app.purpose(q.purpose_id).map(|u| (u.id, u.name.clone()));
    ui.horizontal_wrapped(|ui| {
        let Some((dock, json)) = app.dashboard.docks.get_mut(&q.id) else { return };
        let missing: Vec<Widget> = ALL.into_iter().filter(|w| dock.find_tab(w).is_none()).collect();
        ui.add_enabled_ui(!missing.is_empty(), |ui| {
            ui.menu_button(RichText::new(format!("{}  Add widget", icon::PLUS)).size(13.0), |ui| {
                for w in &missing {
                    if ui.button(w.title()).clicked() {
                        dock.push_to_first_leaf(*w);
                    }
                }
            });
        });
        ui.label(RichText::new("Drag a widget's tab to move or split it; right-click it to remove it.").size(12.5).color(p.faint));
        let json = json.clone();
        theme::right(ui, "dashboard_ui.rs:162", egui::Align::Center, |ui| {
            if theme::ghost(ui, format!("{}  Reset", icon::ARROW_COUNTER_CLOCKWISE))
                .on_hover_text("Forget this project's layout: use its purpose's layout, or the default")
                .clicked()
            {
                save(app, &key(q.id), "");
                app.dashboard.docks.remove(&q.id);
            }
            if let Some((id, name)) = purpose
                && theme::ghost(ui, format!("{}  Use for all {name}", icon::COPY))
                    .on_hover_text(format!("Save this layout for every {name} project without a layout of its own"))
                    .clicked()
            {
                save(app, &purpose_key(id), &json);
                app.message = Some(format!("Saved this layout for {name} projects."));
            }
        });
    });
}

struct Widgets<'a> {
    app: &'a mut App,
    q: &'a Project,
    root: &'a Path,
    p: Palette,
    /// A widget whose tab menu said "Remove widget" this frame.
    remove: Option<Widget>,
}

impl TabViewer for Widgets<'_> {
    type Tab = Widget;

    fn id(&mut self, w: &mut Widget) -> Id {
        Id::new(("widget", self.q.id, *w))
    }

    fn title(&mut self, w: &mut Widget) -> WidgetText {
        RichText::new(w.title()).size(12.5).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, w: &mut Widget) {
        let (app, q, root, p) = (&mut *self.app, self.q, self.root, self.p);
        match *w {
            Widget::Repository => crate::project::repo_card(ui, q, p),
            Widget::Todos => crate::project::todos_card(app, ui, q, root, p),
            Widget::Roadmap => crate::project::roadmap_card(app, ui, q, root, p),
            Widget::Languages => {
                let langs = q.info().map(|i| i.languages.clone()).unwrap_or_default();
                crate::project::languages_card(app, ui, q.id, root, &langs, p);
            }
            Widget::Commits => commits(app, ui, q, root, p),
            Widget::Sessions => sessions(app, ui, q, p),
            Widget::Disk => disk(app, ui, q, root, p),
            Widget::TodoHistory => crate::progress_ui::todo_widget(app, ui, q),
            Widget::RoadmapHistory => crate::progress_ui::roadmap_widget(app, ui, q),
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, w: &mut Widget, _path: egui_dock::NodePath) {
        if ui.button(format!("{}  Remove widget", icon::X)).clicked() {
            self.remove = Some(*w);
            ui.close();
        }
    }

    fn on_close(&mut self, _w: &mut Widget) -> OnCloseResponse {
        OnCloseResponse::Close
    }

    fn allowed_in_windows(&self, _w: &mut Widget) -> bool {
        false
    }
}

/// A spinner while reading, else a Recount button; true when clicked.
fn recount(ui: &mut egui::Ui, reading: bool) -> bool {
    let mut again = false;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if reading {
                ui.spinner();
            } else {
                again = theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Read again").clicked();
            }
        });
    });
    again
}

fn commits(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    if let Entry::Vacant(e) = app.dashboard.commits.entry(q.id) {
        e.insert(None);
        let (id, root) = (q.id, root.to_path_buf());
        let home = pa_core::claude::home(&app.store).ok();
        let folders: Vec<String> = q.checkouts.iter().map(|c| c.path.clone()).collect();
        app.background(ui.ctx(), move || {
            let sessions = home.map_or_else(|| vec![0; WEEKS], |h| pa_core::history::sessions_per_week(&h, &folders, WEEKS));
            let c = (pa_core::history::commits_per_week(&root, WEEKS), sessions);
            Box::new(move |app: &mut App| {
                app.dashboard.commits.insert(id, Some(c));
            }) as Job
        });
    }
    let data = app.dashboard.commits.get(&q.id).cloned().flatten();
    if recount(ui, data.is_none()) {
        app.dashboard.commits.remove(&q.id);
    }
    if let Some((c, s)) = data {
        let h = (ui.available_height() - 50.0).clamp(80.0, 220.0);
        crate::chart::weekly_bars(ui, &[("Commits", p.blue, &c), ("Claude sessions", p.purple, &s)], h, p);
    }
}

fn sessions(app: &mut App, ui: &mut egui::Ui, q: &Project, p: Palette) {
    if let Entry::Vacant(e) = app.dashboard.sessions.entry(q.id) {
        e.insert(None);
        let id = q.id;
        let home = pa_core::claude::home(&app.store).ok();
        let folders: Vec<String> = q.checkouts.iter().map(|c| c.path.clone()).collect();
        app.background(ui.ctx(), move || {
            let s = home.map(|h| pa_core::claude::recent_sessions(&h, &folders, SESSIONS)).unwrap_or_default();
            Box::new(move |app: &mut App| {
                app.dashboard.sessions.insert(id, Some(s));
            }) as Job
        });
    }
    let data = app.dashboard.sessions.get(&q.id).cloned().flatten();
    if recount(ui, data.is_none()) {
        app.dashboard.sessions.remove(&q.id);
    }
    let Some(list) = data else { return };
    if list.is_empty() {
        ui.label(RichText::new("No Claude Code sessions in this project's folders.").color(p.dim));
    }
    for s in &list {
        ui.horizontal(|ui| {
            ui.add_sized([56.0, 18.0], egui::Label::new(caps(crate::chart::day_label(s.secs.div_euclid(86_400)), p.faint)));
            let text = if s.prompt.is_empty() { RichText::new("(no prompt)").color(p.faint) } else { RichText::new(&s.prompt) };
            ui.add(egui::Label::new(text).truncate()).on_hover_text(&s.prompt);
        });
    }
}

/// `1.2 GB` and the like.
fn size(bytes: u64) -> String {
    let mut v = bytes as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if v < 1024.0 {
            return if unit == "B" { format!("{bytes} B") } else { format!("{v:.1} {unit}") };
        }
        v /= 1024.0;
    }
    format!("{v:.1} TB")
}

fn disk(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    if let Entry::Vacant(e) = app.dashboard.disk.entry(q.id) {
        e.insert(None);
        let (id, root): (i64, PathBuf) = (q.id, root.to_path_buf());
        app.background(ui.ctx(), move || {
            let d = pa_core::info::disk_usage(&root);
            Box::new(move |app: &mut App| {
                app.dashboard.disk.insert(id, Some(d));
            }) as Job
        });
    }
    let data = app.dashboard.disk.get(&q.id).copied().flatten();
    if recount(ui, data.is_none()) {
        app.dashboard.disk.remove(&q.id);
    }
    let Some((total, bulky)) = data else {
        ui.label(RichText::new("Measuring…").color(p.faint));
        return;
    };
    ui.label(RichText::new(size(total)).font(theme::semibold(22.0)));
    ui.label(RichText::new("this checkout, everything included").color(p.dim));
    ui.add_space(8.0);
    let source = total - bulky.iter().sum::<u64>();
    let rows = std::iter::once(("everything else", source, p.blue)).chain(BULKY.iter().zip(bulky).zip([p.orange, p.green, p.purple]).map(|((n, b), c)| (*n, b, c)));
    for (name, bytes, color) in rows.filter(|r| r.1 > 0) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, color);
            ui.label(RichText::new(name).color(p.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(size(bytes)).monospace()));
        });
    }
}
