//! The overview (SPEC § 5): purpose tabs with counts, search, filters, and every project as a tall table row
//! (ArtCraft launcher) or a card (DockHand). Clicking a project opens its tab.

use crate::theme::{self, Palette, avatar, caps, chip, lang_dot, pal, relative, tag};
use crate::{App, LAUNCHERS, Tab};
use egui::{Align, Color32, Layout, Rect, RichText, Sense, Stroke, UiBuilder, pos2, vec2};
use egui_phosphor::regular as icon;
use pa_core::{Project, Status, info::github_url};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Grouping {
    #[default]
    None,
    Group,
    Purpose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Table,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Col {
    #[default]
    Name,
    Status,
    Languages,
    Version,
    Git,
    Todos,
    Updated,
}

/// Setting: favourite project ids, comma-separated, in order.
pub const FAVOURITES: &str = "projects.favourites";
/// Projects with no commit for this many days count as stale.
const STALE_DAYS: i64 = 180;
const ROW_H: f32 = 62.0;
const CARD_W: f32 = 340.0;
const CARD_H: f32 = 214.0;

#[derive(Default)]
pub struct State {
    search: String,
    /// Purpose tab: `None` = all; `Some(None)` = without a purpose.
    purpose: Option<Option<i64>>,
    /// `None` = any; `Some(None)` = without a group. Set from the sidebar.
    group: Option<Option<i64>>,
    status: Option<Status>,
    language: Option<String>,
    changes: bool,
    not_on_github: bool,
    stale: bool,
    show_hidden: bool,
    grouping: Grouping,
    view: View,
    sort: Col,
    descending: bool,
    /// Favourite project ids in the user's order (`projects.favourites`).
    favourites: Vec<i64>,
}

impl State {
    pub fn group_filter(&self) -> Option<Option<i64>> {
        self.group
    }

    /// Restore the remembered table/grid choice.
    pub fn set_view(&mut self, setting: &str) {
        self.view = if setting == "grid" { View::Grid } else { View::Table };
    }

    /// Restore the favourites from their setting (ids, comma-separated).
    pub fn set_favourites(&mut self, setting: &str) {
        self.favourites = setting.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    }

    pub fn favourites(&self) -> &[i64] {
        &self.favourites
    }

    pub fn set_group(&mut self, g: Option<Option<i64>>) {
        self.group = g;
    }

    fn active_filters(&self) -> usize {
        [self.status.is_some(), self.language.is_some(), self.changes, self.not_on_github, self.stale, self.show_hidden]
            .iter()
            .filter(|b| **b)
            .count()
    }
}

enum Action {
    Open(i64),
    /// Add or remove a favourite.
    Favourite(i64),
    /// Move the favourite at the first index to the second.
    MoveFavourite(usize, usize),
    Sort(Col),
    Launch(String, &'static str, &'static str),
    Purpose(i64, Option<i64>),
    Status(i64, Status),
    Remove(i64),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    if app.projects.is_empty() {
        empty(app, ui, p);
        return;
    }
    toolbar(app, ui, p);
    ui.add_space(10.0);
    let rows = sorted_rows(app);
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // Text in rows and cards must not take the click that opens the project.
        ui.style_mut().interaction.selectable_labels = false;
        if rows.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("No projects match these filters.").color(p.dim)));
        }
        for (label, rows) in sections(app, &rows) {
            if let Some(label) = label {
                ui.add_space(6.0);
                ui.label(caps(format!("{label}   {}", rows.len()), p.dim));
                ui.add_space(4.0);
            }
            match app.overview.view {
                View::Table => table(app, ui, &rows, &mut actions, p),
                View::Grid => grid(app, ui, &rows, &mut actions, p),
            }
            ui.add_space(14.0);
        }
    });
    apply(app, actions);
}

fn empty(app: &mut App, ui: &mut egui::Ui, p: Palette) {
    ui.add_space(80.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon::FOLDERS).size(48.0).color(p.faint));
        ui.add_space(8.0);
        ui.heading("No projects yet");
        ui.label(RichText::new("Add a scan root such as your Code folder and scan it, or add a single folder.").color(p.dim));
        ui.add_space(12.0);
        if theme::primary(ui, format!("{}  Import projects", icon::DOWNLOAD_SIMPLE)).clicked() {
            app.open.push(Tab::Import);
        }
    });
}

