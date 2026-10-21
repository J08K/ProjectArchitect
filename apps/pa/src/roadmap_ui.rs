//! The Roadmap tab (M8): versions (`vX.Y.Z`, what ships) and milestones (themes), shown as a Now / Next / Later
//! board (default), a vertical timeline, the editable list, or a Gantt chart when most entries have a target date.
//! The view is remembered per project (`roadmap.view.<id>`). Edits go through `project::edit_roadmap` (lossless,
//! re-read check). Release suggestions come from git tags with the project's prefix.

use crate::project::{
    ItemAction, add_box, apply_items, edit_roadmap, empty_file, file_bar, fold_toggle, item_glance, item_list, load_roadmap, progress, read_only, status_color,
    status_hover, status_label, timeline_entry,
};
use crate::theme::{self, Palette, caps, card, chip, tag};
use crate::App;
use egui::{Align, Color32, Layout, RichText, Sense, Stroke, vec2};
use egui_phosphor::regular as icon;
use pa_core::roadmap::{self, EntryKind, Milestone, MilestoneStatus, Roadmap};
use pa_core::{Project, ops};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Board,
    Timeline,
    List,
    Gantt,
}

impl View {
    /// The name shown: `Timeline` is the read-only progress list (no time axis), so it shows as "Progress"; the
    /// dated Gantt chart is the real timeline. The setting keys stay as they were.
    fn label(self) -> &'static str {
        match self {
            View::Board => "Board",
            View::Timeline => "Progress",
            View::List => "List",
            View::Gantt => "Timeline",
        }
    }

    fn key(self) -> &'static str {
        match self {
            View::Board => "board",
            View::Timeline => "timeline",
            View::List => "list",
            View::Gantt => "gantt",
        }
    }

    fn parse(s: &str) -> View {
        match s {
            "timeline" => View::Timeline,
            "list" => View::List,
            "gantt" => View::Gantt,
            _ => View::Board,
        }
    }
}

/// What a view asks for after it is drawn (the roadmap is borrowed while drawing).
pub(crate) enum Act {
    Status(String, MilestoneStatus),
    /// Drop on a lane: new status, and the entry to move before (`None` = end).
    Lane(String, MilestoneStatus, Option<String>),
    Link(String, String),
    AddVersion(String, String, Option<String>),
}

/// The key an entry is edited by: its version, else its title.
pub(crate) fn key(m: &Milestone) -> String {
    m.version.clone().unwrap_or_else(|| m.title.clone())
}

fn done_status(m: &Milestone) -> MilestoneStatus {
    if m.kind == EntryKind::Version { MilestoneStatus::Released } else { MilestoneStatus::Done }
}

fn is_finished(s: &MilestoneStatus) -> bool {
    matches!(s, MilestoneStatus::Done | MilestoneStatus::Released)
}

pub fn show(app: &mut App, ui: &mut egui::Ui, id: i64, q: &Project, root: &Path, p: Palette) {
    load_roadmap(app, q, root);
    let Some(l) = app.project.roadmaps.get(&id) else {
        empty_file(app, ui, id, root, "ROADMAP.md", "# Roadmap\n\n## v0.1.0 — First release\nStatus: in progress\n\nFirst usable version.\n\n- [ ] First item\n", p);
        return;
    };
    let (path, editable, text) = (l.path.clone(), l.doc.editable, l.text.clone());
    let r = l.doc.clone();
    let versions = r.milestones.iter().filter(|m| m.kind == EntryKind::Version).count();
    let finished = r.milestones.iter().filter(|m| is_finished(&m.effective)).count();
    let summary = format!("{versions} versions · {} milestones · {finished} finished", r.milestones.len() - versions);
    let setting = format!("roadmap.view.{id}");
    let mut view = View::parse(&app.setting(&setting, "board"));
    let dated = r.milestones.iter().filter(|m| m.target.as_deref().and_then(parse_month).is_some()).count();
    let gantt_ok = !r.milestones.is_empty() && dated * 2 >= r.milestones.len();
    if view == View::Gantt && !gantt_ok {
        view = View::Board;
    }
    let mut chosen = view;
    file_bar(app, ui, &path, root, &summary, p, |_, ui| {
        if !editable {
            return;
        }
        // The file bar is right-to-left, which shows the segments reversed: this reads List · Progress · Board ·
        // Timeline on screen.
        let mut views = vec![View::Board, View::Timeline, View::List];
        if gantt_ok {
            views.insert(0, View::Gantt);
        }
        let items: Vec<(String, Option<usize>)> = views.iter().map(|v| (v.label().to_string(), None)).collect();
        if let Some(i) = theme::segmented(ui, &items, views.iter().position(|v| *v == view).unwrap_or(0)) {
            chosen = views[i];
        }
    });
    if chosen != view {
        let res = app.store.set_setting(&setting, chosen.key());
        app.ok(res);
        view = chosen;
    }
    if !editable {
        read_only(app, ui, id, root, &text, true, p);
        return;
    }
    let mut acts = Vec::new();
    suggestions(app, ui, q, &r, p, &mut acts);
    match view {
        View::Board => board(ui, id, &r, p, &mut acts),
        View::Timeline => timeline(ui, &r, p),
        View::List => list(app, ui, id, &r, p, &mut acts),
        View::Gantt => gantt(ui, &r, p),
    }
    apply(app, id, acts);
}

