//! Command palette (Ctrl+K, after YouTrack's commands): type to find a page, a project, a project's section or an
//! action; Enter runs it. Every word typed must appear in the entry, in any order and case. Also the app's other
//! shortcuts: Ctrl+Shift+K capture, F5 refresh all.

use crate::theme::{self, caps, pal};
use crate::{App, LAUNCHERS, Tab};
use egui::{Key, KeyboardShortcut, Modifiers, RichText, vec2};
use egui_phosphor::regular as icon;

pub const SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
/// Rows shown at once.
const ROWS: usize = 15;

#[derive(Default)]
pub struct Palette {
    query: String,
    selected: usize,
}

#[derive(Clone)]
enum Cmd {
    Tab(Tab),
    Section(i64, &'static str),
    Capture,
    Refresh,
    NewProject,
    Theme(&'static str),
    Launch(i64, &'static str, &'static str),
    Search(String),
}

struct Entry {
    label: String,
    /// Its keyboard shortcut, shown as a key cap on the right.
    keys: Option<&'static str>,
    cmd: Cmd,
}

impl Entry {
    fn new(label: impl Into<String>, cmd: Cmd) -> Entry {
        Entry { label: label.into(), keys: None, cmd }
    }

    /// The heading it is listed under, and that heading's place in the list.
    fn group(&self) -> (usize, &'static str) {
        match self.cmd {
            Cmd::Launch(..) => (0, "This project"),
            Cmd::Tab(Tab::Project(_)) => (2, "Projects"),
            Cmd::Tab(_) => (1, "Pages"),
            Cmd::Section(..) => (3, "Sections"),
            Cmd::Search(_) => (4, "Saved searches"),
            Cmd::Capture | Cmd::Refresh | Cmd::NewProject | Cmd::Theme(_) => (5, "Actions"),
        }
    }

    fn icon(&self) -> &'static str {
        match self.cmd {
            Cmd::Tab(Tab::Overview) => icon::SQUARES_FOUR,
            Cmd::Tab(Tab::Todos) => icon::LIST_CHECKS,
            Cmd::Tab(Tab::Board) => icon::KANBAN,
            Cmd::Tab(Tab::Reports) => icon::CHART_LINE,
            Cmd::Tab(Tab::Claude) => icon::ROBOT,
            Cmd::Tab(Tab::Templates) => icon::STACK,
            Cmd::Tab(Tab::Settings) => icon::GEAR,
            Cmd::Tab(Tab::Import) => icon::DOWNLOAD_SIMPLE,
            Cmd::Tab(_) => icon::FOLDER,
            Cmd::Section(..) => icon::ARROW_RIGHT,
            Cmd::Capture => icon::LIGHTBULB,
            Cmd::Refresh => icon::ARROWS_CLOCKWISE,
            Cmd::NewProject => icon::PLUS,
            Cmd::Theme(_) => icon::CIRCLE_HALF,
            Cmd::Launch(..) => icon::ARROW_SQUARE_OUT,
            Cmd::Search(_) => icon::BOOKMARK_SIMPLE,
        }
    }
}

const PAGES: [(&str, Tab); 8] = [
    ("Projects", Tab::Overview),
    ("TODOs", Tab::Todos),
    ("Board: now, next, later across projects", Tab::Board),
    ("Reports: progress and activity per project", Tab::Reports),
    ("Import", Tab::Import),
    ("Claude", Tab::Claude),
    ("Templates", Tab::Templates),
    ("Settings", Tab::Settings),
];
const SECTIONS: [(&str, &str); 12] = [
    ("Overview (dashboard)", "overview"),
    ("TODOs", "todos"),
    ("Roadmap", "roadmap"),
    ("TODOs in code", "code"),
    ("Notes", "notes"),
    ("Git", "git"),
    ("GitHub", "github"),
    ("Tasks", "tasks"),
    ("Dependencies", "deps"),
    ("Claude", "claude"),
    ("Checkouts", "checkouts"),
    ("Settings", "settings"),
];

/// Everything the palette offers. Without a query: pages, actions and the open project's sections and launchers;
/// with one, also every project and every project's sections.
fn entries(app: &App, active: Option<Tab>, all: bool) -> Vec<Entry> {
    let mut v: Vec<Entry> = Vec::new();
    let current = match active {
        Some(Tab::Project(id)) => app.projects.iter().find(|q| q.id == id),
        _ => None,
    };
    // Order matters without a query: the first ROWS are what the palette opens with.
    if let Some(q) = current {
        for &(key, label, default) in LAUNCHERS {
            v.push(Entry::new(format!("Open {}  in {label}", q.name), Cmd::Launch(q.id, key, default)));
        }
    }
    for (label, tab) in PAGES {
        v.push(Entry::new(label, Cmd::Tab(tab)));
    }
    v.push(Entry { keys: Some("Ctrl+Shift+K"), ..Entry::new("Capture an idea, bug, TODO or note", Cmd::Capture) });
    v.push(Entry { keys: Some("F5"), ..Entry::new("Refresh all projects", Cmd::Refresh) });
    v.push(Entry::new("New project from a template", Cmd::NewProject));
    for (name, query) in crate::todos_page::saved(app) {
        v.push(Entry::new(format!("Search: {name}"), Cmd::Search(query.clone())));
    }
    if let Some(q) = current {
        for (label, key) in SECTIONS {
            v.push(Entry::new(format!("{label}  ·  {}", q.name), Cmd::Section(q.id, key)));
        }
    }
    for (label, pref) in [("Theme: dark", "dark"), ("Theme: light", "light"), ("Theme: follow the system", "system")] {
        v.push(Entry::new(label, Cmd::Theme(pref)));
    }
    if all {
        for q in &app.projects {
            v.push(Entry::new(q.name.clone(), Cmd::Tab(Tab::Project(q.id))));
            if current.is_some_and(|c| c.id == q.id) {
                continue;
            }
            for (label, key) in SECTIONS.iter().skip(1) {
                v.push(Entry::new(format!("{label}  ·  {}", q.name), Cmd::Section(q.id, key)));
            }
        }
    }
    v
}

/// The entries every word of `query` appears in, those starting with the query first.
fn matches(entries: Vec<Entry>, query: &str) -> Vec<Entry> {
    let q = query.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<Entry> = entries.into_iter().filter(|e| words.iter().all(|w| e.label.to_lowercase().contains(w))).collect();
    // Grouped under their headings; within a group, those starting with the query first.
    hits.sort_by_key(|e| (e.group().0, !e.label.to_lowercase().starts_with(&q)));
    hits
}

pub fn open(app: &mut App) {
    app.palette = Some(Palette::default());
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    // After capture's window: Ctrl+K would also match Ctrl+Shift+K.
    if ctx.input_mut(|i| i.consume_shortcut(&SHORTCUT)) {
        app.palette = if app.palette.is_some() { None } else { Some(Palette::default()) };
    }
    if app.palette.is_none() && app.capture.is_none() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) {
        app.refresh_all(ctx);
    }
    let active = app.active_tab();
    let Some(query) = app.palette.as_ref().map(|s| s.query.clone()) else { return };
    let hits = matches(entries(app, active, !query.trim().is_empty()), &query);
    let p = pal(ctx);
    let Some(s) = app.palette.as_mut() else { return };
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.key_pressed(Key::Escape),
        )
    });
    let shown = hits.len().min(ROWS);
    if down && shown > 0 {
        s.selected = (s.selected + 1) % shown;
    }
    if up && shown > 0 {
        s.selected = (s.selected + shown - 1) % shown;
    }
    s.selected = s.selected.min(shown.saturating_sub(1));
    let mut run = enter.then(|| hits.get(s.selected).map(|e| e.cmd.clone())).flatten();
    egui::Modal::new(egui::Id::new("palette")).show(ctx, |ui| {
        let w = 560.0;
        ui.set_width(w);
        let r = ui.add(theme::field(&mut s.query, w).hint_text(format!("{}  Jump to a project, section or page, or run an action", icon::MAGNIFYING_GLASS)));
        r.request_focus();
        if r.changed() {
            s.selected = 0;
        }
        ui.add_space(6.0);
        let mut heading = None;
        for (k, e) in hits.iter().take(ROWS).enumerate() {
            let g = e.group().1;
            if heading != Some(g) {
                heading = Some(g);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(caps(g, p.faint));
                });
            }
            if row(ui, e, k == s.selected, w, p).clicked() {
                run = Some(e.cmd.clone());
            }
        }
        if hits.is_empty() {
            ui.label(RichText::new("Nothing matches.").color(p.dim));
        } else if hits.len() > ROWS {
            ui.label(RichText::new(format!("and {} more: type more to narrow it down", hits.len() - ROWS)).size(12.0).color(p.faint));
        }
        ui.add_space(6.0);
        ui.label(caps("Ctrl+K palette  ·  Ctrl+Shift+K capture  ·  F5 refresh  ·  Esc close", p.faint));
    });
    if esc {
        app.palette = None;
    }
    if let Some(cmd) = run {
        app.palette = None;
        execute(app, ctx, active, cmd);
    }
}

