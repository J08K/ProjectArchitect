//! Templates (SPEC § 9, D-10, D-21): the Templates page (list, edit `template.toml`, open the folder, duplicate, make
//! a template from a project) and the New Project dialog: template → prompts → parent folder and group → preview →
//! create, with the steps' output streamed. A failed step leaves the partly created folder and says so.

use crate::theme::{self, caps, card, pal, tag};
use crate::{App, Tab};
use egui::{Align, Layout, RichText, vec2};
use egui_phosphor::regular as icon;
use pa_core::template::{self, Action, Listed, Planned, Template};
use pa_core::todo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

#[derive(Default)]
pub struct State {
    /// The templates folder and its templates, read when the page or the dialog opens.
    list: Option<(PathBuf, Vec<Listed>)>,
    edit: Option<TomlEdit>,
    make: Option<Make>,
}

struct TomlEdit {
    path: PathBuf,
    read: String,
    text: String,
}

/// "Make template from a project": the project, the new template's name and the top-level entries to copy.
struct Make {
    project: Option<i64>,
    name: String,
    picks: Vec<(String, bool, bool)>,
}

pub struct NewProject {
    templates: Vec<Template>,
    pick: usize,
    values: HashMap<String, String>,
    parent: String,
    group: Option<i64>,
    stage: Stage,
    log: Vec<String>,
}

enum Stage {
    Form(Option<String>),
    Preview(Planned),
    Running(Planned, Receiver<Msg>),
    Failed(String),
}

enum Msg {
    Line(String),
    Done(Result<(), String>),
}

/// Seed and read the templates folder once (and track it, D-21). `None` with the error shown.
fn ensure_list(app: &mut App) -> Option<PathBuf> {
    if let Some((dir, _)) = &app.templates.list {
        return Some(dir.clone());
    }
    let r = template::ensure(&app.store, &app.data_dir);
    let dir = app.ok(r)?;
    app.templates.list = Some((dir.clone(), template::list(&dir)));
    app.reload();
    Some(dir)
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    let Some(dir) = ensure_list(app) else { return };
    let mut reload = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Templates").font(theme::semibold(22.0)));
        theme::right(ui, "templates_ui.rs:76", egui::Align::Center, |ui| {
            if theme::primary(ui, format!("{}  New project", icon::FOLDER_PLUS)).clicked() {
                open_new(app, None);
            }
            if theme::secondary(ui, format!("{}  Open folder", icon::FOLDER_OPEN)).clicked() {
                app.launch(crate::LAUNCHERS[1].0, crate::LAUNCHERS[1].2, &dir.display().to_string());
            }
            if theme::secondary(ui, "Change folder…").on_hover_text("Where templates live (setting templates.dir)").clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder()
            {
                let r = app.store.set_setting(template::DIR, &pa_core::norm(&d));
                app.ok(r);
                reload = true;
            }
            if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Read the folder again").clicked() {
                reload = true;
            }
        });
    });
    ui.add(egui::Label::new(RichText::new(format!("{}  {}", icon::FOLDER, dir.display())).monospace().color(p.dim)).truncate());
    ui.label(RichText::new("A template is a folder with template.toml (questions and steps) and files/ (copied into the new project, {{placeholders}} filled). The folder is a project in the app too.").color(p.dim));
    ui.add_space(12.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("templates").show(ui, |ui| {
        ui.set_max_width(1000.0);
        make_card(app, ui, &dir);
        ui.add_space(12.0);
        let listed: Vec<(PathBuf, Result<Template, String>)> =
            app.templates.list.as_ref().map(|l| l.1.iter().map(|x| (x.dir.clone(), x.template.clone())).collect()).unwrap_or_default();
        for (tdir, t) in listed {
            template_card(app, ui, &tdir, t, &mut reload);
            ui.add_space(12.0);
        }
    });
    if reload {
        app.templates.list = None;
    }
}

