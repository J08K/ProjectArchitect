//! The Dependencies section (SPEC § 7.10, D-19): what this project uses (another tracked project, at a version read
//! from a file with a pattern, or fixed) and what uses it, shown as "uses spec 10.1 — latest spec-v11" with a warning
//! when behind. Versions are read on a background thread (`pa_core::deps`); [`recheck_all`] also feeds the
//! overview's "deps behind" chip.

use crate::App;
use crate::theme::{self, Palette, caps, card, chip};
use egui::{Align, Layout, RichText};
use egui_phosphor::regular as icon;
use pa_core::Project;
use pa_core::deps::{self, Check, Dependency, Standing};
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    /// Every dependency in the store; `None` = read it again.
    all: Option<Vec<Dependency>>,
    /// The last check of each dependency, by id.
    checks: HashMap<i64, Check>,
    checking: bool,
    /// Projects with a dependency behind its latest version, and which (the overview's chip).
    pub behind: HashMap<i64, Vec<String>>,
    form: Option<Form>,
}

impl State {
    /// Read the dependencies again (after the MCP server changed them).
    pub fn forget(&mut self) {
        self.all = None;
    }
}

/// The add / edit form.
struct Form {
    d: Dependency,
    fixed: bool,
    by_tag: bool,
    file: String,
    pattern: String,
    value: String,
    glob: String,
}

impl Form {
    fn new(project_id: i64) -> Form {
        Form { d: Dependency { project_id, ..Default::default() }, fixed: false, by_tag: false, file: String::new(), pattern: String::new(), value: String::new(), glob: String::new() }
    }

    fn edit(d: &Dependency) -> Form {
        let s = |o: &Option<String>| o.clone().unwrap_or_default();
        Form { d: d.clone(), fixed: d.used_fixed.is_some(), by_tag: d.latest_tag.is_some(), file: s(&d.used_file), pattern: s(&d.used_pattern), value: s(&d.used_fixed), glob: s(&d.latest_tag) }
    }

    fn dependency(&self) -> Dependency {
        let some = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        Dependency {
            used_fixed: if self.fixed { some(&self.value) } else { None },
            used_file: if self.fixed { None } else { some(&self.file) },
            used_pattern: if self.fixed { None } else { (!self.pattern.is_empty()).then(|| self.pattern.clone()) },
            latest_tag: if self.by_tag { some(&self.glob) } else { None },
            ..self.d.clone()
        }
    }
}

fn name(app: &App, id: i64) -> String {
    app.projects.iter().find(|q| q.id == id).map_or_else(|| format!("#{id}"), |q| q.name.clone())
}

/// "uses spec 10.1 — latest spec-v11" (or what could not be read).
fn describe(app: &App, d: &Dependency, c: Option<&Check>) -> String {
    let v = |r: Option<&Result<String, String>>| match r {
        Some(Ok(v)) => v.clone(),
        Some(Err(_)) => "?".into(),
        None => "…".into(),
    };
    format!("uses {} {} — latest {}", name(app, d.depends_on), v(c.map(|c| &c.used)), v(c.map(|c| &c.latest)))
}

/// Check every dependency on a background thread; updates the section and the overview's chips.
pub fn recheck_all(app: &mut App, ctx: &egui::Context) {
    if app.deps.all.is_none() {
        let r = app.store.all_dependencies();
        app.deps.all = Some(app.ok(r).unwrap_or_default());
    }
    let list = app.deps.all.clone().unwrap_or_default();
    if list.is_empty() {
        app.deps.behind.clear();
        return;
    }
    if app.deps.checking {
        return;
    }
    app.deps.checking = true;
    let projects = app.projects.clone();
    app.background(ctx, move || {
        let checks: Vec<(Dependency, Option<Check>)> = list.into_iter().map(|d| {
            let c = deps::check_projects(&projects, &d);
            (d, c)
        }).collect();
        Box::new(move |app: &mut App| {
            app.deps.checking = false;
            app.deps.behind.clear();
            for (d, c) in checks {
                let Some(c) = c else { continue };
                if c.standing == Standing::Behind {
                    let text = describe(app, &d, Some(&c));
                    app.deps.behind.entry(d.project_id).or_default().push(text);
                }
                app.deps.checks.insert(d.id, c);
            }
        })
    });
}