/// One result: icon, label, the shortcut as a key cap on the right; the selected one filled, with a bar on its left.
fn row(ui: &mut egui::Ui, e: &Entry, selected: bool, w: f32, p: theme::Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &e.label));
    let h = theme::hover_t(ui, &resp);
    let painter = ui.painter();
    let fill = if selected { p.hover } else { egui::Color32::TRANSPARENT.lerp_to_gamma(p.hover, h) };
    painter.rect_filled(rect, 6, fill);
    if selected {
        let bar = egui::Rect::from_center_size(egui::pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 18.0));
        painter.rect_filled(bar, 2, p.accent);
    }
    let y = rect.center().y;
    let fg = if selected { p.text } else { p.dim };
    painter.text(egui::pos2(rect.left() + 12.0, y), egui::Align2::LEFT_CENTER, e.icon(), egui::FontId::proportional(15.0), fg);
    let mut right = rect.right() - 10.0;
    if let Some(keys) = e.keys {
        let g = painter.layout_no_wrap(keys.to_string(), egui::FontId::monospace(10.5), p.dim);
        let cap = egui::Rect::from_min_size(egui::pos2(right - g.size().x - 12.0, y - g.size().y / 2.0 - 3.0), g.size() + vec2(12.0, 6.0));
        painter.rect_stroke(cap, 4, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        painter.galley(cap.min + vec2(6.0, 3.0), g, p.dim);
        right = cap.left() - 8.0;
    }
    let font = if selected { theme::semibold(13.5) } else { egui::FontId::proportional(13.5) };
    let label = painter.layout_no_wrap(e.label.clone(), font, p.text);
    let clip = egui::Rect::from_min_max(egui::pos2(rect.left() + 38.0, rect.top()), egui::pos2(right, rect.bottom()));
    painter.with_clip_rect(clip).galley(egui::pos2(clip.left(), y - label.size().y / 2.0), label, p.text);
    resp
}