/// Carry out what a view asked for, on project `id`'s roadmap.
pub(crate) fn apply(app: &mut App, id: i64, acts: Vec<Act>) {
    for a in acts {
        match a {
            Act::Status(k, s) => edit_roadmap(app, id, |r| r.set_status(&k, s)),
            Act::Lane(k, s, before) => edit_roadmap(app, id, |r| {
                r.set_status(&k, s)?;
                r.move_entry(&k, before.as_deref())
            }),
            Act::Link(v, m) => edit_roadmap(app, id, |r| r.link(&v, &m)),
            Act::AddVersion(v, t, m) => edit_roadmap(app, id, |r| r.add_version(&v, &t, MilestoneStatus::Planned, m.as_deref())),
        }
    }
}

/// "v0.5.0 has a tag: mark it released?" for versions whose tag exists.
fn suggestions(app: &mut App, ui: &mut egui::Ui, q: &Project, r: &Roadmap, p: Palette, acts: &mut Vec<Act>) {
    let tags = ops::tags(q);
    let found = roadmap::release_suggestions(r, &tags, ops::tag_prefix(q));
    if found.is_empty() {
        return;
    }
    egui::Frame::new().fill(p.blue.gamma_multiply(0.08)).stroke(Stroke::new(1.0, p.blue.gamma_multiply(0.4))).corner_radius(8).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(icon::TAG).size(16.0).color(p.blue));
            ui.label(format!("Tagged in git but not marked released: {}", found.join(", ")));
            for v in &found {
                if theme::secondary(ui, format!("Mark {v} released")).clicked() {
                    acts.push(Act::Status(v.clone(), MilestoneStatus::Released));
                }
            }
        });
    });
    let _ = app;
    ui.add_space(12.0);
}

// --- board ----------------------------------------------------------------------------------------------------

/// Board lanes: title and what is in it.
pub(crate) const LANES: [(&str, &str); 4] = [("Now", "In progress"), ("Next", "Up to the next version"), ("Later", "Planned after that"), ("Done", "Released and done")];

/// The lanes' entries (indices into the roadmap's milestones), in [`LANES`] order.
pub(crate) fn lanes(b: &roadmap::Board) -> [&Vec<usize>; 4] {
    [&b.now, &b.next, &b.later, &b.done]
}

/// What dropping entry `k` into lane `lane` does: the lane's status, and a move before the first Later entry
/// (Next) or to the end of the file (Later). `None` for an unknown entry.
pub(crate) fn drop_act(r: &Roadmap, b: &roadmap::Board, lane: usize, k: String) -> Option<Act> {
    let m = r.milestones.iter().find(|m| key(m) == k)?;
    let status = match lane {
        0 => MilestoneStatus::InProgress,
        1 | 2 => MilestoneStatus::Planned,
        _ => done_status(m),
    };
    let before = match lane {
        1 => b.later.first().map(|&i| key(&r.milestones[i])).filter(|f| *f != k),
        2 => None,
        _ => Some(k.clone()),
    };
    Some(if before.as_deref() == Some(k.as_str()) { Act::Status(k, status) } else { Act::Lane(k, status, before) })
}