fn standing_chip(ui: &mut egui::Ui, c: Option<&Check>, p: Palette) {
    let Some(c) = c else {
        ui.spinner();
        return;
    };
    let (text, color, tip) = match c.standing {
        Standing::Current => ("current", p.green, String::new()),
        Standing::Behind => ("behind", p.orange, "A newer version exists".to_string()),
        Standing::Ahead => ("ahead", p.blue, "Uses a version newer than the latest one found".to_string()),
        Standing::Differs => ("differs", p.purple, "The versions are not numbers to compare".to_string()),
        Standing::Unknown => {
            let errs: Vec<&str> = [&c.used, &c.latest].iter().filter_map(|r| r.as_ref().err()).map(String::as_str).collect();
            ("unknown", p.red, errs.join("\n"))
        }
    };
    let r = chip(ui, text, color);
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
}

enum Act {
    Add,
    Edit(Dependency),
    Delete(i64),
    Save,
    Cancel,
    Recheck,
    Open(i64),
}

pub fn show(app: &mut App, ui: &mut egui::Ui, q: &Project, p: Palette) {
    let id = q.id;
    if app.deps.all.is_none() {
        recheck_all(app, ui.ctx());
    }
    let all = app.deps.all.clone().unwrap_or_default();
    let uses: Vec<&Dependency> = all.iter().filter(|d| d.project_id == id).collect();
    let used_by: Vec<&Dependency> = all.iter().filter(|d| d.depends_on == id).collect();
    let mut acts: Vec<Act> = Vec::new();

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Other tracked projects this one uses, at which version, and whether a newer one exists.").color(p.dim));
        theme::right(ui, "deps_ui.rs:159", egui::Align::Center, |ui| {
            if app.deps.checking {
                ui.spinner();
            } else if theme::secondary(ui, format!("{}  Check again", icon::ARROWS_CLOCKWISE)).clicked() {
                acts.push(Act::Recheck);
            }
        });
    });
    ui.add_space(8.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps(format!("Uses   {}", uses.len()), p.faint));
        ui.add_space(4.0);
        if uses.is_empty() && app.deps.form.is_none() {
            ui.label(RichText::new("No dependencies yet.").color(p.dim));
        }
        for d in &uses {
            if app.deps.form.as_ref().is_some_and(|f| f.d.id == d.id) {
                form(app, ui, q, p, &mut acts);
                continue;
            }
            let c = app.deps.checks.get(&d.id);
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                standing_chip(ui, c, p);
                ui.label(describe(app, d, c));
                ui.label(RichText::new(how(d)).size(12.5).color(p.faint));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(ui, icon::TRASH, "Remove the dependency").clicked() {
                        acts.push(Act::Delete(d.id));
                    }
                    if theme::icon_button(ui, icon::PENCIL_SIMPLE, "Edit").clicked() {
                        acts.push(Act::Edit((*d).clone()));
                    }
                    if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, "Open that project").clicked() {
                        acts.push(Act::Open(d.depends_on));
                    }
                });
            });
        }
        if app.deps.form.as_ref().is_some_and(|f| f.d.id == 0) {
            form(app, ui, q, p, &mut acts);
        } else if theme::secondary(ui, format!("{}  Add dependency", icon::PLUS)).clicked() {
            acts.push(Act::Add);
        }
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps(format!("Used by   {}", used_by.len()), p.faint));
        ui.add_space(4.0);
        if used_by.is_empty() {
            ui.label(RichText::new("No tracked project says it uses this one.").color(p.dim));
        }
        for d in &used_by {
            let c = app.deps.checks.get(&d.id);
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                standing_chip(ui, c, p);
                ui.label(format!("{} {}", name(app, d.project_id), describe(app, d, c)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, "Open that project").clicked() {
                        acts.push(Act::Open(d.project_id));
                    }
                });
            });
        }
    });

    for a in acts {
        match a {
            Act::Add => app.deps.form = Some(Form::new(id)),
            Act::Edit(d) => app.deps.form = Some(Form::edit(&d)),
            Act::Cancel => app.deps.form = None,
            Act::Delete(dep) => {
                let r = app.store.delete_dependency(dep);
                app.ok(r);
                changed(app, ui.ctx());
            }
            Act::Save => {
                let Some(d) = app.deps.form.as_ref().map(Form::dependency) else { continue };
                match deps::validate(&d) {
                    Err(e) => app.message = Some(format!("Not saved: {e}")),
                    Ok(()) => {
                        let r = app.store.set_dependency(&d);
                        if app.ok(r).is_some() {
                            app.deps.form = None;
                            changed(app, ui.ctx());
                        }
                    }
                }
            }
            Act::Recheck => {
                app.deps.all = None;
                recheck_all(app, ui.ctx());
            }
            Act::Open(other) => app.open.push(crate::Tab::Project(other)),
        }
    }
}