fn favs(app: &App) -> &[i64] {
    &app.overview.favourites
}

/// Drag payload of a sidebar favourite: its index.
pub(crate) struct FavDrag(pub usize);

/// Move a favourite (the sidebar's drag and drop) and save the order.
pub(crate) fn move_favourite(app: &mut App, from: usize, to: usize) {
    apply(app, vec![Action::MoveFavourite(from, to)]);
}

fn apply(app: &mut App, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Open(id) => app.open.push(Tab::Project(id)),
            Action::Favourite(_) | Action::MoveFavourite(..) => {
                let f = &mut app.overview.favourites;
                match a {
                    Action::Favourite(id) if f.contains(&id) => f.retain(|x| *x != id),
                    Action::Favourite(id) => f.push(id),
                    Action::MoveFavourite(from, to) if from < f.len() => {
                        let x = f.remove(from);
                        f.insert(to.min(f.len()), x);
                    }
                    _ => {}
                }
                let text = f.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
                let r = app.store.set_setting(FAVOURITES, &text);
                app.ok(r);
            }
            Action::Sort(c) => {
                let s = &mut app.overview;
                s.descending = s.sort == c && !s.descending;
                s.sort = c;
            }
            Action::Launch(path, key, default) => app.launch(key, default, &path),
            Action::Purpose(id, purpose) => edit(app, id, |p| p.purpose_id = purpose),
            Action::Status(id, status) => edit(app, id, |p| p.status = status),
            Action::Remove(id) => {
                let r = app.store.remove_project(id);
                app.ok(r);
                app.reload();
            }
        }
    }
}

fn edit(app: &mut App, id: i64, f: impl FnOnce(&mut Project)) {
    if let Some(mut p) = app.projects.iter().find(|p| p.id == id).cloned() {
        f(&mut p);
        app.save_project(p);
    }
}

/// Purpose tabs with counts, then search, filters, grouping and the table/grid switch on the right.
fn toolbar(app: &mut App, ui: &mut egui::Ui, p: Palette) {
    // Counts respect every filter except the purpose tab itself.
    let saved = app.overview.purpose.take();
    let base: Vec<&Project> = app.projects.iter().filter(|q| matches(app, q)).collect();
    app.overview.purpose = saved;
    let mut tabs: Vec<(String, Option<usize>, Option<Option<i64>>)> = vec![("All".into(), Some(base.len()), None)];
    for u in &app.purposes {
        let n = base.iter().filter(|q| q.purpose_id == Some(u.id)).count();
        if n > 0 || app.overview.purpose == Some(Some(u.id)) {
            tabs.push((u.name.clone(), Some(n), Some(Some(u.id))));
        }
    }
    let none = base.iter().filter(|q| q.purpose_id.is_none()).count();
    if none > 0 {
        tabs.push(("No purpose".into(), Some(none), Some(None)));
    }
    ui.horizontal_wrapped(|ui| {
        let items: Vec<(String, Option<usize>)> = tabs.iter().map(|t| (t.0.clone(), t.1)).collect();
        let selected = tabs.iter().position(|t| t.2 == app.overview.purpose).unwrap_or(0);
        if let Some(i) = theme::segmented(ui, &items, selected) {
            app.overview.purpose = tabs[i].2;
        }
        if let Some(g) = app.overview.group {
            let name = if g.is_none() { "No group".to_string() } else { app.group_name(g).to_string() };
            if theme::secondary(ui, format!("{}  {name}  {}", icon::FOLDERS, icon::X)).on_hover_text("Clear the group filter").clicked() {
                app.overview.group = None;
            }
        }
        theme::right(ui, "overview.rs:234", egui::Align::Center, |ui| {
            let s = &mut app.overview;
            let views = [("Grid".to_string(), None), ("Table".to_string(), None)];
            if let Some(i) = theme::segmented(ui, &views, if s.view == View::Grid { 0 } else { 1 }) {
                s.view = if i == 0 { View::Grid } else { View::Table };
                let r = app.store.set_setting("overview.view", if i == 0 { "grid" } else { "table" });
                app.ok(r);
            }
            filters_menu(app, ui, p);
            if theme::secondary(ui, format!("{}  Add projects", icon::PLUS)).on_hover_text("Scan folders or add one (Import)").clicked() {
                app.open.push(Tab::Import);
            }
            ui.add(
                theme::field(&mut app.overview.search, 240.0).hint_text(format!("{}  Search projects", icon::MAGNIFYING_GLASS)),
            );
        });
    });
}