fn board(ui: &mut egui::Ui, pid: i64, r: &Roadmap, p: Palette, acts: &mut Vec<Act>) {
    let b = roadmap::board(r);
    ui.columns(4, |cols| {
        for (lane, (ui, ((title, sub), idx))) in cols.iter_mut().zip(LANES.into_iter().zip(lanes(&b))).enumerate() {
            let frame = egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).corner_radius(10).inner_margin(12);
            let (_, dropped) = ui.dnd_drop_zone::<(i64, String), ()>(frame, |ui| {
                ui.set_min_height(240.0);
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(caps(title, p.text));
                    ui.label(caps(idx.len().to_string(), p.faint));
                });
                ui.label(RichText::new(sub).size(12.0).color(p.faint));
                ui.add_space(6.0);
                let folded = title == "Done" && idx.len() > 6;
                let shown: Vec<usize> = if folded { idx.iter().rev().take(6).rev().copied().collect() } else { idx.clone() };
                if folded {
                    ui.label(RichText::new(format!("{} older, see Progress", idx.len() - 6)).size(12.0).color(p.faint));
                }
                for i in shown {
                    board_card(ui, pid, i, &r.milestones[i], p, acts);
                    ui.add_space(6.0);
                }
            });
            if let Some(d) = dropped.filter(|d| d.0 == pid)
                && let Some(a) = drop_act(r, &b, lane, d.1.clone())
            {
                acts.push(a);
            }
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Drag a card to another lane to change its status. Right-click a card for more.").size(12.0).color(p.faint));
}

/// A card: drag payload `(project, entry key)`, so a drop only lands in the same project's lanes.
pub(crate) fn board_card(ui: &mut egui::Ui, pid: i64, index: usize, m: &Milestone, p: Palette, acts: &mut Vec<Act>) {
    let k = key(m);
    // The index too: a file may repeat a heading (a roadmap may have two `## v0.9`).
    let id = egui::Id::new(("board-card", pid, index, &k));
    let resp = ui.dnd_drag_source(id, (pid, k.clone()), |ui| {
        egui::Frame::new().fill(p.raised).stroke(Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                kind_badge(ui, m, p);
                ui.add(egui::Label::new(RichText::new(&m.title).font(theme::semibold(14.0))).wrap());
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                status_hover(chip(ui, status_label(&m.effective), status_color(&m.effective, p)), m);
                if let Some(w) = &m.warning {
                    chip(ui, "check", p.orange).on_hover_text(w);
                }
                for l in &m.links {
                    tag(ui, l, p.dim);
                }
                if let Some(t) = &m.target {
                    ui.label(RichText::new(format!("{} {t}", icon::FLAG)).size(12.0).color(p.faint));
                }
            });
            if m.total > 0 {
                let open = ui
                    .horizontal(|ui| {
                        let open = fold_toggle(ui, egui::Id::new(("roadmap-fold", pid, index, k.clone())));
                        ui.label(RichText::new(format!("{}/{}", m.done, m.total)).monospace().size(11.5).color(p.faint));
                        progress(ui, m.done as f32 / m.total as f32, p);
                        open
                    })
                    .inner;
                if open {
                    item_glance(ui, &m.items, p);
                }
            }
        });
    });
    resp.response.context_menu(|ui| {
        ui.label(caps("Status", p.faint));
        let done = done_status(m);
        for s in [MilestoneStatus::Planned, MilestoneStatus::InProgress, done] {
            if ui.button(status_label(&s)).clicked() {
                acts.push(Act::Status(k.clone(), s));
            }
        }
    });
}

/// A small badge: the version in monospace, or a flag for a milestone.
pub(crate) fn kind_badge(ui: &mut egui::Ui, m: &Milestone, p: Palette) {
    match (&m.kind, &m.version) {
        (EntryKind::Version, Some(v)) => {
            tag(ui, v, p.blue);
        }
        _ => {
            ui.label(RichText::new(icon::FLAG).color(p.purple)).on_hover_text("Milestone");
        }
    }
}

// --- timeline -------------------------------------------------------------------------------------------------

fn timeline(ui: &mut egui::Ui, r: &Roadmap, p: Palette) {
    let show_done = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(egui::Id::new("roadmap-timeline-done"), true));
    let mut sd = show_done;
    theme::checkbox(ui, &mut sd, "Show finished");
    if sd != show_done {
        ui.ctx().data_mut(|d| d.insert_persisted(egui::Id::new("roadmap-timeline-done"), sd));
    }
    ui.add_space(8.0);
    // (index in the file, entry): the index keeps the fold state shared with the board.
    let entries: Vec<(usize, &Milestone)> = r.milestones.iter().enumerate().filter(|(_, m)| sd || !is_finished(&m.effective)).collect();
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        for (n, &(i, m)) in entries.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add_space(if m.kind == EntryKind::Version && !m.links.is_empty() { 18.0 } else { 0.0 });
                let fold = egui::Id::new(("roadmap-fold", i, key(m)));
                ui.vertical(|ui| timeline_entry(ui, m, n + 1 == entries.len(), !is_finished(&m.effective), Some(fold), p));
            });
        }
        if entries.is_empty() {
            ui.label(RichText::new("Nothing planned.").color(p.dim));
        }
    });
}

// --- list (editable) ------------------------------------------------------------------------------------------