fn template_card(app: &mut App, ui: &mut egui::Ui, tdir: &Path, t: Result<Template, String>, reload: &mut bool) {
    let p = pal(ui.ctx());
    let file = tdir.join(template::FILE);
    let editing = app.templates.edit.as_ref().is_some_and(|e| e.path == file);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let name = t.as_ref().map_or_else(|_| pa_core::folder_name(&tdir.to_string_lossy()), |t| t.name.clone());
            ui.label(RichText::new(name).font(theme::semibold(16.0)));
            ui.label(RichText::new(pa_core::folder_name(&tdir.to_string_lossy())).monospace().size(12.0).color(p.faint));
            if let Ok(t) = &t {
                for x in &t.tags {
                    tag(ui, x, theme::hash_color(x));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Ok(t) = &t
                    && theme::icon_button(ui, icon::COPY, "Duplicate").clicked()
                {
                    match template::duplicate(t) {
                        Ok(d) => app.message = Some(format!("Duplicated to {}", d.display())),
                        Err(e) => app.message = Some(format!("Not duplicated: {e}")),
                    }
                    *reload = true;
                }
                if theme::icon_button(ui, icon::FOLDER_OPEN, "Open the template's folder").clicked() {
                    app.launch(crate::LAUNCHERS[1].0, crate::LAUNCHERS[1].2, &tdir.display().to_string());
                }
                if !editing && theme::icon_button(ui, icon::PENCIL_SIMPLE, "Edit template.toml").clicked() {
                    let read = std::fs::read_to_string(&file).unwrap_or_default();
                    app.templates.edit = Some(TomlEdit { path: file.clone(), text: read.clone(), read });
                }
                if t.is_ok() && theme::secondary(ui, "Use…").clicked() {
                    open_new(app, Some(tdir.to_path_buf()));
                }
            });
        });
        match &t {
            Ok(t) => {
                if !t.description.is_empty() {
                    ui.label(RichText::new(&t.description).color(p.dim));
                }
                ui.add_space(4.0);
                for (i, s) in t.steps.iter().enumerate() {
                    let line = match (&s.run, &s.overlay) {
                        (Some(r), _) => format!("{}  $ {r}{}", i + 1, s.cwd.as_deref().map(|c| format!("   (in {c})")).unwrap_or_default()),
                        (_, Some(o)) => format!("{}  copy {o}/ into the project", i + 1),
                        _ => String::new(),
                    };
                    ui.label(RichText::new(line).monospace().size(12.0).color(p.dim));
                }
                let asks: Vec<String> = t.questions().into_iter().map(|q| q.key).collect();
                ui.label(caps(format!("asks: {}", asks.join(" · ")), p.faint));
            }
            Err(e) => {
                ui.label(RichText::new(format!("{}  {e}", icon::WARNING)).color(p.red));
            }
        }
        if editing {
            toml_editor(app, ui, tdir, reload);
        }
    });
}

fn toml_editor(app: &mut App, ui: &mut egui::Ui, tdir: &Path, reload: &mut bool) {
    let p = pal(ui.ctx());
    let Some(e) = app.templates.edit.as_mut() else { return };
    ui.add_space(8.0);
    egui::Frame::new().fill(p.bg).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::TextEdit::multiline(&mut e.text).code_editor().frame(egui::Frame::NONE).desired_rows(18).desired_width(f32::INFINITY));
    });
    let check = template::parse(&e.text, tdir);
    match &check {
        Ok(_) => ui.label(RichText::new(format!("{}  valid", icon::CHECK_CIRCLE)).size(12.0).color(p.faint)),
        Err(err) => ui.label(RichText::new(format!("{}  {err}", icon::WARNING)).size(12.0).color(p.red)),
    };
    let (mut save, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        save = ui.add_enabled_ui(check.is_ok() && e.text != e.read, |ui| theme::primary(ui, "Save")).inner.clicked();
        cancel = theme::ghost(ui, "Cancel").clicked();
    });
    if cancel {
        app.templates.edit = None;
    } else if save {
        let Some(e) = app.templates.edit.take() else { return };
        let r = todo::backup(&app.data_dir, &e.path)
            .map_err(|x| x.to_string())
            .and_then(|_| todo::save(&e.path, &e.read, &e.text).map_err(|x| x.to_string()));
        match r {
            Ok(()) => app.message = Some(format!("Saved {}", e.path.display())),
            Err(x) => {
                app.message = Some(format!("Not saved: {x}"));
                app.templates.edit = Some(e);
            }
        }
        *reload = true;
    }
}

