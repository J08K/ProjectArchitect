//! Import (SPEC § 4): scan the roots in the background, then confirm which candidates become projects, which are
//! checkouts of a project (same remote or root commit, D-12), and which folders become groups.

use crate::theme::{self, caps, card, chip, pal};
use crate::widgets::purpose_combo;
use egui::RichText;
use crate::{App, Tab};
use pa_core::ops;
use pa_core::scan::{self, Candidate, GroupSuggestion, ScanResult};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    New,
    /// Another checkout of a tracked project.
    Project(i64),
    /// Another checkout of an earlier row (created together).
    Row(usize),
}

struct Row {
    cand: Candidate,
    selected: bool,
    purpose: Option<i64>,
    target: Target,
}

struct GroupRow {
    sugg: GroupSuggestion,
    selected: bool,
}

#[derive(Default)]
pub struct State {
    rx: Option<Receiver<ScanResult>>,
    rows: Vec<Row>,
    groups: Vec<GroupRow>,
    scanned: bool,
    bulk_purpose: Option<i64>,
}

impl State {
    fn add_candidates(&mut self, app_projects: &[pa_core::Project], cands: Vec<Candidate>) {
        for cand in cands {
            if self.rows.iter().any(|r| r.cand.path == cand.path) {
                continue;
            }
            let id = (cand.remote.clone(), cand.root_commit.clone());
            let tracked = app_projects.iter().find(|p| {
                p.checkouts.iter().filter_map(|c| c.info.as_ref()?.git.as_ref()).any(|g| ops::same_identity(&id, &(g.remote.clone(), g.root_commit.clone())))
            });
            let target = match tracked {
                Some(p) => Target::Project(p.id),
                None => match self.rows.iter().position(|r| r.target == Target::New && ops::same_identity(&id, &(r.cand.remote.clone(), r.cand.root_commit.clone()))) {
                    Some(i) => Target::Row(i),
                    None => Target::New,
                },
            };
            self.rows.push(Row { cand, selected: true, purpose: None, target });
        }
    }
}

/// Take a finished scan's result.
pub fn poll(app: &mut App) {
    let Some(rx) = &app.import.rx else { return };
    let Ok(result) = rx.try_recv() else { return };
    app.import.rx = None;
    app.import.scanned = true;
    let known_groups: HashSet<String> = app.groups.iter().filter_map(|g| g.path.clone()).collect();
    app.import.groups = result
        .groups
        .into_iter()
        .filter(|g| !known_groups.contains(&g.path))
        .map(|g| GroupRow { selected: g.curated, sugg: g })
        .collect();
    let projects = app.projects.clone();
    app.import.add_candidates(&projects, result.candidates);
}

fn start_scan(app: &mut App, ctx: &egui::Context) {
    let r = app.store.scan_roots();
    let Some(roots) = app.ok(r) else { return };
    let r = app.store.ignored();
    let Some(ignored) = app.ok(r) else { return };
    let tracked: HashSet<String> = app.projects.iter().flat_map(|p| &p.checkouts).map(|c| c.path.clone()).collect();
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(scan::scan(&roots, &ignored, &tracked));
        ctx.request_repaint();
    });
    app.import.rx = Some(rx);
    app.import.rows.clear();
    app.import.groups.clear();
}