fn list(app: &mut App, ui: &mut egui::Ui, id: i64, r: &Roadmap, p: Palette, acts: &mut Vec<Act>) {
    let headings: Vec<String> = r.milestones.iter().map(|m| m.heading.clone()).collect();
    let milestones: Vec<String> = r.milestones.iter().filter(|m| m.kind == EntryKind::Milestone).map(|m| m.title.clone()).collect();
    let mut items_acts: Vec<ItemAction> = Vec::new();
    for (i, (m, head)) in r.milestones.iter().zip(&headings).enumerate() {
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            let salt = ui.make_persistent_id(("ms", id, i, &head));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), salt, !is_finished(&m.effective))
                .show_header(ui, |ui| {
                    kind_badge(ui, m, p);
                    ui.label(RichText::new(&m.title).font(theme::semibold(16.0)));
                    let r = ui.menu_button(caps(status_label(&m.effective), status_color(&m.effective, p)), |ui| {
                        for s in [MilestoneStatus::Planned, MilestoneStatus::InProgress, done_status(m)] {
                            if ui.button(status_label(&s)).clicked() {
                                acts.push(Act::Status(key(m), s));
                            }
                        }
                    }).response;
                    status_hover(r, m);
                    if let Some(w) = &m.warning {
                        chip(ui, "check status", p.orange).on_hover_text(w);
                    }
                    if let Some(t) = &m.target {
                        ui.label(RichText::new(format!("{} {t}", icon::FLAG)).size(13.0).color(p.dim));
                    }
                    for l in &m.links {
                        tag(ui, l, p.dim);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if m.total > 0 {
                            ui.label(RichText::new(format!("{}/{}", m.done, m.total)).monospace().color(p.dim));
                        }
                        if m.kind == EntryKind::Version && !milestones.is_empty() {
                            ui.menu_button(RichText::new(format!("{} Milestone", icon::FLAG)).size(12.5), |ui| {
                                for ms in &milestones {
                                    if ui.button(ms).clicked() {
                                        acts.push(Act::Link(key(m), ms.clone()));
                                    }
                                }
                            });
                        }
                    });
                })
                .body(|ui| {
                    if m.total > 0 {
                        progress(ui, m.done as f32 / m.total as f32, p);
                    }
                    if !m.body.trim().is_empty() {
                        ui.label(RichText::new(m.body.trim()).color(p.dim));
                    }
                    item_list(&app.project, ui, id, &m.items, &headings, true, None, p, &mut items_acts);
                    add_box(&mut app.project.new_item, ui, id, head, "Add an item…", &mut items_acts);
                });
        });
        ui.add_space(12.0);
    }
    new_entry(app, ui, id, &milestones, p, acts);
    apply_items(app, id, items_acts, true);
}

/// "New version" (version, title, milestone) and "New milestone" rows.
fn new_entry(app: &mut App, ui: &mut egui::Ui, id: i64, milestones: &[String], p: Palette, acts: &mut Vec<Act>) {
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("New version", p.faint));
        let (mut v, mut t, mut m) = {
            let b = &app.project.new_item;
            (
                b.get(&(id, "\0version".into())).cloned().unwrap_or_default(),
                b.get(&(id, "\0version-title".into())).cloned().unwrap_or_default(),
                b.get(&(id, "\0version-ms".into())).cloned().unwrap_or_default(),
            )
        };
        ui.horizontal(|ui| {
            ui.add(theme::field(&mut v, 120.0).hint_text("v0.6.0"));
            ui.add(theme::field(&mut t, (ui.available_width() - 330.0).max(160.0)).hint_text("Title"));
            theme::select(ui, ("new-version-ms", id), 160.0, if m.is_empty() { "No milestone".to_string() } else { m.clone() }, |ui| {
                ui.selectable_value(&mut m, String::new(), "No milestone");
                for ms in milestones {
                    ui.selectable_value(&mut m, ms.clone(), ms);
                }
            });
            if theme::primary(ui, "Add").clicked() && !v.trim().is_empty() {
                let ms = (!m.is_empty()).then(|| m.clone());
                acts.push(Act::AddVersion(v.trim().to_string(), t.trim().to_string(), ms));
                v.clear();
                t.clear();
            }
        });
        let b = &mut app.project.new_item;
        b.insert((id, "\0version".into()), v);
        b.insert((id, "\0version-title".into()), t);
        b.insert((id, "\0version-ms".into()), m);
        ui.add_space(8.0);
        ui.label(caps("New milestone", p.faint));
        let buf = app.project.new_item.entry((id, "\0milestone".into())).or_default();
        let r = ui.add(theme::field(buf, ui.available_width()).hint_text("A theme, e.g. UI revamp — Enter to add"));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !buf.trim().is_empty() {
            let title = buf.trim().to_string();
            buf.clear();
            edit_roadmap(app, id, |r| r.add_milestone(&title, MilestoneStatus::Planned));
        }
    });
}

