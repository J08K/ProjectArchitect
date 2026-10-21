//! "Change TODO style": rewrite a project's TODO file in another convention (checkboxes, bullets, numbered, plain
//! lines; done = ticked, moved to a section, or deleted), or only tell the app how to read the file as it is.
//! The rewrite goes to the working copy, or is committed to a branch without touching the working tree, optionally
//! pushed with a pull request opened (`pa_core::git`). Git and GitHub run on a background thread.
//! The same dialog converts a table-style roadmap into roadmap headings (`roadmap::convert_roadmap`).

use crate::App;
use crate::theme::{self, caps, pal};
use egui::RichText;
use pa_core::{git, roadmap};
use pa_core::todo::{self, Completion, Marker, Style};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Copy, PartialEq)]
enum Done {
    Tick,
    Move,
    Delete,
}

pub struct Restyle {
    project: i64,
    name: String,
    repo: PathBuf,
    path: PathBuf,
    rel: String,
    before: String,
    current_style: Style,
    marker: Marker,
    done: Done,
    section: String,
    to_branch: bool,
    branch: String,
    base: String,
    branches: Vec<String>,
    current_branch: Option<String>,
    can_pr: bool,
    pr: bool,
    running: Option<Receiver<Result<Outcome, String>>>,
    /// Converting a table roadmap instead of restyling the TODO file.
    roadmap: bool,
}

struct Outcome {
    pr: Option<String>,
    /// The commit landed on the checked-out branch, so the working copy now has the new style.
    in_working_copy: bool,
}

impl Restyle {
    /// The converted text.
    fn after(&self) -> String {
        if self.roadmap { roadmap::convert_roadmap(&self.before) } else { todo::convert_to(&self.before, &self.target()) }
    }

    fn what(&self) -> String {
        if self.roadmap { "Convert the roadmap table to version and milestone headings".into() } else { format!("Change TODO style to: {}", self.target().label()) }
    }

    fn target(&self) -> Style {
        let completion = match (self.done, self.marker) {
            (Done::Tick, Marker::Checkbox) => Completion::Tick,
            (Done::Move, _) => Completion::MoveTo(if self.section.trim().is_empty() { "Done".into() } else { self.section.trim().to_string() }),
            _ => Completion::Delete,
        };
        Style { marker: self.marker, completion }
    }
}

/// Open the dialog to convert a project's loaded, table-style roadmap.
pub fn open_roadmap(app: &mut App, project: i64, root: &Path) {
    let Some(l) = app.project.roadmaps.get(&project) else { return };
    let (path, text) = (l.path.clone(), l.text.clone());
    open_for(app, project, root, path, text, Style::CHECKLIST, true);
}

/// Open the dialog for a project's loaded TODO file.
pub fn open(app: &mut App, project: i64, root: &Path) {
    let Some(l) = app.project.todos.get(&project) else { return };
    let (path, text, style) = (l.path.clone(), l.text.clone(), l.doc.style.clone());
    open_for(app, project, root, path, text, style, false);
}

fn open_for(app: &mut App, project: i64, root: &Path, path: PathBuf, before: String, style: Style, is_roadmap: bool) {
    let Some(q) = app.projects.iter().find(|q| q.id == project) else { return };
    let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
    let is_git = root.join(".git").exists();
    let current_branch = if is_git { git::current_branch(root) } else { None };
    let branches = if is_git { git::branches(root).unwrap_or_default() } else { Vec::new() };
    let base = git::default_branch(root).or_else(|| current_branch.clone()).unwrap_or_else(|| "main".into());
    // Default target: the app format, unless the file already uses it.
    let (marker, done, section) = if style == Style::CHECKLIST { (Marker::Bullet, Done::Delete, String::new()) } else { (Marker::Checkbox, Done::Tick, String::new()) };
    let can_pr = is_git && git::has_remote(root) && git::gh_available();
    app.restyle = Some(Restyle {
        project,
        name: q.name.clone(),
        repo: root.to_path_buf(),
        path,
        rel,
        before,
        current_style: style,
        marker,
        done,
        section,
        to_branch: false,
        branch: if is_roadmap { "roadmap-headings".into() } else { "todo-style".into() },
        base,
        branches,
        current_branch,
        can_pr,
        pr: false,
        running: None,
        roadmap: is_roadmap,
    });
}