/// Add one folder chosen by hand (any folder, project-like or not) to the list.
pub fn add_folder(app: &mut App, dir: &std::path::Path) {
    let path = pa_core::norm(dir);
    if app.projects.iter().flat_map(|p| &p.checkouts).any(|c| c.path == path) {
        app.message = Some(format!("{path} is already tracked"));
        return;
    }
    let git = dir.join(".git").exists();
    let (remote, root_commit) = if git { pa_core::info::identity(dir) } else { (None, None) };
    let cand = Candidate { name: pa_core::folder_name(&path), path, git, remote, root_commit };
    let projects = app.projects.clone();
    app.import.add_candidates(&projects, vec![cand]);
    app.import.scanned = true;
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let roots = app.store.scan_roots().unwrap_or_default();
    ui.horizontal(|ui| {
        let scanning = app.import.rx.is_some();
        if !scanning && !roots.is_empty() && theme::primary(ui, format!("{}  Scan roots", egui_phosphor::regular::MAGNIFYING_GLASS)).clicked() {
            start_scan(app, &ctx);
        }
        if scanning {
            ui.spinner();
            ui.label("Scanning…");
        }
        if theme::secondary(ui, "Add folder…").on_hover_text("Any folder, anywhere").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            add_folder(app, &dir);
        }
        if theme::secondary(ui, "Add scan root…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            let r = app.store.set_scan_root(&pa_core::norm(&dir), 3);
            app.ok(r);
        }
        if roots.is_empty() {
            ui.label(RichText::new("No scan roots yet: add one, e.g. your Code folder.").color(pal(ui.ctx()).dim));
        } else {
            let list: Vec<&str> = roots.iter().map(|r| r.0.as_str()).collect();
            ui.label(RichText::new(format!("Roots: {}", list.join("; "))).color(pal(ui.ctx()).dim)).on_hover_text("Edit them in Settings");
        }
    });
    ui.add_space(12.0);
    if app.import.scanned && app.import.rows.is_empty() && app.import.groups.is_empty() {
        ui.label("Nothing new found. Ignored folders are listed in Settings.");
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let p = pal(ui.ctx());
        if !app.import.groups.is_empty() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(caps("Groups", p.dim));
                ui.label(RichText::new("Folders that hold several projects. Pre-selected when they have a README.md or CLAUDE.md.").color(p.dim));
                ui.add_space(4.0);
                for g in &mut app.import.groups {
                    ui.horizontal(|ui| {
                        theme::checkbox(ui, &mut g.selected, RichText::new(&g.sugg.name).font(theme::semibold(14.0)));
                        ui.label(RichText::new(format!("{} projects", g.sugg.members.len())).color(p.dim));
                        ui.label(RichText::new(&g.sugg.path).monospace().size(12.0).color(p.faint));
                    });
                }
            });
            ui.add_space(12.0);
        }
        if !app.import.rows.is_empty() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(caps("Projects", p.dim));
                rows(app, ui);
            });
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let n = app.import.rows.iter().filter(|r| r.selected).count();
        if (n > 0 || app.import.groups.iter().any(|g| g.selected)) && theme::primary(ui, format!("Import {n} selected")).clicked() {
            import_selected(app, &ctx);
        }
        if n > 0 && theme::secondary(ui, format!("Ignore {n} selected")).on_hover_text("Never suggest these folders again").clicked() {
            for r in app.import.rows.iter().filter(|r| r.selected) {
                let res = app.store.ignore(&r.cand.path);
                if let Err(e) = res {
                    app.message = Some(format!("Error: {e}"));
                }
            }
            remove_rows(&mut app.import, |r| r.selected);
        }
        ui.separator();
        ui.label("Purpose for selected:");
        let purposes = app.purposes.clone();
        if purpose_combo(ui, "bulk_purpose", &purposes, &mut app.import.bulk_purpose) {
            let v = app.import.bulk_purpose;
            app.import.rows.iter_mut().filter(|r| r.selected).for_each(|r| r.purpose = v);
        }
    });
}

fn rows(app: &mut App, ui: &mut egui::Ui) {
    let (projects, purposes) = (app.projects.clone(), app.purposes.clone());
    let s = &mut app.import;
    let names: Vec<String> = s.rows.iter().map(|r| r.cand.name.clone()).collect();
    ui.horizontal(|ui| {
        if theme::ghost(ui, "Select all").clicked() {
            s.rows.iter_mut().for_each(|r| r.selected = true);
        }
        if theme::ghost(ui, "Select none").clicked() {
            s.rows.iter_mut().for_each(|r| r.selected = false);
        }
    });
    egui::Grid::new("import_rows").striped(true).num_columns(5).spacing([12.0, 4.0]).show(ui, |ui| {
        for h in ["", "Name", "Folder", "Becomes", "Purpose"] {
            ui.label(caps(h, pal(ui.ctx()).faint));
        }
        ui.end_row();
        for (i, r) in s.rows.iter_mut().enumerate() {
            theme::checkbox(ui, &mut r.selected, "");
            ui.add(theme::field(&mut r.cand.name, 200.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::overview::short_path(&r.cand.path)).monospace().size(12.5));
                if !r.cand.git {
                    chip(ui, "no git", pal(ui.ctx()).faint);
                }
            });
            let becomes = match r.target {
                Target::New => "new project".to_string(),
                Target::Project(id) => format!("checkout of {}", projects.iter().find(|p| p.id == id).map_or("?", |p| p.name.as_str())),
                Target::Row(j) => format!("checkout of {}", names.get(j).map_or("?", String::as_str)),
            };
            ui.horizontal(|ui| {
                ui.label(becomes);
                if r.target != Target::New && theme::ghost(ui, "separate").on_hover_text("Import as its own project instead").clicked() {
                    r.target = Target::New;
                }
            });
            purpose_combo(ui, ("import_purpose", i), &purposes, &mut r.purpose);
            ui.end_row();
        }
    });
}