/// The checked entries by default: the files a new project usually starts from.
fn picked_by_default(name: &str) -> bool {
    matches!(name, ".claude" | "CLAUDE.md" | "AGENTS.md" | ".gitignore" | "TODO.md" | "ROADMAP.md" | ".mcp.json" | "rustfmt.toml" | ".editorconfig")
}

fn make_card(app: &mut App, ui: &mut egui::Ui, dir: &Path) {
    let p = pal(ui.ctx());
    if app.templates.make.is_none() {
        if theme::secondary(ui, format!("{}  Make a template from a project…", icon::MAGIC_WAND))
            .on_hover_text("Copy chosen files of a project into a new template")
            .clicked()
        {
            app.templates.make = Some(Make { project: None, name: String::new(), picks: Vec::new() });
        }
        return;
    }
    let projects: Vec<(i64, String, Option<String>)> =
        app.projects.iter().map(|q| (q.id, q.name.clone(), q.primary().map(|c| c.path.clone()))).collect();
    let Some(m) = app.templates.make.as_mut() else { return };
    let (mut go, mut cancel) = (false, false);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Make a template from a project", p.faint));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Project").color(p.dim));
            let text = m.project.and_then(|id| projects.iter().find(|q| q.0 == id)).map_or("Choose…", |q| q.1.as_str()).to_string();
            let before = m.project;
            theme::select(ui, "make_project", 260.0, text, |ui| {
                for (id, name, _) in &projects {
                    ui.selectable_value(&mut m.project, Some(*id), name);
                }
            });
            if m.project != before
                && let Some((_, name, Some(root))) = projects.iter().find(|q| Some(q.0) == m.project)
            {
                m.picks = template::project_entries(Path::new(root)).into_iter().map(|(n, d)| (n.clone(), d, picked_by_default(&n))).collect();
                if m.name.is_empty() {
                    m.name = format!("Like {name}");
                }
            }
            ui.label(RichText::new("Template name").color(p.dim));
            ui.add(theme::field(&mut m.name, 240.0));
        });
        if !m.picks.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Copied as they are into files/ (edit them afterwards to add {{placeholders}}):").size(12.5).color(p.dim));
            ui.horizontal_wrapped(|ui| {
                for (name, is_dir, on) in &mut m.picks {
                    let label = if *is_dir { format!("{}  {name}/", icon::FOLDER) } else { name.clone() };
                    theme::checkbox(ui, on, label);
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = m.project.is_some() && !m.name.trim().is_empty() && m.picks.iter().any(|x| x.2);
            go = ui.add_enabled_ui(ready, |ui| theme::primary(ui, "Make template")).inner.clicked();
            cancel = theme::ghost(ui, "Cancel").clicked();
        });
    });
    if cancel {
        app.templates.make = None;
    }
    if go && let Some(m) = app.templates.make.take() {
        let root = m.project.and_then(|id| projects.iter().find(|q| q.0 == id)).and_then(|q| q.2.clone()).unwrap_or_default();
        let picks: Vec<String> = m.picks.into_iter().filter(|x| x.2).map(|x| x.0).collect();
        match template::from_project(dir, &m.name, Path::new(&root), &picks) {
            Ok(d) => app.message = Some(format!("Template made in {}; edit its template.toml to add commands", d.display())),
            Err(e) => app.message = Some(format!("No template made: {e}")),
        }
        app.templates.list = None;
    }
}

// --- New Project ----------------------------------------------------------------------------------------------

/// Open the New Project dialog, on a template's folder if given.
pub fn open_new(app: &mut App, dir: Option<PathBuf>) {
    if ensure_list(app).is_none() {
        return;
    }
    let templates: Vec<Template> =
        app.templates.list.as_ref().map(|l| l.1.iter().filter_map(|x| x.template.clone().ok()).collect()).unwrap_or_default();
    if templates.is_empty() {
        app.message = Some("No usable templates; see the Templates page".into());
        return;
    }
    let pick = dir.and_then(|d| templates.iter().position(|t| t.dir == d)).unwrap_or(0);
    let parent = app
        .store
        .setting(template::PARENT)
        .ok()
        .flatten()
        .or_else(|| app.store.scan_roots().ok().and_then(|r| r.first().map(|x| x.0.clone())))
        .unwrap_or_default();
    let group = group_of(app, &parent);
    app.new_project = Some(NewProject { templates, pick, values: HashMap::new(), parent, group, stage: Stage::Form(None), log: Vec::new() });
}

