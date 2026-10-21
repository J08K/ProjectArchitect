//! The TODOs page: every TODO-file and roadmap item of every project in one list, filtered by status, project,
//! group, purpose and source, and by a query (`pa_core::query`: words, `project:`, `in:`, `is:`, `kind:`,
//! `source:`, `has:issue`, `-` negates; completions as chips), sorted and grouped. The choices are remembered
//! (setting `todos.page`); queries can be saved by name (`todos.saved`) and show in the sidebar with their counts.
//! Items are read and written through the project tab's cache (`project::load_todo` / `edit_todo`), so both views
//! always agree and every write keeps the re-read check.

use crate::project::{edit_roadmap, edit_todo, load_roadmap, load_todo, open_at};
use crate::theme::{self, avatar, caps, card, pal, tag};
use crate::{App, Tab};
use egui::{Align, Layout, RichText, vec2};
use egui_phosphor::regular as icon;
use pa_core::TodoItem;
use pa_core::query::{Item, Query};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Show {
    #[default]
    Open,
    Done,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Sort {
    /// File order within each project.
    #[default]
    File,
    Text,
    Section,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum By {
    #[default]
    Project,
    Section,
    None,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Filters {
    show: Show,
    search: String,
    /// Empty = every project.
    projects: Vec<i64>,
    group: Option<i64>,
    purpose: Option<i64>,
    no_todo_file: bool,
    no_roadmap: bool,
    sort: Sort,
    by: By,
}

impl Filters {
    /// `key=value` pairs separated by `;` (values never contain either).
    fn encode(&self) -> String {
        let ids = |v: &[i64]| v.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let opt = |v: Option<i64>| v.map_or(String::new(), |v| v.to_string());
        format!(
            "show={:?};search={};projects={};group={};purpose={};no_todo={};no_roadmap={};sort={:?};by={:?}",
            self.show,
            self.search.replace([';', '='], " "),
            ids(&self.projects),
            opt(self.group),
            opt(self.purpose),
            self.no_todo_file,
            self.no_roadmap,
            self.sort,
            self.by
        )
    }

    /// Unknown keys and bad values fall back to the defaults.
    fn decode(s: &str) -> Filters {
        let mut f = Filters::default();
        for (k, v) in s.split(';').filter_map(|kv| kv.split_once('=')) {
            match k {
                "show" => f.show = [Show::Open, Show::Done, Show::All].into_iter().find(|x| format!("{x:?}") == v).unwrap_or_default(),
                "search" => f.search = v.to_string(),
                "projects" => f.projects = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                "group" => f.group = v.parse().ok(),
                "purpose" => f.purpose = v.parse().ok(),
                "no_todo" => f.no_todo_file = v == "true",
                "no_roadmap" => f.no_roadmap = v == "true",
                "sort" => f.sort = [Sort::File, Sort::Text, Sort::Section].into_iter().find(|x| format!("{x:?}") == v).unwrap_or_default(),
                "by" => f.by = [By::Project, By::Section, By::None].into_iter().find(|x| format!("{x:?}") == v).unwrap_or_default(),
                _ => {}
            }
        }
        f
    }
}

#[derive(Default)]
pub struct State {
    f: Option<Filters>,
    saved: String,
    /// Saved searches `(name, query)` (setting `todos.saved`: one `name<TAB>query` per line), loaded once.
    searches: Option<Vec<(String, String)>>,
    /// Matching items per saved search for the sidebar, and when they were counted.
    counts: Option<(Instant, Vec<usize>)>,
    save_name: String,
}

const SAVED_KEY: &str = "todos.saved";

/// The saved searches, loaded from the store on first use.
fn searches(app: &mut App) -> Vec<(String, String)> {
    if app.todos_page.searches.is_none() {
        let text = app.setting(SAVED_KEY, "");
        app.todos_page.searches = Some(text.lines().filter_map(|l| l.split_once('\t')).map(|(n, q)| (n.to_string(), q.to_string())).collect());
    }
    app.todos_page.searches.clone().unwrap_or_default()
}

fn store_searches(app: &mut App, v: Vec<(String, String)>) {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ").trim().to_string();
    let text = v.iter().map(|(n, q)| format!("{}\t{}", clean(n), clean(q))).collect::<Vec<_>>().join("\n");
    let r = app.store.set_setting(SAVED_KEY, &text);
    app.ok(r);
    app.todos_page.searches = Some(v);
    app.todos_page.counts = None;
}

/// The saved searches as loaded so far (the palette lists them).
pub fn saved(app: &App) -> &[(String, String)] {
    app.todos_page.searches.as_deref().unwrap_or_default()
}

/// Saved searches with how many items match each (open ones unless the query says `is:`), counted at most every
/// 2 seconds: the sidebar shows them.
pub fn sidebar_searches(app: &mut App) -> Vec<(String, String, usize)> {
    let list = searches(app);
    if list.is_empty() {
        return Vec::new();
    }
    let fresh = app.todos_page.counts.as_ref().is_some_and(|(t, c)| t.elapsed().as_secs() < 2 && c.len() == list.len());
    if !fresh {
        let rows = collect(app, &Filters::default());
        let counts = list
            .iter()
            .map(|(_, q)| {
                let q = Query::parse(q);
                let show = if q.has_status() { Show::All } else { Show::Open };
                rows.iter().filter(|r| matches(show, &r.item) && q.matches(&r.view())).count()
            })
            .collect();
        app.todos_page.counts = Some((Instant::now(), counts));
    }
    let counts = app.todos_page.counts.as_ref().map(|c| c.1.clone()).unwrap_or_default();
    list.into_iter().zip(counts).map(|((n, q), c)| (n, q, c)).collect()
}

/// Open the TODOs page with `query` (all items when the query says `is:` itself, else open ones).
pub fn apply(app: &mut App, query: &str) {
    if app.todos_page.f.is_none() {
        let saved = app.setting("todos.page", "");
        app.todos_page.f = Some(Filters::decode(&saved));
        app.todos_page.saved = saved;
    }
    if let Some(f) = app.todos_page.f.as_mut() {
        f.search = query.to_string();
        f.show = if Query::parse(query).has_status() { Show::All } else { Show::Open };
    }
    app.open.push(Tab::Todos);
}

/// Whether the TODOs page shows `query` now (the sidebar marks that saved search).
pub fn showing(app: &App, query: &str) -> bool {
    app.todos_page.f.as_ref().is_some_and(|f| f.search.trim() == query.trim())
}

#[derive(Clone)]
struct Row {
    project: i64,
    name: String,
    roadmap: bool,
    file: PathBuf,
    item: TodoItem,
}

impl Row {
    fn view(&self) -> Item<'_> {
        Item { project: &self.name, section: &self.item.section, text: &self.item.text, done: self.item.done, roadmap: self.roadmap }
    }
}

enum Act {
    Toggle(i64, bool, String, bool),
    OpenFile(PathBuf, usize),
    OpenProject(i64, bool),
}

/// Open TODO items over all projects (for the sidebar badge), from the cached project info.
/// The sidebar's count: the page's Open count without filters (TODO file and roadmap items of tracked projects).
pub fn open_total(app: &mut App) -> usize {
    collect(app, &Filters::default()).iter().filter(|r| matches(Show::Open, &r.item)).count()
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    if app.todos_page.f.is_none() {
        let saved = app.setting("todos.page", "");
        app.todos_page.f = Some(Filters::decode(&saved));
        app.todos_page.saved = saved;
    }
    let mut f = app.todos_page.f.clone().unwrap_or_default();
    let all = collect(app, &f);
    let query = Query::parse(&f.search);
    let count = |s: Show| all.iter().filter(|r| matches(s, &r.item) && query.matches(&r.view())).count();
    let (n_open, n_done, n_all) = (count(Show::Open), count(Show::Done), count(Show::All));
    let list = searches(app);

    let mut field = None;
    ui.horizontal_wrapped(|ui| {
        let items = [("Open".to_string(), Some(n_open)), ("Done".to_string(), Some(n_done)), ("All".to_string(), Some(n_all))];
        let sel = match f.show {
            Show::Open => 0,
            Show::Done => 1,
            Show::All => 2,
        };
        if let Some(i) = theme::segmented(ui, &items, sel) {
            f.show = [Show::Open, Show::Done, Show::All][i];
        }
        theme::right(ui, "todos_page.rs:227", egui::Align::Center, |ui| {
            view_menu(ui, &mut f, p);
            filters_menu(app, ui, &mut f, p);
            saved_menu(app, ui, &mut f, &list, p);
            let hint = format!("{}  Search: words, project:, in:, is:done, kind:bug, has:issue, -word", icon::MAGNIFYING_GLASS);
            field = Some(ui.add(theme::field(&mut f.search, 420.0).hint_text(hint)));
        });
    });
    // Completions for the word being typed, as chips under the bar.
    let names: Vec<String> = app.projects.iter().map(|q| q.name.clone()).collect();
    let mut sections: Vec<String> = all.iter().map(|r| r.item.section.clone()).filter(|s| !s.is_empty()).collect();
    sections.sort();
    sections.dedup();
    let hints = pa_core::query::suggest(&f.search, &names, &sections);
    if !hints.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(caps("Complete", p.faint));
            // A completion is the query with its last word replaced: the chip shows that word.
            let head = f.search.rfind(char::is_whitespace).map_or(0, |i| i + 1);
            for h in &hints {
                let last = h.get(head..).unwrap_or(h).trim_end();
                if theme::ghost(ui, last).clicked() {
                    f.search = h.clone();
                    if let Some(r) = &field {
                        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                            let end = egui::text::CCursor::new(f.search.chars().count());
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                            st.store(ui.ctx(), r.id);
                        }
                        r.request_focus();
                    }
                }
            }
        });
    }
    ui.add_space(12.0);

    let mut rows: Vec<Row> = all.into_iter().filter(|r| matches(f.show, &r.item) && query.matches(&r.view())).collect();
    match f.sort {
        Sort::File => {}
        Sort::Text => rows.sort_by_key(|r| r.item.text.to_lowercase()),
        Sort::Section => rows.sort_by_key(|r| r.item.section.to_lowercase()),
    }
    let mut groups: Vec<(String, Option<i64>, Vec<Row>)> = Vec::new();
    for r in rows {
        let (label, pid) = match f.by {
            By::Project => (r.name.clone(), Some(r.project)),
            By::Section => (if r.item.section.is_empty() { "General".into() } else { r.item.section.clone() }, None),
            By::None => (String::new(), None),
        };
        match groups.iter_mut().find(|g| g.0 == label) {
            Some(g) => g.2.push(r),
            None => groups.push((label, pid, vec![r])),
        }
    }
    if f.by != By::Project {
        groups.sort_by_key(|g| g.0.to_lowercase());
    }

    let mut acts = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if groups.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::LIST_CHECKS).size(40.0).color(p.faint));
                ui.label(RichText::new("No TODOs match. Projects need a TODO file (TODO.md, TODOS.md, TODO.txt) or a ROADMAP.md.").color(p.dim));
            });
        }
        for (label, pid, rows) in &groups {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if let Some(pid) = pid {
                        let color = app.projects.iter().find(|q| q.id == *pid).and_then(|q| q.info()?.languages.first().map(|l| theme::lang_color(&l.0)));
                        avatar(ui, label, color.unwrap_or_else(|| theme::hash_color(label)), 26.0);
                        ui.label(RichText::new(label).font(theme::semibold(15.0)));
                    } else if !label.is_empty() {
                        ui.label(RichText::new(label).font(theme::semibold(15.0)));
                    }
                    ui.label(caps(format!("{}", rows.len()), p.faint));
                    if let Some(pid) = pid {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if theme::ghost(ui, format!("{}  Open project", icon::FOLDER_OPEN)).clicked() {
                                acts.push(Act::OpenProject(*pid, false));
                            }
                        });
                    }
                });
                ui.add_space(4.0);
                for r in rows {
                    item_row(ui, r, &f, p, &mut acts);
                }
            });
            ui.add_space(12.0);
        }
    });

    for a in acts {
        match a {
            Act::Toggle(pid, roadmap, id, done) => {
                if roadmap {
                    edit_roadmap(app, pid, |r| r.doc.set_done(&id, done));
                } else {
                    edit_todo(app, pid, |d| d.set_done(&id, done));
                }
            }
            Act::OpenFile(path, line) => open_at(app, &path, Some(line)),
            Act::OpenProject(pid, roadmap) => {
                app.project.set_section(pid, if roadmap { "roadmap" } else { "todos" });
                app.open.push(Tab::Project(pid));
            }
        }
    }
    let enc = f.encode();
    if enc != app.todos_page.saved {
        let r = app.store.set_setting("todos.page", &enc);
        app.ok(r);
        app.todos_page.saved = enc;
    }
    app.todos_page.f = Some(f);
}