/// The style presets offered in project settings (the detected style first when it is not one of them).
pub fn presets(detected: &Style) -> Vec<Style> {
    let mut v = vec![
        Style::CHECKLIST,
        Style { marker: Marker::Bullet, completion: Completion::MoveTo("Done".into()) },
        Style { marker: Marker::Bullet, completion: Completion::Delete },
        Style { marker: Marker::Numbered, completion: Completion::Delete },
        Style { marker: Marker::Line, completion: Completion::Delete },
    ];
    if !v.contains(detected) {
        v.insert(0, detected.clone());
    }
    v
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let p = pal(ctx);
    poll(app, ctx);
    let Some(r) = app.restyle.as_mut() else { return };
    let mut open = true;
    let mut action: Option<&str> = None;
    let busy = r.running.is_some();
    let title = if r.roadmap { format!("Convert roadmap · {}", r.name) } else { format!("TODO style · {}", r.name) };
    egui::Window::new(title).open(&mut open).default_size([1040.0, 720.0]).collapsible(false).show(ctx, |ui| {
        if r.roadmap {
            ui.label(RichText::new(format!("{}  ·  a table roadmap becomes one `##` section per row (versions and milestones)", r.rel)).color(p.dim));
        } else {
            ui.label(RichText::new(format!("{}  ·  now: {}", r.rel, r.current_style.label())).color(p.dim));
        }
        ui.add_space(8.0);
        if !r.roadmap {
        ui.horizontal(|ui| {
            ui.label(caps("Items", p.faint));
            for (m, label) in [(Marker::Checkbox, "- [ ] Checkboxes"), (Marker::Bullet, "- Bullets"), (Marker::Numbered, "1. Numbered"), (Marker::Line, "Plain lines")] {
                ui.selectable_value(&mut r.marker, m, label);
            }
        });
        ui.horizontal(|ui| {
            ui.label(caps("When done", p.faint));
            if r.marker == Marker::Checkbox {
                ui.selectable_value(&mut r.done, Done::Tick, "Tick [x]");
            } else if r.done == Done::Tick {
                r.done = Done::Delete;
            }
            ui.selectable_value(&mut r.done, Done::Move, "Move to section");
            ui.selectable_value(&mut r.done, Done::Delete, "Delete");
            if r.done == Done::Move {
                ui.add(theme::field(&mut r.section, 200.0).hint_text("Done"));
            }
        });
        }
        let after = r.after();
        let after_title = if r.roadmap { "After".to_string() } else { format!("After: {}", r.target().label()) };
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            for (ui, (title, text)) in cols.iter_mut().zip([("Now", &r.before), (after_title.as_str(), &after)]) {
                ui.label(caps(title, p.faint));
                egui::ScrollArea::both().id_salt(title).max_height(340.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(text.as_str()).monospace().size(12.0)).extend());
                });
            }
        });
        ui.add_space(10.0);
        ui.label(caps("Where", p.faint));
        ui.radio_value(&mut r.to_branch, false, "Write the working copy (no commit; a backup is kept in the app's data folder)");
        ui.add_enabled_ui(r.current_branch.is_some(), |ui| {
            ui.radio_value(&mut r.to_branch, true, "Commit to a branch (the working tree is left alone unless it is the checked-out branch)");
        });
        if r.to_branch {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label("Branch");
                ui.add(theme::field(&mut r.branch, 220.0));
                ui.menu_button(RichText::new(egui_phosphor::regular::CARET_DOWN).size(14.0), |ui| {
                    for b in r.branches.clone() {
                        if ui.button(&b).clicked() {
                            r.branch = b;
                        }
                    }
                });
                ui.label("from");
                theme::select(ui, "restyle_base", 160.0, r.base.clone(), |ui| {
                    for b in r.branches.clone() {
                        ui.selectable_value(&mut r.base, b.clone(), b);
                    }
                });
                if Some(&r.branch) == r.current_branch.as_ref() {
                    ui.label(RichText::new("checked out: committed in the working copy").color(p.orange));
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.add_enabled(r.can_pr, egui::Checkbox::new(&mut r.pr, "Push the branch and open a pull request"))
                    .on_disabled_hover_text("Needs an `origin` remote and the GitHub CLI (`gh`) signed in");
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
                ui.label("Working…");
            } else {
                let go = match (r.to_branch, r.pr) {
                    (false, _) => "Convert".to_string(),
                    (true, false) => format!("Convert and commit to {}", r.branch),
                    (true, true) => format!("Convert, push {} and open a PR", r.branch),
                };
                if theme::primary(ui, go).clicked() {
                    action = Some("convert");
                }
                if !r.roadmap && theme::secondary(ui, "Only read the file with this style")
                    .on_hover_text("Keep the file as it is and tell the app to use this style for it")
                    .clicked()
                {
                    action = Some("style_only");
                }
            }
        });
    });
    if !open && !busy {
        app.restyle = None;
        return;
    }
    match action {
        Some("convert") => convert(app, ctx),
        Some("style_only") => {
            let (project, style) = { (r_project(app), app.restyle.as_ref().map(|r| r.target().encode())) };
            if let (Some(project), Some(style)) = (project, style) {
                set_style(app, project, Some(style));
                app.message = Some("TODO style saved for this project; the file was not changed".into());
            }
            app.restyle = None;
        }
        _ => {}
    }
}

