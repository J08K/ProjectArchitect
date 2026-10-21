//! The Tasks section (SPEC § 7.7): the project's named commands (kept in the app's store), suggestions from its
//! manifests, and runs with live output, a stop button and the exit code (`pa_core::tasks`). Runs belong to the app,
//! not to the tab, so a dev server keeps running when its tab closes; the overview shows that it runs. The last
//! [`KEEP_RUNS`] runs per project stay in memory; closing the app stops what still runs.

use crate::App;
use crate::theme::{self, Palette, caps, card, chip, tag};
use egui::{Align, Layout, RichText, vec2};
use egui_phosphor::regular as icon;
use pa_core::Project;
use pa_core::tasks::{self, KEEP_RUNS, Run, Task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Task runs and the Tasks section's state.
#[derive(Default)]
pub struct Runs {
    /// Per project, oldest first, each with a serial number.
    runs: HashMap<i64, Vec<(u64, Run)>>,
    next: u64,
    /// The run whose output is shown, per project.
    shown: HashMap<i64, u64>,
    tasks: HashMap<i64, Vec<Task>>,
    suggestions: HashMap<PathBuf, Vec<(String, String)>>,
    editing: Option<Task>,
    /// New task fields per project: name, command.
    new: HashMap<i64, (String, String)>,
}

impl Runs {
    /// How many tasks of the project are running.
    pub fn running(&self, project: i64) -> usize {
        self.runs.get(&project).map_or(0, |v| v.iter().filter(|(_, r)| r.running()).count())
    }

    /// Drop cached task lists (re-read from the store), e.g. after the MCP server added one.
    pub fn forget_tasks(&mut self) {
        self.tasks.clear();
    }

    fn running_task(&self, project: i64, task: i64) -> Option<&Run> {
        self.runs.get(&project)?.iter().map(|(_, r)| r).find(|r| r.task_id == task && r.running())
    }
}

enum Act {
    Run(Task),
    Stop(i64),
    Edit(Task),
    Save(Task),
    Cancel,
    Delete(i64),
    Add(String, String),
    Show(u64),
    Forget(u64),
}

pub fn show(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    let id = q.id;
    if !app.runs.tasks.contains_key(&id) {
        let list = app.store.tasks(id);
        let list = app.ok(list).unwrap_or_default();
        app.runs.tasks.insert(id, list);
    }
    let tasks = app.runs.tasks[&id].clone();
    let suggestions = app.runs.suggestions.entry(root.to_path_buf()).or_insert_with(|| tasks::suggestions(root)).clone();
    let mut acts: Vec<Act> = Vec::new();

    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Tasks", p.faint));
        ui.add_space(4.0);
        if tasks.is_empty() {
            ui.label(RichText::new("No tasks yet: add a command below, or take one of the suggestions.").color(p.dim));
        }
        for t in &tasks {
            match app.runs.editing.as_mut().filter(|e| e.id == t.id) {
                Some(e) => edit_form(ui, e, p, &mut acts),
                None => task_row(ui, t, app.runs.running_task(id, t.id).is_some(), p, &mut acts),
            }
        }
        ui.add_space(6.0);
        let (name, command) = app.runs.new.entry(id).or_default();
        ui.horizontal(|ui| {
            ui.add(theme::field(name, 180.0).hint_text("Name"));
            let w = (ui.available_width() - 90.0).max(200.0);
            let r = ui.add(theme::field(command, w).hint_text("Command, e.g. cargo run -p pa").font(egui::TextStyle::Monospace));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (theme::secondary(ui, format!("{}  Add", icon::PLUS)).clicked() || enter) && !command.trim().is_empty() {
                let label = if name.trim().is_empty() { command.trim().to_string() } else { name.trim().to_string() };
                acts.push(Act::Add(label, command.trim().to_string()));
                name.clear();
                command.clear();
            }
        });
    });

    let fresh: Vec<&(String, String)> = suggestions.iter().filter(|(_, c)| !tasks.iter().any(|t| t.command == *c)).collect();
    if !fresh.is_empty() {
        ui.add_space(12.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(caps("Suggested by the manifests", p.faint));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (name, command) in fresh {
                    if theme::secondary(ui, format!("{}  {command}", icon::PLUS)).on_hover_text(format!("Add the task “{name}”")).clicked() {
                        acts.push(Act::Add(name.clone(), command.clone()));
                    }
                }
            });
        });
    }

    ui.add_space(12.0);
    runs_card(app, ui, id, p, &mut acts);

    for a in acts {
        apply(app, ui.ctx(), q, root, a);
    }
}