fn matches(s: Show, it: &TodoItem) -> bool {
    match s {
        Show::Open => !it.done,
        Show::Done => it.done,
        Show::All => true,
    }
}

/// Every item of every project the filters allow (status and search are applied by the caller).
fn collect(app: &mut App, f: &Filters) -> Vec<Row> {
    let projects: Vec<pa_core::Project> = app
        .projects
        .iter()
        .filter(|q| f.projects.is_empty() || f.projects.contains(&q.id))
        .filter(|q| f.group.is_none_or(|g| q.group_id == Some(g)))
        .filter(|q| f.purpose.is_none_or(|u| q.purpose_id == Some(u)))
        .filter(|q| app.purpose(q.purpose_id).is_none_or(|u| u.track_todos))
        .cloned()
        .collect();
    let mut rows = Vec::new();
    for q in &projects {
        let Some(root) = q.primary().map(|c| PathBuf::from(&c.path)) else { continue };
        if !f.no_todo_file {
            load_todo(app, q, &root);
            if let Some(l) = app.project.todos.get(&q.id) {
                for it in &l.doc.items {
                    rows.push(Row { project: q.id, name: q.name.clone(), roadmap: false, file: l.path.clone(), item: it.clone() });
                }
            }
        }
        if !f.no_roadmap {
            load_roadmap(app, q, &root);
            if let Some(l) = app.project.roadmaps.get(&q.id) {
                for it in &l.doc.doc.items {
                    rows.push(Row { project: q.id, name: q.name.clone(), roadmap: true, file: l.path.clone(), item: it.clone() });
                }
            }
        }
    }
    rows
}