fn filters_menu(app: &mut App, ui: &mut egui::Ui, p: Palette) {
    let n = app.overview.active_filters();
    let label = if n > 0 { format!("{}  Filters · {n}", icon::FUNNEL) } else { format!("{}  Filters", icon::FUNNEL) };
    let mut languages: Vec<String> =
        app.projects.iter().filter_map(|q| q.info()).flat_map(|i| i.languages.iter().take(3).map(|l| l.0.clone())).collect();
    languages.sort();
    languages.dedup();
    let s = &mut app.overview;
    ui.menu_button(RichText::new(label).size(13.0), |ui| {
        ui.set_min_width(240.0);
        ui.label(caps("Status", p.faint));
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut s.status, None, "Any");
            for st in Status::ALL {
                ui.selectable_value(&mut s.status, Some(st), st.as_str());
            }
        });
        ui.add_space(4.0);
        ui.label(caps("Language", p.faint));
        theme::select(ui, "f_lang", 220.0, s.language.clone().unwrap_or("Any language".into()), |ui| {
            ui.selectable_value(&mut s.language, None, "Any language");
            for l in &languages {
                ui.selectable_value(&mut s.language, Some(l.clone()), l);
            }
        });
        ui.add_space(4.0);
        ui.label(caps("Show only", p.faint));
        theme::checkbox(ui, &mut s.changes, "With local changes");
        theme::checkbox(ui, &mut s.not_on_github, "Not on GitHub");
        theme::checkbox(ui, &mut s.stale, format!("Stale ({STALE_DAYS}+ days without a commit)"));
        theme::checkbox(ui, &mut s.show_hidden, "Include hidden purposes (e.g. Archive)");
        ui.add_space(4.0);
        ui.label(caps("Group by", p.faint));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.grouping, Grouping::None, "Nothing");
            ui.selectable_value(&mut s.grouping, Grouping::Group, "Group");
            ui.selectable_value(&mut s.grouping, Grouping::Purpose, "Purpose");
        });
        ui.add_space(4.0);
        if ui.button("Reset filters").clicked() {
            (s.status, s.language, s.changes, s.not_on_github, s.stale, s.show_hidden) = (None, None, false, false, false, false);
        }
    });
}

fn matches(app: &App, p: &Project) -> bool {
    let s = &app.overview;
    let info = p.info();
    let git = info.and_then(|i| i.git.as_ref());
    // A purpose tab shows its projects even when the purpose is hidden from the overview.
    if !s.show_hidden && s.purpose.is_none() && app.purpose(p.purpose_id).is_some_and(|u| !u.show_in_overview) {
        return false;
    }
    let q = s.search.trim().to_lowercase();
    if !q.is_empty()
        && !p.name.to_lowercase().contains(&q)
        && !p.tags.iter().any(|t| t.to_lowercase().contains(&q))
        && !p.checkouts.iter().any(|c| c.path.to_lowercase().contains(&q))
        && !info.and_then(|i| i.description.as_deref()).is_some_and(|d| d.to_lowercase().contains(&q))
    {
        return false;
    }
    if s.purpose.is_some_and(|u| u != p.purpose_id) || s.group.is_some_and(|g| g != p.group_id) || s.status.is_some_and(|st| st != p.status) {
        return false;
    }
    if let Some(l) = &s.language
        && !info.is_some_and(|i| i.languages.iter().take(3).any(|x| &x.0 == l))
    {
        return false;
    }
    if s.changes && !git.is_some_and(|g| g.dirty > 0) {
        return false;
    }
    if s.not_on_github && git.and_then(|g| g.remote.as_deref()).and_then(github_url).is_some() {
        return false;
    }
    if s.stale {
        let last = git.and_then(|g| g.last_commit.as_deref()).and_then(pa_core::iso_days);
        if last.is_some_and(|d| pa_core::today_days() - d < STALE_DAYS) {
            return false;
        }
    }
    true
}