fn task_row(ui: &mut egui::Ui, t: &Task, running: bool, p: Palette, acts: &mut Vec<Act>) {
    ui.horizontal_wrapped(|ui| {
        ui.set_min_height(34.0);
        ui.label(RichText::new(&t.name).font(theme::semibold(14.0)));
        ui.add(egui::Label::new(RichText::new(&t.command).monospace().size(12.5).color(p.dim)).truncate());
        if !t.cwd.trim().is_empty() {
            tag(ui, format!("{} {}", icon::FOLDER, t.cwd.trim()), p.dim);
        }
        if !tasks::parse_env(&t.env).is_empty() {
            tag(ui, "env", p.purple).on_hover_text(t.env.trim());
        }
        theme::right(ui, "tasks_ui.rs:135", egui::Align::Center, |ui| {
            if theme::icon_button(ui, icon::TRASH, "Delete the task").clicked() {
                acts.push(Act::Delete(t.id));
            }
            if theme::icon_button(ui, icon::PENCIL_SIMPLE, "Edit").clicked() {
                acts.push(Act::Edit(t.clone()));
            }
            if running {
                if theme::secondary(ui, format!("{}  Stop", icon::STOP)).clicked() {
                    acts.push(Act::Stop(t.id));
                }
                chip(ui, "running", p.green);
            } else if theme::secondary(ui, format!("{}  Run", icon::PLAY)).on_hover_text(format!("Run in the checkout: {}", t.command)).clicked() {
                acts.push(Act::Run(t.clone()));
            }
        });
    });
}

fn edit_form(ui: &mut egui::Ui, e: &mut Task, p: Palette, acts: &mut Vec<Act>) {
    egui::Frame::new().fill(p.raised).corner_radius(8).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new(("task_edit", e.id)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            let w = (ui.available_width() - 120.0).max(260.0);
            ui.label(RichText::new("Name").color(p.dim));
            ui.add(theme::field(&mut e.name, w));
            ui.end_row();
            ui.label(RichText::new("Command").color(p.dim));
            ui.add(theme::field(&mut e.command, w).font(egui::TextStyle::Monospace));
            ui.end_row();
            ui.label(RichText::new("Folder").color(p.dim));
            ui.add(theme::field(&mut e.cwd, w).hint_text("relative to the checkout; empty = the checkout"));
            ui.end_row();
            ui.label(RichText::new("Environment").color(p.dim));
            ui.add(egui::TextEdit::multiline(&mut e.env).desired_rows(2).desired_width(w).margin(vec2(10.0, 7.0)).font(egui::TextStyle::Monospace).hint_text("KEY=VALUE, one per line"));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if theme::primary(ui, "Save").clicked() && !e.command.trim().is_empty() {
                acts.push(Act::Save(e.clone()));
            }
            if theme::ghost(ui, "Cancel").clicked() {
                acts.push(Act::Cancel);
            }
        });
    });
}