/// Room kept on the right of each item for its tags and the open-file button.
const TAGS_W: f32 = 300.0;

fn item_row(ui: &mut egui::Ui, r: &Row, f: &Filters, p: theme::Palette, acts: &mut Vec<Act>) {
    let row = ui.horizontal_top(|ui| {
        ui.add_space(r.item.depth as f32 * 22.0);
        let glyph = if r.item.done { icon::CHECK_SQUARE } else { icon::SQUARE };
        let tip = if r.item.done { "Mark as open" } else { "Mark as done" };
        if theme::button(ui, glyph, egui::FontId::proportional(18.0), 0.0, theme::Kind::Ghost, vec2(26.0, 26.0)).on_hover_text(tip).clicked() {
            acts.push(Act::Toggle(r.project, r.roadmap, r.item.id.clone(), !r.item.done));
        }
        let mut text = RichText::new(theme::plain_md(&r.item.text));
        if r.item.done {
            text = text.strikethrough().color(p.faint);
        }
        // The text wraps in what is left after the tags column, so tags never overlap it.
        let text_w = (ui.available_width() - TAGS_W).max(200.0);
        ui.allocate_ui(vec2(text_w, 0.0), |ui| {
            ui.set_width(text_w);
            ui.add_space(4.0);
            ui.add(egui::Label::new(text).wrap()).on_hover_text("Double-click: open the project's TODOs");
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, "Open the file at this line in VS Code").clicked() {
                acts.push(Act::OpenFile(r.file.clone(), r.item.line + 1));
            }
            if f.by != By::Project {
                tag(ui, &r.name, theme::hash_color(&r.name));
            }
            if f.by != By::Section && !r.item.section.is_empty() {
                tag(ui, &r.item.section, p.dim);
            }
            if r.roadmap {
                tag(ui, format!("{} roadmap", icon::MAP_TRIFOLD), p.purple);
            }
        });
    });
    if row.response.interact(egui::Sense::click()).double_clicked() {
        acts.push(Act::OpenProject(r.project, r.roadmap));
    }
}