fn sorted_rows(app: &App) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..app.projects.len()).filter(|&i| matches(app, &app.projects[i])).collect();
    let key = |p: &Project| -> String {
        let info = p.info();
        let git = info.and_then(|i| i.git.as_ref());
        match app.overview.sort {
            Col::Name => p.name.to_lowercase(),
            Col::Status => app.purpose(p.purpose_id).map_or("~".into(), |u| format!("{:05}{}", u.sort, p.status.as_str())),
            Col::Languages => info.and_then(|i| i.languages.first()).map_or("~".into(), |l| l.0.clone()),
            Col::Version => info.and_then(|i| i.version.clone()).unwrap_or("~".into()),
            Col::Git => format!("{:08}", git.map_or(0, |g| g.dirty)),
            Col::Todos => format!("{:08}", info.and_then(|i| i.todo_open).unwrap_or(0)),
            Col::Updated => git.and_then(|g| g.last_commit.clone()).unwrap_or_default(),
        }
    };
    rows.sort_by_cached_key(|&i| (key(&app.projects[i]), app.projects[i].name.to_lowercase()));
    if app.overview.descending {
        rows.reverse();
    }
    rows
}

/// Rows split by the grouping; one unnamed section when not grouping.
fn sections(app: &App, rows: &[usize]) -> Vec<(Option<String>, Vec<usize>)> {
    let g = app.overview.grouping;
    if g == Grouping::None {
        return vec![(None, rows.to_vec())];
    }
    let mut out: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for &i in rows {
        let p = &app.projects[i];
        let label = match g {
            Grouping::Group => app.group_name(p.group_id).to_string(),
            _ => app.purpose(p.purpose_id).map_or(String::new(), |u| u.name.clone()),
        };
        let label = Some(if label.is_empty() { "None".to_string() } else { label });
        match out.iter_mut().find(|(l, _)| *l == label) {
            Some((_, v)) => v.push(i),
            None => out.push((label, vec![i])),
        }
    }
    out.sort_by_key(|(l, _)| (l.as_deref() == Some("None"), l.clone().unwrap_or_default().to_lowercase()));
    out
}

// --- table ----------------------------------------------------------------------------------------------------

/// Right-hand columns: id, header, width. Name takes the rest and keeps at least `NAME_MIN`: narrow panes drop
/// Languages, then TODOs, Version, Updated and Status, in that order.
fn columns(width: f32) -> Vec<(Col, &'static str, f32)> {
    let mut cols = vec![
        (Col::Status, "Status", 180.0),
        (Col::Languages, "Languages", 180.0),
        (Col::Version, "Version", 80.0),
        (Col::Git, "Git", 190.0),
        (Col::Todos, "TODOs", 64.0),
        (Col::Updated, "Updated", 96.0),
    ];
    for drop in [Col::Languages, Col::Todos, Col::Version, Col::Updated, Col::Status] {
        let name_w = width - 32.0 - ACTIONS_W - cols.iter().map(|c| c.2).sum::<f32>();
        if name_w >= NAME_MIN {
            break;
        }
        cols.retain(|c| c.0 != drop);
    }
    cols
}

const ACTIONS_W: f32 = 72.0;
const NAME_MIN: f32 = 300.0;