fn runs_card(app: &mut App, ui: &mut egui::Ui, id: i64, p: Palette, acts: &mut Vec<Act>) {
    let Some(list) = app.runs.runs.get(&id).filter(|l| !l.is_empty()) else { return };
    let shown = app.runs.shown.get(&id).copied().filter(|s| list.iter().any(|(n, _)| n == s)).unwrap_or(list[list.len() - 1].0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(caps("Runs", p.faint));
            ui.label(RichText::new(format!("the last {KEEP_RUNS}, kept while the app runs; they go on when the tab closes")).size(12.0).color(p.faint));
        });
        ui.add_space(4.0);
        for (serial, run) in list.iter().rev() {
            ui.horizontal(|ui| {
                let (text, color) = status(run, p);
                let label = format!("{}\t{}", run.name, ago(run.elapsed()));
                if theme::button(ui, &label, egui::FontId::proportional(13.5), 0.0, theme::Kind::Nav(*serial == shown), vec2(260.0, 30.0)).clicked() {
                    acts.push(Act::Show(*serial));
                }
                chip(ui, text, color);
                ui.label(RichText::new(&run.command).monospace().size(12.0).color(p.faint));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if run.running() {
                        if theme::secondary(ui, format!("{}  Stop", icon::STOP)).clicked() {
                            acts.push(Act::Stop(run.task_id));
                        }
                    } else if theme::icon_button(ui, icon::X, "Forget this run").clicked() {
                        acts.push(Act::Forget(*serial));
                    }
                });
            });
        }
        ui.add_space(6.0);
        let Some((_, run)) = list.iter().find(|(n, _)| *n == shown) else { return };
        let running = run.running();
        let log = run.log();
        ui.horizontal(|ui| {
            let lines = log.lines.len() + log.dropped;
            ui.label(caps(format!("Output · {lines} lines{}", if log.dropped > 0 { format!(" ({} oldest dropped)", log.dropped) } else { String::new() }), p.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::ghost(ui, format!("{}  Copy", icon::COPY)).clicked() {
                    ui.ctx().copy_text(log.text());
                }
            });
        });
        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(6).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Rows touch: show_rows counts the spacing in, so it is set before.
            ui.spacing_mut().item_spacing.y = 0.0;
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
            if log.lines.is_empty() {
                ui.label(RichText::new(if running { "Waiting for output…" } else { "No output." }).monospace().color(p.faint));
                return;
            }
            egui::ScrollArea::both().id_salt(("run_output", id, shown)).max_height(360.0).stick_to_bottom(true).auto_shrink([false, true]).show_rows(
                ui,
                row_h,
                log.lines.len(),
                |ui, range| {
                    for (err, line) in log.lines.range(range) {
                        let rt = RichText::new(line).monospace();
                        ui.add(egui::Label::new(if *err { rt.color(p.orange) } else { rt }).extend());
                    }
                },
            );
        });
    });
}

/// A run's state as a chip: running, exit code, or stopped.
fn status(run: &Run, p: Palette) -> (String, egui::Color32) {
    let exit = run.log().exit;
    match exit {
        None => ("running".into(), p.green),
        Some(_) if run.was_stopped() => ("stopped".into(), p.dim),
        Some(Some(0)) => ("exit 0".into(), p.green),
        Some(Some(code)) => (format!("exit {code}"), p.red),
        Some(None) => ("killed".into(), p.red),
    }
}

/// "12 s", "4 min", "2 h 5 min".
fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        ..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

fn apply(app: &mut App, ctx: &egui::Context, q: &Project, root: &Path, a: Act) {
    let id = q.id;
    match a {
        Act::Run(t) => {
            let repaint = ctx.clone();
            match Run::start(&t, root, move || repaint.request_repaint_after(Duration::from_millis(50))) {
                Ok(run) => {
                    let serial = app.runs.next;
                    app.runs.next += 1;
                    let list = app.runs.runs.entry(id).or_default();
                    list.push((serial, run));
                    // Drop the oldest finished runs; running ones always stay.
                    while list.len() > KEEP_RUNS
                        && let Some(i) = list.iter().position(|(_, r)| !r.running())
                    {
                        list.remove(i);
                    }
                    app.runs.shown.insert(id, serial);
                }
                Err(e) => app.message = Some(format!("Could not start {}: {e}", t.name)),
            }
        }
        Act::Stop(task) => {
            if let Some(r) = app.runs.running_task(id, task) {
                r.stop();
            }
        }
        Act::Edit(t) => app.runs.editing = Some(t),
        Act::Cancel => app.runs.editing = None,
        Act::Save(t) => {
            let r = app.store.update_task(&t);
            app.ok(r);
            app.runs.editing = None;
            app.runs.tasks.remove(&id);
        }
        Act::Delete(task) => {
            let r = app.store.delete_task(task);
            app.ok(r);
            app.runs.tasks.remove(&id);
        }
        Act::Add(name, command) => {
            let r = app.store.add_task(id, &name, &command, "", "");
            app.ok(r);
            app.runs.tasks.remove(&id);
        }
        Act::Show(serial) => {
            app.runs.shown.insert(id, serial);
        }
        Act::Forget(serial) => {
            if let Some(list) = app.runs.runs.get_mut(&id) {
                list.retain(|(n, _)| *n != serial);
            }
        }
    }
}