fn filters_menu(app: &App, ui: &mut egui::Ui, f: &mut Filters, p: theme::Palette) {
    let n = [!f.projects.is_empty(), f.group.is_some(), f.purpose.is_some(), f.no_todo_file || f.no_roadmap].iter().filter(|b| **b).count();
    let label = if n > 0 { format!("{}  Filters · {n}", icon::FUNNEL) } else { format!("{}  Filters", icon::FUNNEL) };
    ui.menu_button(RichText::new(label).size(13.0), |ui| {
        ui.set_min_width(260.0);
        ui.label(caps("Source", p.faint));
        let mut todo = !f.no_todo_file;
        let mut roadmap = !f.no_roadmap;
        theme::checkbox(ui, &mut todo, "TODO files");
        theme::checkbox(ui, &mut roadmap, "Roadmaps");
        (f.no_todo_file, f.no_roadmap) = (!todo, !roadmap);
        ui.add_space(4.0);
        ui.label(caps("Group", p.faint));
        crate::widgets::group_combo(ui, "todo_group", &app.groups, &mut f.group);
        ui.label(caps("Purpose", p.faint));
        crate::widgets::purpose_combo(ui, "todo_purpose", &app.purposes, &mut f.purpose);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(caps("Projects", p.faint));
            if !f.projects.is_empty() && theme::ghost(ui, "All").clicked() {
                f.projects.clear();
            }
        });
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for q in &app.projects {
                let mut on = f.projects.contains(&q.id);
                if theme::checkbox(ui, &mut on, &q.name).changed() {
                    if on {
                        f.projects.push(q.id);
                    } else {
                        f.projects.retain(|x| *x != q.id);
                    }
                }
            }
        });
    });
}