/// The group whose folder is `parent`.
fn group_of(app: &App, parent: &str) -> Option<i64> {
    let parent = pa_core::norm(Path::new(parent));
    app.groups.iter().find(|g| g.path.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&parent))).map(|g| g.id)
}

enum Step {
    Preview,
    Back,
    Create,
    Close,
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    poll(app, ctx);
    let p = pal(ctx);
    let groups = app.groups.clone();
    let Some(np) = app.new_project.as_mut() else { return };
    let busy = matches!(np.stage, Stage::Running(..));
    let mut step = None;
    // A dialog over the dimmed app (theme::dialog); Escape or the close button leave it unless a run is going.
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let (_, closed) = theme::dialog(ctx, "new_project", "New project", 776.0, |ui| {
        match &np.stage {
            Stage::Form(err) => {
                let err = err.clone();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(220.0);
                        ui.label(caps("Template", p.faint));
                        for (i, t) in np.templates.iter().enumerate() {
                            if theme::button(ui, &t.name, egui::FontId::proportional(13.5), 0.0, theme::Kind::Nav(i == np.pick), vec2(220.0, 32.0)).clicked() {
                                np.pick = i;
                            }
                        }
                    });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        let t = &np.templates[np.pick];
                        ui.label(RichText::new(&t.name).font(theme::semibold(16.0)));
                        if !t.description.is_empty() {
                            ui.label(RichText::new(&t.description).color(p.dim));
                        }
                        ui.add_space(8.0);
                        egui::Grid::new("np_prompts").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                            for q in t.questions() {
                                let label = if q.label.is_empty() { q.key.clone() } else { q.label.clone() };
                                ui.label(RichText::new(label).color(p.dim));
                                let v = np.values.entry(q.key.clone()).or_default();
                                ui.add(theme::field(v, 360.0).hint_text(q.default.clone()));
                                ui.end_row();
                            }
                            ui.label(RichText::new("Create in").color(p.dim));
                            ui.horizontal(|ui| {
                                ui.add(theme::field(&mut np.parent, 270.0).hint_text("parent folder"));
                                if theme::secondary(ui, "Browse…").clicked()
                                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                                {
                                    np.parent = pa_core::norm(&d);
                                    if let Some(g) = groups.iter().find(|g| g.path.as_deref() == Some(np.parent.as_str())) {
                                        np.group = Some(g.id);
                                    }
                                }
                            });
                            ui.end_row();
                            ui.label(RichText::new("Group").color(p.dim));
                            crate::widgets::group_combo(ui, "np_group", &groups, &mut np.group);
                            ui.end_row();
                        });
                        if let Some(e) = err {
                            ui.add_space(6.0);
                            ui.label(RichText::new(format!("{}  {e}", icon::WARNING)).color(p.red));
                        }
                    });
                });
                ui.add_space(16.0);
                footer(ui, |ui| {
                    if theme::primary(ui, "Preview…").clicked() {
                        step = Some(Step::Preview);
                    }
                    if theme::secondary(ui, "Cancel").clicked() {
                        step = Some(Step::Close);
                    }
                });
            }
            Stage::Preview(plan) => {
                ui.label(RichText::new(format!("Creates {}", plan.project_dir.display())).font(theme::semibold(15.0)));
                ui.add_space(8.0);
                ui.label(caps("Steps", p.faint));
                for (i, a) in plan.actions.iter().enumerate() {
                    match a {
                        Action::Run { command, cwd } => {
                            ui.label(RichText::new(format!("{}  $ {command}", i + 1)).monospace());
                            ui.label(RichText::new(format!("     in {}", cwd.display())).monospace().size(12.0).color(p.faint));
                        }
                        Action::Overlay { from, files } => {
                            ui.label(RichText::new(format!("{}  copy {} files from {}", i + 1, files.len(), from.display())).monospace());
                            let shown: Vec<&str> = files.iter().take(12).map(String::as_str).collect();
                            let more = if files.len() > 12 { format!(" … {} more", files.len() - 12) } else { String::new() };
                            ui.label(RichText::new(format!("     {}{more}", shown.join(", "))).monospace().size(12.0).color(p.faint));
                        }
                    }
                }
                ui.add_space(8.0);
                ui.label(caps("Placeholders", p.faint));
                egui::Grid::new("np_vars").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (k, v) in &plan.vars {
                        ui.label(RichText::new(format!("{{{{{k}}}}}")).monospace().color(p.dim));
                        ui.add(egui::Label::new(v).truncate());
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("The commands run on this computer and their output shows here. If a step fails, the folder is left as it is. Then the folder is added to the app.").size(12.5).color(p.dim));
                ui.add_space(8.0);
                footer(ui, |ui| {
                    if theme::primary(ui, format!("{}  Create", icon::PLAY)).clicked() {
                        step = Some(Step::Create);
                    }
                    if theme::secondary(ui, "Back").clicked() {
                        step = Some(Step::Back);
                    }
                });
            }
            Stage::Running(plan, _) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Creating {}…", plan.project_dir.display()));
                });
                log_view(ui, &np.log, p);
            }
            Stage::Failed(e) => {
                ui.label(RichText::new(format!("{}  {e}", icon::WARNING)).color(p.red));
                log_view(ui, &np.log, p);
                footer(ui, |ui| {
                    if theme::secondary(ui, "Close").clicked() {
                        step = Some(Step::Close);
                    }
                    if theme::secondary(ui, "Back").clicked() {
                        step = Some(Step::Back);
                    }
                });
            }
        }
    });
    if ((closed || escape) && !busy) || matches!(step, Some(Step::Close)) {
        app.new_project = None;
        return;
    }
    let Some(np) = app.new_project.as_mut() else { return };
    match step {
        Some(Step::Preview) => {
            np.stage = match template::plan(&np.templates[np.pick], &np.values, Path::new(np.parent.trim())) {
                Ok(plan) => Stage::Preview(plan),
                Err(e) => Stage::Form(Some(e)),
            }
        }
        Some(Step::Back) => np.stage = Stage::Form(None),
        Some(Step::Create) => {
            let Stage::Preview(plan) = std::mem::replace(&mut np.stage, Stage::Form(None)) else { return };
            let parent = np.parent.trim().to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            let (job, ctx) = (plan.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = template::run(&job, &mut |l| {
                    let _ = tx.send(Msg::Line(l));
                    ctx.request_repaint();
                });
                let _ = tx.send(Msg::Done(r));
                ctx.request_repaint();
            });
            np.log.clear();
            np.stage = Stage::Running(plan, rx);
            let r = app.store.set_setting(template::PARENT, &parent);
            app.ok(r);
        }
        _ => {}
    }
}

