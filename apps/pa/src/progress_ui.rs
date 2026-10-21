//! Progress (after YouTrack's reports): the dashboard's history widgets (TODO items over time, the roadmap's Now and
//! Next entries over time) and the Reports page (every project side by side). Everything comes from git history and Claude transcripts (`pa_core::history`), is counted on a background
//! thread, and is kept for the session; Recount reads it again.

use crate::chart::{self, Series};
use crate::theme::{self, caps, card, pal};
use crate::{App, Job, Tab};
use egui::{Align, Layout, RichText, vec2};
use egui_phosphor::regular as icon;
use pa_core::history::{self, EntryPoint, Point};
use pa_core::todo::Style;
use pa_core::{Project, ops, roadmap};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Days with changes kept per file in a project's section.
const POINTS: usize = 60;

#[derive(Default)]
pub struct State {
    /// Per project; `None` while counting.
    data: HashMap<i64, Option<Data>>,
    /// The Reports page's rows; `Some(None)` while counting.
    report: Option<Option<Vec<ReportRow>>>,
}

struct Data {
    todo: Vec<Point>,
    roadmap: Vec<Point>,
    /// Today's Now and Next entries with their history.
    active: Vec<(String, Vec<EntryPoint>)>,
}

#[derive(Clone)]
struct ReportRow {
    id: i64,
    name: String,
    /// Open TODO-file items on the last days the file changed, oldest first.
    open: Vec<f32>,
    /// Open items now minus 30 days ago (`None` without history that old).
    delta: Option<i64>,
    commits: usize,
    sessions: usize,
}

/// What a count needs from a project: the store stays on the UI thread.
struct Input {
    id: i64,
    name: String,
    root: PathBuf,
    todo: Option<(String, Style)>,
    roadmap: Option<String>,
    folders: Vec<String>,
}

fn input(q: &Project) -> Option<Input> {
    let root = PathBuf::from(&q.primary()?.path);
    let rel = |p: &Path| p.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
    let todo = ops::todo_path_in(q, &root).and_then(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        Some((rel(&p)?, ops::style_of(q, &text)))
    });
    let roadmap = ops::roadmap_path_in(q, &root).and_then(|p| rel(&p));
    Some(Input { id: q.id, name: q.name.clone(), todo, roadmap, folders: q.checkouts.iter().map(|c| c.path.clone()).collect(), root })
}

fn entry_key(m: &roadmap::Milestone) -> String {
    m.version.clone().unwrap_or_else(|| m.title.clone())
}

fn count(i: &Input) -> Data {
    let todo = i.todo.as_ref().map(|(rel, style)| history::todo_history(&i.root, rel, style, POINTS)).unwrap_or_default();
    let (roadmap, entries) = i.roadmap.as_ref().map(|rel| history::roadmap_history(&i.root, rel, POINTS)).unwrap_or_default();
    let now: Vec<String> = i
        .roadmap
        .as_ref()
        .and_then(|rel| std::fs::read_to_string(i.root.join(rel)).ok())
        .map(|t| {
            let r = roadmap::parse_roadmap(&t);
            let b = roadmap::board(&r);
            b.now.iter().chain(&b.next).map(|&k| entry_key(&r.milestones[k])).collect()
        })
        .unwrap_or_default();
    let active = now.iter().filter_map(|k| entries.iter().find(|e| &e.0 == k).cloned()).filter(|e| e.1.iter().any(|p| p.total > 0)).take(6).collect();
    Data { todo, roadmap, active }
}

// --- the dashboard's history widgets --------------------------------------------------------------------------

/// The project's histories, counted on a background thread the first time a widget asks; `None` while counting.
fn data<'a>(app: &'a mut App, ui: &egui::Ui, q: &Project) -> Option<&'a Data> {
    if !app.progress.data.contains_key(&q.id)
        && let Some(i) = input(q)
    {
        app.progress.data.insert(q.id, None);
        app.background(ui.ctx(), move || {
            let d = count(&i);
            Box::new(move |app: &mut App| {
                app.progress.data.insert(i.id, Some(d));
            }) as Job
        });
    }
    app.progress.data.get(&q.id)?.as_ref()
}

/// Counting spinner or Recount; then the data, if any.
fn header<'a>(app: &'a mut App, ui: &mut egui::Ui, q: &Project) -> Option<&'a Data> {
    let counting = data(app, ui, q).is_none() && app.progress.data.contains_key(&q.id);
    let mut again = false;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if counting {
                ui.spinner();
            } else {
                again = theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Read the git history again").clicked();
            }
        });
    });
    if again {
        app.progress.data.remove(&q.id);
    }
    app.progress.data.get(&q.id)?.as_ref()
}

/// Open and done items of the TODO file over its git history.
pub fn todo_widget(app: &mut App, ui: &mut egui::Ui, q: &Project) {
    let p = pal(ui.ctx());
    let Some(d) = header(app, ui, q) else { return };
    if let (Some(a), Some(b)) = (d.todo.first(), d.todo.last()) {
        let text = format!("Open {} → {} since {} · {} done", a.open, b.open, chart::day_label(a.day), b.done);
        ui.label(RichText::new(text).color(p.dim));
    }
    ui.add_space(6.0);
    let pts = |f: fn(&Point) -> usize| d.todo.iter().map(|x| (x.day, f(x) as f32)).collect::<Vec<_>>();
    let series = [
        Series { label: "Open".into(), color: p.blue, points: pts(|x| x.open) },
        Series { label: "Done".into(), color: p.green, points: pts(|x| x.done) },
    ];
    chart::lines(ui, &series, (ui.available_height() - 50.0).clamp(80.0, 220.0), "", p);
}