/// Remove rows matching `f`, keeping `Target::Row` indices pointing at the same rows (or `New` when theirs went).
fn remove_rows(s: &mut State, f: impl Fn(&Row) -> bool) {
    let keep: Vec<bool> = s.rows.iter().map(|r| !f(r)).collect();
    let mut new_index = Vec::with_capacity(keep.len());
    let mut n = 0;
    for &k in &keep {
        new_index.push(k.then_some(n));
        n += k as usize;
    }
    let mut i = 0;
    s.rows.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    for r in &mut s.rows {
        if let Target::Row(j) = r.target {
            r.target = new_index[j].map_or(Target::New, Target::Row);
        }
    }
}

fn import_selected(app: &mut App, ctx: &egui::Context) {
    // Groups first, so new projects inside them get the group.
    let mut group_of: Vec<(String, i64)> = app.groups.iter().filter_map(|g| Some((g.path.clone()?, g.id))).collect();
    let mut new_groups = Vec::new();
    for g in app.import.groups.iter().filter(|g| g.selected) {
        match app.store.add_group(&g.sugg.name, Some(&g.sugg.path)) {
            Ok(id) => {
                group_of.push((g.sugg.path.clone(), id));
                new_groups.push((id, g.sugg.members.clone()));
            }
            Err(e) => app.message = Some(format!("Error: {e}")),
        }
    }
    let parent_group = |path: &str| {
        let parent = std::path::Path::new(path).parent().map(pa_core::norm)?;
        group_of.iter().find(|(p, _)| *p == parent).map(|g| g.1)
    };
    let mut created: Vec<Option<i64>> = vec![None; app.import.rows.len()];
    let mut added = Vec::new();
    for (i, r) in app.import.rows.iter().enumerate().filter(|(_, r)| r.selected) {
        let into = match r.target {
            Target::Project(id) => Some(id),
            Target::Row(j) => created[j],
            Target::New => None,
        };
        let res = match into {
            Some(pid) => app.store.add_checkout(pid, &r.cand.path).map(|_| pid),
            None => app.store.add_project(&r.cand.name, parent_group(&r.cand.path), r.purpose, std::slice::from_ref(&r.cand.path)),
        };
        match res {
            Ok(pid) => {
                created[i] = Some(pid);
                added.push(r.cand.path.clone());
            }
            Err(e) => app.message = Some(format!("Error importing {}: {e}", r.cand.path)),
        }
    }
    app.reload();
    // Already tracked projects inside a new group join it when they have none.
    for (gid, members) in new_groups {
        let join: Vec<_> = app
            .projects
            .iter()
            .filter(|p| p.group_id.is_none() && p.checkouts.iter().any(|c| members.contains(&c.path)))
            .cloned()
            .collect();
        for mut p in join {
            p.group_id = Some(gid);
            app.save_project(p);
        }
    }
    let jobs: Vec<(i64, String)> =
        app.projects.iter().flat_map(|p| &p.checkouts).filter(|c| added.contains(&c.path)).map(|c| (c.id, c.path.clone())).collect();
    app.refresh(ctx, jobs);
    remove_rows(&mut app.import, |r| r.selected);
    app.import.groups.retain(|g| !g.selected);
    if !added.is_empty() {
        app.message = Some(format!("Imported {} folders", added.len()));
        app.open.push(Tab::Overview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(path: &str, remote: Option<&str>, root: Option<&str>) -> Candidate {
        Candidate { path: path.into(), name: path.into(), git: true, remote: remote.map(Into::into), root_commit: root.map(Into::into) }
    }

    #[test]
    fn clones_become_checkouts() {
        let mut s = State::default();
        s.add_candidates(
            &[],
            vec![
                cand("a", Some("https://github.com/o/r.git"), Some("1")),
                cand("b", Some("git@github.com:O/r.git"), None),
                cand("c", None, Some("1")),
                cand("d", None, None),
            ],
        );
        let t: Vec<Target> = s.rows.iter().map(|r| r.target).collect();
        assert_eq!(t, [Target::New, Target::Row(0), Target::Row(0), Target::New]);
        s.rows[0].selected = true;
        s.rows[3].selected = true;
        s.rows[1].selected = false;
        s.rows[2].selected = false;
        remove_rows(&mut s, |r| r.selected);
        let t: Vec<Target> = s.rows.iter().map(|r| r.target).collect();
        assert_eq!(t, [Target::New, Target::New], "rows pointing at a removed row become new projects");
    }
}