/// A dialog's button row, right-aligned; `add` adds the main action first (it ends up rightmost).
fn footer(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
}

fn log_view(ui: &mut egui::Ui, log: &[String], p: theme::Palette) {
    egui::Frame::new().fill(p.bg).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(380.0).stick_to_bottom(true).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(log.join("\n")).monospace().size(12.0)).extend());
        });
    });
}

/// Take the running steps' output; when they finish, track the project and open its tab, or show the failure.
fn poll(app: &mut App, ctx: &egui::Context) {
    let Some(np) = app.new_project.as_mut() else { return };
    let Stage::Running(_, rx) = &np.stage else { return };
    let mut done = None;
    while let Ok(m) = rx.try_recv() {
        match m {
            Msg::Line(l) => np.log.push(l),
            Msg::Done(r) => done = Some(r),
        }
    }
    let Some(result) = done else { return };
    let Stage::Running(plan, _) = std::mem::replace(&mut np.stage, Stage::Form(None)) else { return };
    if let Err(e) = result {
        np.stage = Stage::Failed(e);
        return;
    }
    let t = np.templates[np.pick].clone();
    let group = np.group.and_then(|id| app.groups.iter().find(|g| g.id == id)).map(|g| g.name.clone());
    match template::finish(&app.store, &t, &plan, group.as_deref()) {
        Ok(id) => {
            app.new_project = None;
            app.reload();
            app.open.push(Tab::Project(id));
            app.refresh_project(ctx, id);
            app.message = Some(format!("Created {}", plan.name()));
        }
        Err(e) => {
            if let Some(np) = app.new_project.as_mut() {
                np.stage = Stage::Failed(format!("Created {}, but it was not added to the app: {e}", plan.project_dir.display()));
            }
        }
    }
}