fn table(app: &App, ui: &mut egui::Ui, rows: &[usize], actions: &mut Vec<Action>, p: Palette) {
    let width = ui.available_width();
    let cols = columns(width);
    let name_w = (width - 32.0 - ACTIONS_W - cols.iter().map(|c| c.2).sum::<f32>()).max(160.0);
    egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).corner_radius(8).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // Header
        let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
        ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));
        let mut x = rect.left() + 16.0;
        let mut header = |ui: &mut egui::Ui, col: Col, label: &str, w: f32| {
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
            let arrow = match (app.overview.sort == col, app.overview.descending) {
                (false, _) => "",
                (true, false) => " ↑",
                (true, true) => " ↓",
            };
            let mut child = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(Align::Center)));
            let resp = theme::button(&mut child, &format!("{label}{arrow}").to_uppercase(), egui::FontId::monospace(10.5), 1.1, theme::Kind::Ghost, vec2(0.0, 26.0));
            if resp.clicked() {
                actions.push(Action::Sort(col));
            }
            x += w;
        };
        header(ui, Col::Name, "Project", name_w);
        for &(c, label, w) in &cols {
            header(ui, c, label, w);
        }
        for (n, &i) in rows.iter().enumerate() {
            row(app, ui, &app.projects[i], width, name_w, &cols, n + 1 == rows.len(), actions, p);
        }
    });
}

fn row(app: &App, ui: &mut egui::Ui, q: &Project, width: f32, name_w: f32, cols: &[(Col, &str, f32)], last: bool, actions: &mut Vec<Action>, p: Palette) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    let t = theme::hover_t(ui, &resp);
    if t > 0.0 {
        ui.painter().rect_filled(rect, 0, p.surface.lerp_to_gamma(p.hover, t));
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !last {
        ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));
    }
    if resp.clicked() {
        actions.push(Action::Open(q.id));
    }
    resp.context_menu(|ui| context_menu(app, ui, q, actions));
    let info = q.info();
    let git = info.and_then(|i| i.git.as_ref());
    let mut x = rect.left() + 16.0;
    let mut cell = |ui: &mut egui::Ui, w: f32, f: &mut dyn FnMut(&mut egui::Ui)| {
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w - 12.0, rect.height()));
        let mut child = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(Align::Center)));
        child.set_clip_rect(r.intersect(ui.clip_rect()));
        f(&mut child);
        x += w;
    };
    cell(ui, name_w, &mut |ui| name_cell(ui, q, p, 36.0));
    for &(c, _, w) in cols {
        cell(ui, w, &mut |ui| match c {
            Col::Status => status_chips(app, ui, q, p),
            Col::Languages => {
                for (l, _) in info.map_or(&[][..], |i| &i.languages).iter().take(2) {
                    lang_dot(ui, l);
                }
            }
            Col::Version => {
                if let Some(v) = info.and_then(|i| i.version.as_deref()) {
                    ui.label(RichText::new(crate::widgets::version(v)).monospace().size(12.5)).on_hover_text(v);
                } else {
                    ui.label(RichText::new("—").color(p.faint));
                }
            }
            Col::Git => git_cell(ui, q, p),
            Col::Todos => match info.and_then(|i| i.todo_open) {
                Some(n) => {
                    ui.label(RichText::new(n.to_string()).monospace().color(if n > 0 { p.text } else { p.faint }));
                }
                None => {
                    ui.label(RichText::new("—").color(p.faint));
                }
            },
            Col::Updated => {
                let d = git.and_then(|g| g.last_commit.as_deref());
                ui.label(RichText::new(d.map(relative).unwrap_or("—".into())).size(13.0).color(p.dim)).on_hover_text(d.unwrap_or(""));
            }
            Col::Name => {}
        });
    }
    let r = Rect::from_min_size(pos2(rect.right() - ACTIONS_W - 4.0, rect.top()), vec2(ACTIONS_W - 8.0, rect.height()));
    let mut child = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::right_to_left(Align::Center)));
    // The row itself opens the project; launchers (VS Code, Explorer, …) are in the "…" menu.
    more_button(app, &mut child, q, actions);
    star(app, &mut child, q.id, resp.contains_pointer(), actions, p);
}