/// Saved searches: apply or delete one, or save the current query under a name.
fn saved_menu(app: &mut App, ui: &mut egui::Ui, f: &mut Filters, list: &[(String, String)], p: theme::Palette) {
    let label = if list.is_empty() { format!("{}  Saved", icon::BOOKMARK_SIMPLE) } else { format!("{}  Saved · {}", icon::BOOKMARK_SIMPLE, list.len()) };
    let mut change: Option<Vec<(String, String)>> = None;
    ui.menu_button(RichText::new(label).size(13.0), |ui| {
        ui.set_min_width(320.0);
        ui.label(caps("Saved searches", p.faint));
        if list.is_empty() {
            ui.label(RichText::new("None yet. They also show in the sidebar.").color(p.dim));
        }
        for (k, (name, query)) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                if theme::icon_button(ui, icon::TRASH, "Delete this saved search").clicked() {
                    let mut v = list.to_vec();
                    v.remove(k);
                    change = Some(v);
                }
                if ui.button(name).on_hover_text(query).clicked() {
                    f.search = query.clone();
                    f.show = if Query::parse(query).has_status() { Show::All } else { Show::Open };
                    ui.close();
                }
            });
        }
        ui.separator();
        ui.label(caps("Save the current search", p.faint));
        ui.horizontal(|ui| {
            ui.add(theme::field(&mut app.todos_page.save_name, 200.0).hint_text("Name"));
            let ready = !f.search.trim().is_empty() && !app.todos_page.save_name.trim().is_empty();
            if ui.add_enabled_ui(ready, |ui| theme::secondary(ui, "Save")).inner.clicked() {
                let mut v = list.to_vec();
                let name = std::mem::take(&mut app.todos_page.save_name).trim().to_string();
                v.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                v.push((name, f.search.trim().to_string()));
                change = Some(v);
            }
        });
    });
    if let Some(v) = change {
        store_searches(app, v);
    }
}

fn view_menu(ui: &mut egui::Ui, f: &mut Filters, p: theme::Palette) {
    ui.menu_button(RichText::new(format!("{}  View", icon::ROWS)).size(13.0), |ui| {
        ui.set_min_width(200.0);
        ui.label(caps("Group by", p.faint));
        ui.selectable_value(&mut f.by, By::Project, "Project");
        ui.selectable_value(&mut f.by, By::Section, "Section / milestone");
        ui.selectable_value(&mut f.by, By::None, "Nothing");
        ui.add_space(4.0);
        ui.label(caps("Sort", p.faint));
        ui.selectable_value(&mut f.sort, Sort::File, "File order");
        ui.selectable_value(&mut f.sort, Sort::Text, "Text (A–Z)");
        ui.selectable_value(&mut f.sort, Sort::Section, "Section");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_round_trip() {
        let f = Filters {
            show: Show::Done,
            search: "a;b=c".into(),
            projects: vec![3, 7],
            group: Some(2),
            purpose: None,
            no_todo_file: false,
            no_roadmap: true,
            sort: Sort::Text,
            by: By::Section,
        };
        let back = Filters::decode(&f.encode());
        assert_eq!(back, Filters { search: "a b c".into(), ..f });
        assert_eq!(Filters::decode("garbage;show=Nope"), Filters::default());
    }
}
