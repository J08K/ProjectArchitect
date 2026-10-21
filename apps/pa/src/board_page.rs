//! The Board page (after YouTrack's multi-project boards): every project's roadmap entries in Now / Next / Later
//! lanes, one swimlane per project. A card moves within its own project's swimlane (it lives in that ROADMAP.md);
//! the drop writes the file through `roadmap_ui::apply`, as on the project's own board. Projects with something in
//! progress come first. An optional WIP limit (setting `board.wip_now`, 0 = off) marks Now when it holds more.

use crate::project::load_roadmap;
use crate::roadmap_ui::{self, Act, LANES};
use crate::theme::{self, Palette, avatar, caps, pal};
use crate::{App, Tab};
use egui::{Align, Layout, RichText, Stroke};
use egui_phosphor::regular as icon;
use pa_core::roadmap::{self, Roadmap};
use std::path::PathBuf;

const WIP_KEY: &str = "board.wip_now";
/// Cards per lane in one swimlane before "n more".
const CARDS: usize = 4;

/// One project's swimlane: its roadmap and lanes (Now, Next, Later).
struct Lane {
    id: i64,
    name: String,
    color: egui::Color32,
    roadmap: Roadmap,
    board: roadmap::Board,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    let lanes = swimlanes(app);
    let total = |k: usize| lanes.iter().map(|l| roadmap_ui::lanes(&l.board)[k].len()).sum::<usize>();
    let mut wip: u32 = app.setting(WIP_KEY, "0").parse().unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Board").font(theme::semibold(20.0)));
        ui.label(RichText::new("Now, next and later across every project's roadmap").color(p.dim));
        theme::right(ui, "board_page.rs:36", egui::Align::Center, |ui| {
            let wip_box = egui::DragValue::new(&mut wip)
                .range(0..=99)
                .custom_formatter(|n, _| if n == 0.0 { "Off".into() } else { format!("{n}") })
                .custom_parser(|s| if s.trim().eq_ignore_ascii_case("off") { Some(0.0) } else { s.trim().parse().ok() });
            if ui.add(wip_box).on_hover_text("Most items in Now before the lane turns orange; 0 = off").changed() {
                let r = app.store.set_setting(WIP_KEY, &wip.to_string());
                app.ok(r);
            }
            ui.label(caps("WIP limit for Now", p.faint));
        });
    });
    ui.add_space(10.0);
    // Lane headers with totals over all projects, on the swimlanes' column grid (their frame: 12 margin + 1 stroke)
    // and indented like the cards (the lane's 8 margin).
    egui::Frame::new().inner_margin(egui::Margin::symmetric(13, 0)).show(ui, |ui| ui.columns(3, |cols| {
        for (k, ui) in cols.iter_mut().enumerate() {
            let (title, sub) = LANES[k];
            let over = k == 0 && wip > 0 && total(0) > wip as usize;
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(caps(title, if over { p.orange } else { p.text }));
                let count = if k == 0 && wip > 0 { format!("{} / {wip}", total(0)) } else { total(k).to_string() };
                ui.label(caps(count, if over { p.orange } else { p.faint }));
                ui.label(RichText::new(sub).size(12.0).color(p.faint));
            });
        }
    }));
    ui.add_space(6.0);
    let mut acts: Vec<(i64, Vec<Act>)> = Vec::new();
    let mut open: Option<i64> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if lanes.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::KANBAN).size(40.0).color(p.faint));
                ui.label(RichText::new("Nothing planned or in progress. Projects need a ROADMAP.md with versions or milestones.").color(p.dim));
            });
        }
        for l in &lanes {
            let mut mine = Vec::new();
            swimlane(ui, l, p, &mut mine, &mut open);
            if !mine.is_empty() {
                acts.push((l.id, mine));
            }
            ui.add_space(10.0);
        }
    });
    for (id, a) in acts {
        roadmap_ui::apply(app, id, a);
    }
    if let Some(id) = open {
        app.project.set_section(id, "roadmap");
        app.open.push(Tab::Project(id));
    }
}