/// The favourite star: always shown on a favourite, else only while the row or card is hovered.
fn star(app: &App, ui: &mut egui::Ui, id: i64, hovered: bool, actions: &mut Vec<Action>, p: Palette) {
    let on = favs(app).contains(&id);
    let (color, tip) = if on { (p.orange, "Remove from favourites") } else { (if hovered { p.faint } else { Color32::TRANSPARENT }, "Add to favourites") };
    if theme::color_icon(ui, icon::STAR, color, tip).clicked() {
        actions.push(Action::Favourite(id));
    }
}

/// A pill naming where the repository's `origin` lives (GitHub, GitLab, ...); nothing for a local-only repository.
fn remote_pill(ui: &mut egui::Ui, q: &Project, p: Palette) {
    let Some(r) = q.info().and_then(|i| i.git.as_ref()).and_then(|g| g.remote.as_deref()) else { return };
    let lower = r.to_lowercase();
    let label = if lower.contains("github") {
        format!("{} GitHub", icon::GITHUB_LOGO)
    } else if lower.contains("gitlab") {
        format!("{} GitLab", icon::GITLAB_LOGO)
    } else {
        format!("{} remote", icon::CLOUD)
    };
    tag(ui, label, p.blue).on_hover_text(format!("origin: {r}"));
}

fn avatar_color(q: &Project) -> Color32 {
    q.info().and_then(|i| i.languages.first()).map_or_else(|| theme::hash_color(&q.name), |l| theme::lang_color(&l.0))
}

/// Avatar, name and a one-line subtitle (description, else group and folder).
fn name_cell(ui: &mut egui::Ui, q: &Project, p: Palette, size: f32) {
    avatar(ui, &q.name, avatar_color(q), size);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add_space((ROW_H - 40.0) / 2.0 - 2.0);
        let missing = q.info().is_some_and(|i| !i.exists);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(egui::Label::new(RichText::new(&q.name).font(theme::semibold(15.0)).color(if missing { p.red } else { p.text })).truncate());
            remote_pill(ui, q, p);
            if q.checkouts.len() > 1 {
                ui.label(RichText::new(format!("{} {}", icon::GIT_FORK, q.checkouts.len())).size(12.0).color(p.faint))
                    .on_hover_text(format!("{} checkouts", q.checkouts.len()));
            }
        });
        let path = q.primary().map_or(String::new(), |c| short_path(&c.path));
        // The description, else the folder in monospace faint so it reads as a path.
        let sub = if missing {
            RichText::new(format!("Folder missing: {path}")).size(12.5).color(p.dim)
        } else {
            match q.info().and_then(|i| i.description.clone()) {
                Some(d) => RichText::new(d).size(12.5).color(p.dim),
                None => RichText::new(path.clone()).monospace().size(12.0).color(p.faint),
            }
        };
        ui.add(egui::Label::new(sub).truncate()).on_hover_text(path);
    });
}