/// Today's Now and Next roadmap entries, % of their items done over the roadmap's git history.
pub fn roadmap_widget(app: &mut App, ui: &mut egui::Ui, q: &Project) {
    let p = pal(ui.ctx());
    let Some(d) = header(app, ui, q) else { return };
    ui.label(RichText::new("Now and next entries: share of their items done").color(p.dim));
    ui.add_space(6.0);
    if d.active.is_empty() {
        let what = if d.roadmap.is_empty() { "No roadmap history." } else { "Nothing in progress or next has items." };
        ui.label(RichText::new(what).color(p.dim));
        return;
    }
    let series: Vec<Series> = d
        .active
        .iter()
        .map(|(k, pts)| Series {
            label: k.clone(),
            color: theme::hash_color(k),
            points: pts.iter().filter(|x| x.total > 0).map(|x| (x.day, 100.0 * x.done as f32 / x.total as f32)).collect(),
        })
        .collect();
    chart::lines(ui, &series, (ui.available_height() - 50.0).clamp(80.0, 220.0), "%", p);
}

// --- the Reports page -----------------------------------------------------------------------------------------

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    let ctx = ui.ctx().clone();
    if app.progress.report.is_none() {
        let inputs: Vec<Input> = app.projects.iter().filter_map(input).collect();
        let home = pa_core::claude::home(&app.store).ok();
        app.progress.report = Some(None);
        app.background(&ctx, move || {
            let today = pa_core::today_days();
            let rows: Vec<ReportRow> = inputs
                .iter()
                .map(|i| {
                    let h = i.todo.as_ref().map(|(rel, style)| history::todo_history(&i.root, rel, style, 30)).unwrap_or_default();
                    let then = h.iter().rev().find(|x| x.day <= today - 30).map(|x| x.open as i64);
                    let sessions = home.as_deref().map_or(0, |hm| history::sessions_per_week(hm, &i.folders, 4).iter().sum());
                    ReportRow {
                        id: i.id,
                        name: i.name.clone(),
                        open: h.iter().map(|x| x.open as f32).collect(),
                        delta: then.zip(h.last()).map(|(t, l)| l.open as i64 - t),
                        commits: history::commits_per_week(&i.root, 4).iter().sum(),
                        sessions,
                    }
                })
                .filter(|r| !r.open.is_empty() || r.commits + r.sessions > 0)
                .collect();
            Box::new(move |app: &mut App| app.progress.report = Some(Some(rows))) as Job
        });
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Reports").font(theme::semibold(20.0)));
        ui.label(RichText::new("Open TODO items over time and the last 4 weeks of activity, per project").color(p.dim));
        theme::right(ui, "progress_ui.rs:199", egui::Align::Center, |ui| {
            if matches!(app.progress.report, Some(None)) {
                ui.spinner();
                ui.label(RichText::new("Reading git history…").color(p.faint));
            } else if theme::ghost(ui, format!("{}  Recount", icon::ARROWS_CLOCKWISE)).clicked() {
                app.progress.report = None;
            }
        });
    });
    ui.add_space(12.0);
    let Some(Some(rows)) = app.progress.report.clone() else { return };
    let mut rows = rows;
    rows.sort_by_key(|r| (std::cmp::Reverse(r.commits + r.sessions), r.name.to_lowercase()));
    let open_now = |id: i64| app.projects.iter().find(|q| q.id == id).and_then(|q| q.info()?.todo_open);
    let mut open: Option<i64> = None;
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("reports").num_columns(6).spacing(vec2(28.0, 10.0)).striped(true).show(ui, |ui| {
                for h in ["Project", "Open now", "Open items, last changes", "Δ 30 days", "Commits, 4 wk", "Claude sessions, 4 wk"] {
                    ui.label(caps(h, p.faint));
                }
                ui.end_row();
                for r in &rows {
                    if theme::ghost(ui, &r.name).on_hover_text("Open the project's dashboard").clicked() {
                        open = Some(r.id);
                    }
                    ui.label(RichText::new(open_now(r.id).map_or("—".into(), |n| n.to_string())).monospace());
                    chart::spark(ui, &r.open, vec2(160.0, 22.0), p.blue);
                    let (text, color) = match r.delta {
                        Some(d) if d > 0 => (format!("+{d}"), p.orange),
                        Some(d) if d < 0 => (d.to_string(), p.green),
                        Some(_) => ("0".into(), p.faint),
                        None => ("—".into(), p.faint),
                    };
                    ui.label(RichText::new(text).monospace().color(color));
                    ui.label(RichText::new(r.commits.to_string()).monospace());
                    ui.label(RichText::new(r.sessions.to_string()).monospace());
                    ui.end_row();
                }
            });
        });
    });
    if let Some(id) = open {
        app.project.set_section(id, "overview");
        app.open.push(Tab::Project(id));
    }
}