/// Projects whose roadmap has entries in Now, Next or Later: those with something in progress first, then by how
/// much is next, then by name.
fn swimlanes(app: &mut App) -> Vec<Lane> {
    let projects: Vec<pa_core::Project> = app.projects.iter().filter(|q| app.purpose(q.purpose_id).is_none_or(|u| u.track_todos)).cloned().collect();
    let mut out = Vec::new();
    for q in &projects {
        let Some(root) = q.primary().map(|c| PathBuf::from(&c.path)) else { continue };
        load_roadmap(app, q, &root);
        let Some(l) = app.project.roadmaps.get(&q.id) else { continue };
        if !l.doc.editable {
            continue;
        }
        let board = roadmap::board(&l.doc);
        if board.now.is_empty() && board.next.is_empty() && board.later.is_empty() {
            continue;
        }
        let color = q.info().and_then(|i| i.languages.first().map(|x| theme::lang_color(&x.0))).unwrap_or_else(|| theme::hash_color(&q.name));
        out.push(Lane { id: q.id, name: q.name.clone(), color, roadmap: l.doc.clone(), board });
    }
    out.sort_by_key(|l| (l.board.now.is_empty(), std::cmp::Reverse(l.board.next.len()), l.name.to_lowercase()));
    out
}

/// A one-line card: badge, title, progress; the lane says the status. Same drag payload `(project, entry key)` as
/// the project's own board.
fn card(ui: &mut egui::Ui, pid: i64, index: usize, m: &roadmap::Milestone, p: Palette) {
    let k = roadmap_ui::key(m);
    let id = egui::Id::new(("cross-card", pid, index, &k));
    let r = ui.dnd_drag_source(id, (pid, k), |ui| {
        egui::Frame::new().fill(p.raised).stroke(Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                roadmap_ui::kind_badge(ui, m, p);
                let w = (ui.available_width() - if m.total > 0 { 44.0 } else { 0.0 }).max(40.0);
                // Long titles wrap onto a second line rather than being cut off.
                ui.allocate_ui(egui::vec2(w, 0.0), |ui| {
                    ui.set_width(w);
                    ui.add(egui::Label::new(RichText::new(&m.title).font(theme::semibold(13.5))).wrap())
                });
                if m.total > 0 {
                    // Rows are top-aligned (titles wrap): nudge the smaller count onto the title's first line.
                    ui.vertical(|ui| {
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("{}/{}", m.done, m.total)).monospace().size(11.0).color(p.faint));
                    });
                }
            });
        });
    });
    let note = m.status_note.as_deref().map_or(String::new(), |n| format!("\nStatus: {n}"));
    r.response.on_hover_text(format!("{}{note}\nDrag to another lane to change its status", m.title));
}

fn swimlane(ui: &mut egui::Ui, l: &Lane, p: Palette, acts: &mut Vec<Act>, open: &mut Option<i64>) {
    let counts = format!("{} now · {} next · {} later", l.board.now.len(), l.board.next.len(), l.board.later.len());
    egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let id = egui::Id::new(("board-swimlane", l.id));
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        ui.horizontal(|ui| {
            let folded = !state.is_open();
            if theme::icon_button(ui, if folded { icon::CARET_RIGHT } else { icon::CARET_DOWN }, if folded { "Show" } else { "Hide" }).clicked() {
                state.toggle(ui);
            }
            avatar(ui, &l.name, l.color, 24.0);
            ui.label(RichText::new(&l.name).font(theme::semibold(15.0)));
            ui.label(caps(&counts, p.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::ghost(ui, format!("Open roadmap  {}", icon::ARROW_RIGHT)).clicked() {
                    *open = Some(l.id);
                }
            });
        });
        state.show_body_unindented(ui, |ui| {
            ui.add_space(6.0);
            ui.columns(3, |cols| {
                for (k, ui) in cols.iter_mut().enumerate() {
                    let idx = roadmap_ui::lanes(&l.board)[k];
                    let frame = egui::Frame::new().fill(p.bg).corner_radius(8).inner_margin(8);
                    let (_, dropped) = ui.dnd_drop_zone::<(i64, String), ()>(frame, |ui| {
                        ui.set_min_height(48.0);
                        ui.set_width(ui.available_width());
                        for &i in idx.iter().take(CARDS) {
                            card(ui, l.id, i, &l.roadmap.milestones[i], p);
                            ui.add_space(4.0);
                        }
                        if idx.len() > CARDS && theme::ghost(ui, format!("{} more", idx.len() - CARDS)).on_hover_text("Open the project's roadmap").clicked() {
                            *open = Some(l.id);
                        }
                    });
                    if let Some(d) = dropped.filter(|d| d.0 == l.id)
                        && let Some(a) = roadmap_ui::drop_act(&l.roadmap, &l.board, k, d.1.clone())
                    {
                        acts.push(a);
                    }
                }
            });
        });
        state.store(ui.ctx());
    });
}