/// `C:\Users\me\Desktop\Code\Repos\x` → `~\Desktop\Code\Repos\x`.
pub fn short_path(path: &str) -> String {
    match std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        Ok(home) if !home.is_empty() && path.to_lowercase().starts_with(&home.to_lowercase()) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

pub(crate) fn status_color(s: Status, p: Palette) -> Color32 {
    match s {
        Status::Active => p.green,
        Status::Paused => p.orange,
        Status::Done => p.blue,
        Status::Abandoned => p.faint,
    }
}

pub fn purpose_color(app: &App, id: Option<i64>, p: Palette) -> Color32 {
    let palette = [p.blue, p.purple, p.orange, p.green, p.red];
    match app.purposes.iter().position(|u| Some(u.id) == id) {
        Some(i) => palette[i % palette.len()],
        None => p.faint,
    }
}

fn status_chips(app: &App, ui: &mut egui::Ui, q: &Project, p: Palette) {
    ui.spacing_mut().item_spacing.x = 6.0;
    if let Some(u) = app.purpose(q.purpose_id) {
        chip(ui, &u.name, purpose_color(app, q.purpose_id, p));
    }
    if q.status != Status::Active || q.purpose_id.is_none() {
        chip(ui, q.status.as_str(), status_color(q.status, p));
    }
    let running = app.runs.running(q.id);
    if running > 0 {
        chip(ui, format!("{} running", running), p.green).on_hover_text("Tasks are running (project tab → Tasks)");
    }
    if let Some(behind) = app.deps.behind.get(&q.id) {
        chip(ui, format!("{} deps", icon::WARNING), p.orange).on_hover_text(format!("A dependency is behind:\n{}", behind.join("\n")));
    }
}

fn git_cell(ui: &mut egui::Ui, q: &Project, p: Palette) {
    ui.spacing_mut().item_spacing.x = 6.0;
    let info = q.info();
    if info.is_some_and(|i| !i.exists) {
        chip(ui, "missing", p.red);
        return;
    }
    let Some(g) = info.and_then(|i| i.git.as_ref()) else {
        if info.is_some() {
            chip(ui, "no git", p.faint);
        }
        return;
    };
    let branch = g.branch.as_deref().unwrap_or("detached");
    ui.add(egui::Label::new(RichText::new(format!("{} {branch}", icon::GIT_BRANCH)).size(13.0).color(p.dim)).truncate()).on_hover_text(branch);
    if g.dirty > 0 {
        chip(ui, format!("{} changed", g.dirty), p.orange);
    }
    if let Some((a, b)) = g.ahead_behind
        && (a, b) != (0, 0)
    {
        chip(ui, crate::widgets::ahead_behind(Some((a, b))), p.blue);
    }
}

fn more_button(app: &App, ui: &mut egui::Ui, q: &Project, actions: &mut Vec<Action>) {
    ui.menu_button(RichText::new(icon::DOTS_THREE).size(16.0), |ui| context_menu(app, ui, q, actions)).response.on_hover_text("More");
}

fn context_menu(app: &App, ui: &mut egui::Ui, q: &Project, actions: &mut Vec<Action>) {
    if ui.button(format!("{}  Open project", icon::FOLDER_OPEN)).clicked() {
        actions.push(Action::Open(q.id));
    }
    let fav = if favs(app).contains(&q.id) { "Remove from favourites" } else { "Add to favourites" };
    if ui.button(format!("{}  {fav}", icon::STAR)).clicked() {
        actions.push(Action::Favourite(q.id));
    }
    if let Some(c) = q.primary() {
        for &(key, label, default) in LAUNCHERS {
            if ui.button(format!("{}  Open in {label}", icon::ARROW_SQUARE_OUT)).clicked() {
                actions.push(Action::Launch(c.path.clone(), key, default));
            }
        }
    }
    if let Some(url) = q.info().and_then(|i| i.git.as_ref()).and_then(|g| g.remote.as_deref()).and_then(github_url)
        && ui.button(format!("{}  Open on GitHub", icon::GITHUB_LOGO)).clicked()
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    ui.separator();
    ui.menu_button("Purpose", |ui| {
        if ui.button("—").clicked() {
            actions.push(Action::Purpose(q.id, None));
        }
        for u in &app.purposes {
            if ui.button(&u.name).clicked() {
                actions.push(Action::Purpose(q.id, Some(u.id)));
            }
        }
    });
    ui.menu_button("Status", |ui| {
        for s in Status::ALL {
            if ui.button(s.as_str()).clicked() {
                actions.push(Action::Status(q.id, s));
            }
        }
    });
    ui.separator();
    if ui.button(format!("{}  Remove from app", icon::TRASH)).on_hover_text("Forgets the project; its folders are not touched").clicked() {
        actions.push(Action::Remove(q.id));
    }
}

// --- grid -----------------------------------------------------------------------------------------------------

fn grid(app: &App, ui: &mut egui::Ui, rows: &[usize], actions: &mut Vec<Action>, p: Palette) {
    let gap = 14.0;
    let width = ui.available_width();
    let per_row = ((width + gap) / (CARD_W + gap)).floor().max(1.0) as usize;
    let card_w = (width - gap * (per_row - 1) as f32) / per_row as f32;
    for chunk in rows.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &i in chunk {
                card(app, ui, &app.projects[i], card_w, actions, p);
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

fn card(app: &App, ui: &mut egui::Ui, q: &Project, w: f32, actions: &mut Vec<Action>, p: Palette) {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, CARD_H), Sense::click());
    let t = theme::hover_t(ui, &resp);
    let (fill, stroke) = (p.surface.lerp_to_gamma(p.raised, t), p.border.lerp_to_gamma(p.faint, t));
    ui.painter().rect(rect, 10, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        actions.push(Action::Open(q.id));
    }
    resp.context_menu(|ui| context_menu(app, ui, q, actions));
    let info = q.info();
    let git = info.and_then(|i| i.git.as_ref());
    let inner = rect.shrink(16.0);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    let ui = &mut ui;
    ui.spacing_mut().item_spacing.y = 10.0;
    // Header: avatar, name, description; actions on the right.
    ui.horizontal(|ui| {
        avatar(ui, &q.name, avatar_color(q), 40.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.set_max_width(inner.width() - 40.0 - 100.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add(egui::Label::new(RichText::new(&q.name).font(theme::semibold(16.0))).truncate());
                remote_pill(ui, q, p);
            });
            let sub = info.and_then(|i| i.description.clone()).unwrap_or_else(|| q.primary().map_or(String::new(), |c| short_path(&c.path)));
            ui.add(egui::Label::new(RichText::new(sub).size(12.5).color(p.dim)).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            more_button(app, ui, q, actions);
            if let Some(c) = q.primary()
                && theme::icon_button(ui, icon::CODE, "Open in VS Code").clicked()
            {
                actions.push(Action::Launch(c.path.clone(), LAUNCHERS[0].0, LAUNCHERS[0].2));
            }
            star(app, ui, q.id, resp.contains_pointer(), actions, p);
        });
    });
    // Tags: purpose, status, group, own tags.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        status_chips(app, ui, q, p);
        if let Some(g) = app.groups.iter().find(|g| Some(g.id) == q.group_id) {
            tag(ui, &g.name, p.dim);
        }
        for t in q.tags.iter().take(3) {
            tag(ui, t, theme::hash_color(t));
        }
    });
    // Stats with icons.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let stat = |ui: &mut egui::Ui, glyph: &str, text: String, color: Color32, tip: &str| {
            ui.label(RichText::new(format!("{glyph} {text}")).size(13.0).color(color)).on_hover_text(tip);
        };
        match git {
            Some(g) => {
                stat(ui, icon::GIT_BRANCH, g.branch.clone().unwrap_or("detached".into()), p.dim, "Branch");
                let (n, c) = (g.dirty, if g.dirty > 0 { p.orange } else { p.faint });
                stat(ui, icon::GIT_DIFF, n.to_string(), c, "Changed files");
                let ab = crate::widgets::ahead_behind(g.ahead_behind);
                if !ab.is_empty() {
                    ui.label(RichText::new(ab).size(13.0).color(p.blue)).on_hover_text("Ahead / behind upstream");
                }
            }
            None => stat(ui, icon::FOLDER, "no git".into(), p.faint, "Not a git repository"),
        }
        if let Some(n) = info.and_then(|i| i.todo_open) {
            stat(ui, icon::LIST_CHECKS, n.to_string(), if n > 0 { p.text } else { p.faint }, "Open TODOs");
        }
    });
    // Languages.
    let langs = info.map_or(&[][..], |i| &i.languages);
    theme::lang_bar(ui, langs, inner.width());
    ui.horizontal(|ui| {
        for (l, _) in langs.iter().take(3) {
            lang_dot(ui, l);
        }
    });
    // Footer: version, last commit, GitHub, pinned to the card's bottom edge.
    let foot = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 22.0), inner.max);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(foot).layout(Layout::left_to_right(Align::Center)));
    if let Some(v) = info.and_then(|i| i.version.as_deref()) {
        ui.label(RichText::new(crate::widgets::version(v)).monospace().size(12.0).color(p.dim));
    }
    if let Some(d) = git.and_then(|g| g.last_commit.as_deref()) {
        ui.label(RichText::new(format!("{} {}", icon::CLOCK, relative(d))).size(12.5).color(p.faint));
    }
}