fn r_project(app: &App) -> Option<i64> {
    app.restyle.as_ref().map(|r| r.project)
}

/// Store the style override and drop the cached file so it is read again with it.
fn set_style(app: &mut App, project: i64, style: Option<String>) {
    if let Some(mut q) = app.projects.iter().find(|q| q.id == project).cloned() {
        q.todo_style = style;
        app.save_project(q);
    }
    app.project.todos.remove(&project);
}

fn convert(app: &mut App, ctx: &egui::Context) {
    let Some(r) = app.restyle.as_mut() else { return };
    let (target, after, is_roadmap) = (r.target(), r.after(), r.roadmap);
    if !r.to_branch {
        let (project, path, before) = (r.project, r.path.clone(), r.before.clone());
        if let Err(e) = todo::backup(&app.data_dir, &path) {
            app.message = Some(format!("Backup failed, nothing written: {e}"));
            return;
        }
        match todo::save(&path, &before, &after) {
            Ok(()) => {
                if is_roadmap {
                    app.project.roadmaps.remove(&project);
                } else {
                    set_style(app, project, Some(target.encode()));
                }
                app.message = Some(format!("Converted {}", path.display()));
                app.restyle = None;
            }
            Err(e) => app.message = Some(format!("Not converted: {e}")),
        }
        return;
    }
    let (repo, rel, path, before) = (r.repo.clone(), r.rel.clone(), r.path.clone(), r.before.clone());
    let (branch, base, pr, on_current) = (r.branch.trim().to_string(), r.base.clone(), r.pr, r.current_branch.as_deref() == Some(r.branch.trim()));
    let message = r.what();
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let run = || -> Result<Outcome, String> {
            if on_current && std::fs::read_to_string(&path).map_err(|e| e.to_string())? != before {
                return Err(format!("{} changed since it was read; reopen the dialog", path.display()));
            }
            git::commit_file_to_branch(&repo, &rel, &after, &branch, &base, &message).map_err(|e| e.to_string())?;
            let url = if pr {
                git::push(&repo, &branch).map_err(|e| e.to_string())?;
                let body = "Generated by ProjectArchitect. Only the file's layout changes; item texts are unchanged.";
                Some(git::open_pr(&repo, &branch, &base, &message, body).map_err(|e| e.to_string())?)
            } else {
                None
            };
            Ok(Outcome { pr: url, in_working_copy: on_current })
        };
        let _ = tx.send(run());
        ctx.request_repaint();
    });
    r.running = Some(rx);
}

fn poll(app: &mut App, ctx: &egui::Context) {
    let Some(r) = app.restyle.as_mut() else { return };
    let Some(rx) = &r.running else { return };
    let Ok(result) = rx.try_recv() else { return };
    r.running = None;
    let (project, branch, style, is_roadmap) = (r.project, r.branch.clone(), r.target().encode(), r.roadmap);
    match result {
        Ok(out) => {
            if out.in_working_copy && is_roadmap {
                app.project.roadmaps.remove(&project);
            } else if out.in_working_copy {
                set_style(app, project, Some(style));
            }
            app.message = Some(match &out.pr {
                Some(url) => format!("Committed to {branch} and opened {url}"),
                None if out.in_working_copy => format!("Converted and committed on {branch}"),
                None => format!("Committed to {branch}; the working copy is unchanged until you merge it"),
            });
            if let Some(url) = out.pr {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            app.restyle = None;
        }
        Err(e) => app.message = Some(format!("Not converted: {e}")),
    }
}
