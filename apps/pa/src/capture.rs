//! Quick capture (M9): a modal to jot down an idea, bug, TODO or note for a project, by default the open project
//! tab's, else ProjectArchitect itself. `ops::capture` files it: the TODO file's Inbox, or the project's notes; with
//! the project's "captures become issues" on, a GitHub issue follows in the background (`github::capture_issue`).

use crate::theme::{self, caps, pal};
use crate::{App, Tab};
use egui::RichText;
use pa_core::github;
use pa_core::ops::{self, CaptureKind, Captured};

pub const SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT), egui::Key::K);

pub struct Capture {
    kind: usize,
    title: String,
    details: String,
    project: Option<i64>,
    focus: bool,
}

pub fn open(app: &mut App, active: Option<Tab>) {
    let tab = match active {
        Some(Tab::Project(id)) => Some(id),
        _ => None,
    };
    let project = tab.or_else(|| app.projects.iter().find(|q| q.name.eq_ignore_ascii_case("ProjectArchitect")).map(|q| q.id));
    app.capture = Some(Capture { kind: 0, title: String::new(), details: String::new(), project, focus: true });
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    if ctx.input_mut(|i| i.consume_shortcut(&SHORTCUT)) && app.capture.is_none() {
        let active = app.active_tab();
        open(app, active);
    }
    let names: Vec<(i64, String)> = app.projects.iter().map(|q| (q.id, q.name.clone())).collect();
    let Some(c) = app.capture.as_mut() else { return };
    let p = pal(ctx);
    let (mut save, mut cancel) = (false, ctx.input(|i| i.key_pressed(egui::Key::Escape)));
    // Clicks outside do not close it: only Escape and Cancel throw a typed capture away.
    let w = 460.0;
    let (_, closed) = theme::dialog(ctx, "capture", "Capture", w, |ui| {
        let kinds: Vec<(String, Option<usize>)> = CaptureKind::ALL.iter().map(|k| (k.label().to_string(), None)).collect();
        if let Some(k) = theme::segmented(ui, &kinds, c.kind) {
            c.kind = k;
        }
        ui.add_space(8.0);
        let title = ui.add(theme::field(&mut c.title, w).hint_text("One line"));
        if std::mem::take(&mut c.focus) {
            title.request_focus();
        }
        save |= title.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(6.0);
        ui.add(egui::TextEdit::multiline(&mut c.details).desired_rows(4).desired_width(w).margin(egui::vec2(10.0, 7.0)).hint_text("Details (optional)"));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(caps("Project", p.faint));
            let text = c.project.and_then(|id| names.iter().find(|n| n.0 == id)).map_or("Choose…", |n| n.1.as_str());
            theme::select(ui, "capture_project", 260.0, text, |ui| {
                for (id, name) in &names {
                    ui.selectable_value(&mut c.project, Some(*id), name);
                }
            });
        });
        let where_to = if CaptureKind::ALL[c.kind] == CaptureKind::Note {
            "Saved as a note of the project, in the app".to_string()
        } else {
            format!("Added to “{}” in the project's TODO file", ops::INBOX)
        };
        ui.label(RichText::new(where_to).size(12.0).color(p.dim));
        ui.add_space(12.0);
        // Footer: the shortcut left, Cancel then Save on the right.
        ui.horizontal(|ui| {
            ui.label(caps("Ctrl+Shift+K", p.faint));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = c.project.is_some() && !c.title.trim().is_empty();
                save &= ready;
                save |= ui.add_enabled_ui(ready, |ui| theme::primary(ui, "Save")).inner.clicked();
                cancel |= theme::secondary(ui, "Cancel").clicked();
            });
        });
    });
    if cancel || closed {
        app.capture = None;
    } else if save {
        submit(app, ctx);
    }
}

fn submit(app: &mut App, ctx: &egui::Context) {
    let Some(c) = app.capture.take() else { return };
    let Some(q) = c.project.and_then(|id| app.projects.iter().find(|q| q.id == id)).cloned() else { return };
    let kind = CaptureKind::ALL[c.kind];
    match ops::capture(&app.store, &q, kind, &c.title, &c.details) {
        Ok(Captured::Todo { file, id }) => {
            app.project.todos.remove(&q.id);
            app.refresh_project(ctx, q.id);
            app.message = Some(format!("{} captured in {}", kind.label(), file.display()));
            // The project's "captures become issues": open the issue too, then write its number on the line.
            if github::captures_become_issues(&app.store, &q) {
                let (title, details) = (c.title.clone(), c.details.clone());
                app.background(ctx, move || {
                    let r = github::capture_issue(&q, kind, &title, &details, &id);
                    Box::new(move |app: &mut App| {
                        app.project.todos.remove(&q.id);
                        app.message = Some(match r {
                            Ok((n, _)) => format!("{} captured in {} and as issue #{n}", kind.label(), file.display()),
                            Err(e) => format!("{} captured in {}; no issue: {e}", kind.label(), file.display()),
                        });
                    })
                });
            }
        }
        Ok(Captured::Note(_)) => {
            app.project.forget_notes();
            app.message = Some(format!("Note captured for {}", q.name));
        }
        Err(e) => {
            app.message = Some(format!("Not captured: {e}"));
            app.capture = Some(c);
        }
    }
}