fn execute(app: &mut App, ctx: &egui::Context, active: Option<Tab>, cmd: Cmd) {
    match cmd {
        Cmd::Tab(tab) => {
            if tab == Tab::Overview {
                app.overview.set_group(None);
            }
            app.open.push(tab);
        }
        Cmd::Section(id, key) => {
            app.project.set_section(id, key);
            app.open.push(Tab::Project(id));
        }
        Cmd::Capture => crate::capture::open(app, active),
        Cmd::Refresh => app.refresh_all(ctx),
        Cmd::NewProject => crate::templates_ui::open_new(app, None),
        Cmd::Theme(pref) => {
            let r = app.store.set_setting("theme", pref);
            app.ok(r);
            theme::apply(ctx, pref);
        }
        Cmd::Search(query) => crate::todos_page::apply(app, &query),
        Cmd::Launch(id, key, default) => {
            if let Some(path) = app.projects.iter().find(|q| q.id == id).and_then(|q| q.primary()).map(|c| c.path.clone()) {
                app.launch(key, default, &path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(label: &str) -> Entry {
        Entry::new(label, Cmd::Refresh)
    }

    #[test]
    fn hits_are_grouped_pages_before_projects_before_actions() {
        let all = vec![Entry::new("Board refresh", Cmd::Refresh), Entry::new("Board-tool", Cmd::Tab(Tab::Project(1))), Entry::new("Board", Cmd::Tab(Tab::Board))];
        let groups: Vec<&str> = matches(all, "board").iter().map(|e| e.group().1).collect();
        assert_eq!(groups, ["Pages", "Projects", "Actions"]);
    }

    #[test]
    fn every_word_must_match_and_prefix_hits_come_first() {
        let all = || vec![e("Roadmap  ·  trailmix"), e("Git  ·  trailmix-web"), e("trailmix"), e("Settings")];
        let labels = |q: &str| matches(all(), q).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels("trailmix"), ["trailmix", "Roadmap  ·  trailmix", "Git  ·  trailmix-web"]);
        assert_eq!(labels("web GIT"), ["Git  ·  trailmix-web"]);
        assert!(labels("nothing").is_empty());
    }
}