fn changed(app: &mut App, ctx: &egui::Context) {
    app.deps.all = None;
    recheck_all(app, ctx);
}

/// Where the versions come from, in a few words.
fn how(d: &Dependency) -> String {
    let used = match (&d.used_fixed, &d.used_file) {
        (Some(v), _) => format!("fixed {v}"),
        (None, Some(f)) => format!("from {f}"),
        _ => "no used version".into(),
    };
    let latest = d.latest_tag.as_deref().map_or_else(|| "its version".to_string(), |g| format!("tags {g}"));
    format!("{used}  ·  latest: {latest}")
}

fn form(app: &mut App, ui: &mut egui::Ui, q: &Project, p: Palette, acts: &mut Vec<Act>) {
    let others: Vec<(i64, String)> = app.projects.iter().filter(|o| o.id != q.id).map(|o| (o.id, o.name.clone())).collect();
    let Some(f) = app.deps.form.as_mut() else { return };
    egui::Frame::new().fill(p.raised).corner_radius(8).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new(("dep_form", f.d.id)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Uses").color(p.dim));
            let text = others.iter().find(|o| o.0 == f.d.depends_on).map_or("Choose a project…", |o| o.1.as_str()).to_string();
            theme::select(ui, "dep_project", 300.0, text, |ui| {
                for (oid, oname) in &others {
                    ui.selectable_value(&mut f.d.depends_on, *oid, oname);
                }
            });
            ui.end_row();
            ui.label(RichText::new("Used version").color(p.dim));
            ui.horizontal(|ui| {
                ui.radio_value(&mut f.fixed, false, "From a file in this project");
                ui.radio_value(&mut f.fixed, true, "Fixed");
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                if f.fixed {
                    ui.add(theme::field(&mut f.value, 200.0).hint_text("10.1"));
                } else {
                    ui.add(theme::field(&mut f.file, 220.0).hint_text("SPEC_VERSION"));
                    ui.add(theme::field(&mut f.pattern, 260.0).hint_text("spec-v([0-9.]+)").font(egui::TextStyle::Monospace))
                        .on_hover_text("A regular expression; its first group (else the whole match) is the version");
                }
            });
            ui.end_row();
            ui.label(RichText::new("Latest version").color(p.dim));
            ui.horizontal(|ui| {
                ui.radio_value(&mut f.by_tag, false, "That project's version");
                ui.radio_value(&mut f.by_tag, true, "Its newest tag matching");
                if f.by_tag {
                    ui.add(theme::field(&mut f.glob, 160.0).hint_text("spec-v*").font(egui::TextStyle::Monospace));
                }
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if theme::primary(ui, "Save").clicked() {
                acts.push(Act::Save);
            }
            if theme::ghost(ui, "Cancel").clicked() {
                acts.push(Act::Cancel);
            }
        });
    });
}