// --- gantt ----------------------------------------------------------------------------------------------------

/// `2026-11` or `2026-11-30` → months since year 0 (fractional for days).
fn parse_month(s: &str) -> Option<f32> {
    let s = s.trim();
    let mut it = s.split(['-', '/']);
    let y: i32 = it.next()?.trim().parse().ok()?;
    let m: i32 = it.next()?.trim().parse().ok()?;
    let d: f32 = it.next().and_then(|d| d.trim().parse().ok()).unwrap_or(15.0);
    (1..=12).contains(&m).then_some((y * 12 + m - 1) as f32 + (d - 1.0) / 30.0)
}

fn today_month() -> f32 {
    let days = pa_core::today_days();
    // ponytail: 30.44-day months are close enough for drawing bars.
    1970.0 * 12.0 + days as f32 / 30.44
}

/// One bar per dated entry, from the previous dated entry's target (or today) to its own target.
fn gantt(ui: &mut egui::Ui, r: &Roadmap, p: Palette) {
    let mut rows: Vec<(&Milestone, f32)> = r.milestones.iter().filter_map(|m| Some((m, m.target.as_deref().and_then(parse_month)?))).collect();
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    let now = today_month();
    let start = rows.first().map_or(now, |r| r.1.min(now)) - 1.0;
    let end = rows.last().map_or(now + 6.0, |r| r.1.max(now)) + 1.0;
    card(ui, |ui| {
        let width = ui.available_width();
        let label_w = 240.0;
        let chart_w = width - label_w;
        let x_of = |m: f32| label_w + (m - start) / (end - start) * chart_w;
        let row_h = 30.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0 + row_h * rows.len() as f32), Sense::hover());
        let painter = ui.painter_at(rect);
        let o = rect.min;
        // Month ticks
        let mut m = start.ceil();
        while m < end {
            let x = o.x + x_of(m);
            let (y, mo) = ((m as i32).div_euclid(12), (m as i32).rem_euclid(12) + 1);
            painter.line_segment([egui::pos2(x, o.y + 18.0), egui::pos2(x, rect.bottom())], Stroke::new(1.0, p.border));
            painter.text(egui::pos2(x + 3.0, o.y + 2.0), egui::Align2::LEFT_TOP, format!("{y}-{mo:02}"), egui::FontId::monospace(10.0), p.faint);
            m += if end - start > 24.0 { 3.0 } else { 1.0 };
        }
        let xt = o.x + x_of(now);
        painter.line_segment([egui::pos2(xt, o.y + 14.0), egui::pos2(xt, rect.bottom())], Stroke::new(2.0, p.red.gamma_multiply(0.7)));
        let mut prev = now.min(start + 1.0);
        for (i, (m, target)) in rows.iter().enumerate() {
            let y = o.y + 24.0 + row_h * i as f32;
            painter.text(egui::pos2(o.x, y + row_h / 2.0), egui::Align2::LEFT_CENTER, m.version.clone().map_or(m.title.clone(), |v| format!("{v}  {}", m.title)), egui::FontId::proportional(13.0), p.text);
            let from = prev.min(*target - 0.5);
            let bar = egui::Rect::from_min_max(egui::pos2(o.x + x_of(from), y + 7.0), egui::pos2(o.x + x_of(*target), y + row_h - 7.0));
            let c = status_color(&m.effective, p);
            painter.rect_filled(bar, 4, c.gamma_multiply(0.35));
            if m.total > 0 {
                let mut done = bar;
                done.set_width(bar.width() * m.done as f32 / m.total as f32);
                painter.rect_filled(done, 4, c);
            }
            prev = *target;
        }
        if rows.is_empty() {
            ui.label("No entry has a Target date.");
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Bars run from the previous target to each entry's Target. The red line is today.").size(12.0).color(Color32::GRAY));
}

#[cfg(test)]
mod tests {
    use super::parse_month;

    #[test]
    fn months() {
        assert_eq!(parse_month("2026-11"), Some((2026 * 12 + 10) as f32 + 14.0 / 30.0));
        assert!(parse_month("2026-11-01").is_some_and(|m| m < parse_month("2026-11").unwrap()));
        assert_eq!(parse_month("next"), None);
        assert_eq!(parse_month("2026-13"), None);
    }
}
