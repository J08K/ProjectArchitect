//! The Git and GitHub sections of a project tab. Git (SPEC § 7.5): the selected checkout's branch, changed files,
//! branches and worktrees (with how each is used, and bulk removal after a confirmation, D-40), the last 20 commits
//! and the tags, with fetch, pull, push, switch branch and new worktree;
//! each command's output is shown. GitHub (§ 7.6, D-7): visibility, stars, open issues and PRs, the latest release
//! and CI of the default branch through `gh`, "Create GitHub repo" (always confirmed: it publishes), and the
//! per-project "captures become issues" switch. Everything runs on background threads (`App::background`).

use crate::theme::{self, Palette, caps, card, chip, relative, tag};
use crate::{App, Job};
use egui::{Align, Layout, RichText, vec2};
use egui_phosphor::regular as icon;
use pa_core::git::{self, Output, Snapshot};
use pa_core::github::{self, RepoInfo};
use pa_core::Project;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct State {
    panels: HashMap<PathBuf, Panel>,
    /// GitHub info per project: `None` while loading.
    github: HashMap<i64, Option<Result<RepoInfo, String>>>,
    create: Option<Create>,
}

/// The Git section of one checkout.
#[derive(Default)]
struct Panel {
    snap: Option<Result<Snapshot, String>>,
    loading: bool,
    /// A command is running.
    busy: bool,
    /// Commands run here, oldest first.
    log: Vec<Output>,
    wt_branch: String,
    wt_new: bool,
    wt_path: String,
    /// The folder was typed: stop deriving it from the branch.
    wt_path_typed: bool,
    /// Worktree paths ticked for removal.
    wt_selected: HashSet<String>,
    /// The "Remove worktrees" confirmation is open, with its two options.
    wt_confirm: bool,
    wt_force: bool,
    wt_branches: bool,
}

/// Claude Code wrote in a worktree this recently: someone is probably working there.
const ACTIVE_SECS: i64 = 600;

fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.trim_end_matches(['\\', '/']))
}

fn folder_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Safe to clean up: the folder is gone, or it has no changes, nothing unmerged and no recent Claude activity.
fn idle(w: &git::Worktree) -> bool {
    w.missing || (w.changes == Some(0) && w.unmerged == Some(0) && w.claude.is_none_or(|t| pa_core::now_secs() - t >= ACTIVE_SECS))
}

/// How a worktree is used: changes, unmerged commits, Claude activity, last commit.
fn use_chips(ui: &mut egui::Ui, w: &git::Worktree, p: Palette) {
    if w.locked {
        chip(ui, "locked", p.orange).on_hover_text("git worktree lock: git will not remove it");
    }
    if w.missing {
        chip(ui, "folder missing", p.red).on_hover_text("Removing it only prunes git's record");
    }
    match w.changes {
        Some(0) | None => {}
        Some(n) => {
            chip(ui, format!("{n} changes"), p.orange).on_hover_text("Uncommitted or untracked files: lost if removed with force");
        }
    }
    match w.unmerged {
        Some(0) => {
            chip(ui, "merged", p.green).on_hover_text("Every commit is in the default branch");
        }
        Some(n) => {
            chip(ui, format!("{n} unmerged"), p.purple).on_hover_text("Commits the default branch does not have; they stay on the branch");
        }
        None => {}
    }
    if let Some(t) = w.claude {
        let s = (pa_core::now_secs() - t).max(0);
        let text = match s {
            s if s < ACTIVE_SECS => "Claude active".to_string(),
            s if s < 3600 => format!("Claude {} min ago", s / 60),
            s if s < 86_400 => format!("Claude {} h ago", s / 3600),
            s => format!("Claude {} d ago", s / 86_400),
        };
        let color = if s < ACTIVE_SECS { p.green } else { p.faint };
        chip(ui, text, color).on_hover_text("When a Claude Code session in this folder last wrote its transcript");
    }
    if let Some(d) = &w.last_commit {
        ui.label(RichText::new(format!("last commit {}", relative(d))).size(12.0).color(p.faint));
    }
}

/// The "Create GitHub repo" dialog.
struct Create {
    project: i64,
    root: PathBuf,
    owner: String,
    name: String,
    private: bool,
    push: bool,
    running: bool,
}

// --- Git ------------------------------------------------------------------------------------------------------

fn load(app: &mut App, ctx: &egui::Context, root: &Path) {
    let panel = app.git.panels.entry(root.to_path_buf()).or_default();
    if panel.loading {
        return;
    }
    panel.loading = true;
    let root = root.to_path_buf();
    let home = pa_core::claude::home(&app.store).ok();
    app.background(ctx, move || {
        let snap = git::snapshot(&root)
            .map(|mut s| {
                if let Some(h) = &home {
                    for w in &mut s.worktrees {
                        w.claude = pa_core::claude::last_session(h, &w.path);
                    }
                }
                s
            })
            .map_err(|e| e.0);
        Box::new(move |app: &mut App| {
            let panel = app.git.panels.entry(root).or_default();
            (panel.snap, panel.loading) = (Some(snap), false);
        })
    });
}

/// Run a git action in the background; its output goes to the panel's log, then the panel and the project reload.
fn action(app: &mut App, ctx: &egui::Context, project: i64, root: &Path, work: impl FnOnce(&Path) -> (Output, Option<String>) + Send + 'static) {
    let panel = app.git.panels.entry(root.to_path_buf()).or_default();
    if panel.busy {
        return;
    }
    panel.busy = true;
    let root = root.to_path_buf();
    let ctx2 = ctx.clone();
    app.background(ctx, move || {
        // A new worktree comes back as a folder to track as a checkout.
        let (out, new_checkout) = work(&root);
        Box::new(move |app: &mut App| {
            if let Some(path) = new_checkout.filter(|_| out.ok) {
                let path = pa_core::norm(Path::new(&path));
                let r = app.store.add_checkout(project, &path);
                if app.ok(r).is_some() {
                    app.message = Some(format!("New worktree {path} is now a checkout of this project"));
                    app.reload();
                }
            }
            let panel = app.git.panels.entry(root.clone()).or_default();
            panel.busy = false;
            panel.log.push(out);
            load(app, &ctx2, &root);
            app.refresh_project(&ctx2, project);
        }) as Job
    });
}

pub fn git(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    if !root.join(".git").exists() {
        ui.label(RichText::new("This checkout is not a git repository.").color(p.dim));
        return;
    }
    let ctx = ui.ctx().clone();
    if !app.git.panels.get(root).is_some_and(|x| x.snap.is_some() || x.loading) {
        load(app, &ctx, root);
    }
    let panel = &app.git.panels[root];
    let (busy, loading) = (panel.busy, panel.loading);
    let snap = match &panel.snap {
        Some(Ok(s)) => s.clone(),
        Some(Err(e)) => {
            ui.label(RichText::new(format!("git failed: {e}")).color(p.red));
            return;
        }
        None => {
            ui.spinner();
            return;
        }
    };
    let id = q.id;
    type Work = Box<dyn FnOnce(&Path) -> (Output, Option<String>) + Send>;
    let mut run: Option<Work> = None;

    // Branch, upstream and the actions.
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{}  {}", icon::GIT_BRANCH, snap.branch.as_deref().unwrap_or("(detached)"))).font(theme::semibold(16.0)));
        match (&snap.upstream, snap.ahead_behind) {
            (Some(u), Some((a, b))) => {
                let state = if (a, b) == (0, 0) { "up to date".to_string() } else { crate::widgets::ahead_behind(Some((a, b))) };
                ui.label(RichText::new(format!("{u} · {state}")).color(p.dim));
            }
            _ => {
                ui.label(RichText::new("no upstream").color(p.faint));
            }
        }
        theme::right(ui, "git_ui.rs:210", egui::Align::Center, |ui| {
            if busy || loading {
                ui.spinner();
            }
            // The main (inverted) button is the one the state asks for: behind → Pull, else ahead → Push, else Fetch.
            let (ahead, behind) = snap.ahead_behind.unwrap_or((0, 0));
            let main = if behind > 0 { 1 } else if ahead > 0 { 0 } else { 2 };
            let btn = |ui: &mut egui::Ui, i: usize, text: String| if i == main { theme::primary(ui, text) } else { theme::secondary(ui, text) };
            ui.add_enabled_ui(!busy, |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Read the git state again").clicked() {
                    load(app, &ctx, root);
                }
                if btn(ui, 0, format!("{}  Push", icon::ARROW_UP)).on_hover_text("git push (sets the upstream on the first push)").clicked() {
                    run = Some(Box::new(|r: &Path| (git::push_current(r), None)));
                }
                if btn(ui, 1, format!("{}  Pull", icon::ARROW_DOWN)).on_hover_text("git pull --ff-only: never creates a merge commit").clicked() {
                    run = Some(Box::new(|r: &Path| (git::pull(r), None)));
                }
                if btn(ui, 2, format!("{}  Fetch", icon::CLOUD_ARROW_DOWN)).on_hover_text("git fetch --all --prune").clicked() {
                    run = Some(Box::new(|r: &Path| (git::fetch(r), None)));
                }
            });
        });
    });
    ui.add_space(10.0);

    let gap = 14.0;
    let w = ((ui.available_width() - gap) / 2.0).max(300.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            ui.set_width(w);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(caps(format!("Changes   {}", snap.changes.len()), p.faint));
                ui.add_space(4.0);
                if snap.changes.is_empty() {
                    ui.label(RichText::new("Nothing to commit.").color(p.dim));
                }
                for c in snap.changes.iter().take(40) {
                    ui.horizontal(|ui| {
                        let color = match c.code.trim() {
                            "??" => p.green,
                            s if s.contains('D') => p.red,
                            _ => p.orange,
                        };
                        ui.label(RichText::new(format!("{:>2}", c.code.trim())).monospace().size(12.5).color(color));
                        ui.add(egui::Label::new(RichText::new(&c.path).monospace().size(12.5)).truncate());
                    });
                }
                if snap.changes.len() > 40 {
                    ui.label(RichText::new(format!("and {} more", snap.changes.len() - 40)).color(p.faint));
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Commit from VS Code or an agent; the app does not stage or commit.").size(12.0).color(p.faint));
            });
            ui.add_space(gap);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(caps("Last commits", p.faint));
                ui.add_space(4.0);
                for c in &snap.commits {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&c.hash).monospace().size(12.0).color(p.blue));
                        // The date keeps a fixed column; the subject is cut before it.
                        let w = (ui.available_width() - 90.0).max(40.0);
                        ui.allocate_ui(vec2(w, 20.0), |ui| ui.add(egui::Label::new(&c.subject).truncate()));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(relative(&c.date)).size(12.0).color(p.faint)).on_hover_text(format!("{} · {}", c.author, c.date));
                        });
                    });
                }
                if snap.commits.is_empty() {
                    ui.label(RichText::new("No commits yet.").color(p.dim));
                }
            });
        });
        ui.vertical(|ui| {
            ui.set_width(w);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(caps(format!("Branches   {}", snap.branches.len()), p.faint));
                ui.add_space(4.0);
                for b in &snap.branches {
                    ui.horizontal(|ui| {
                        let current = snap.branch.as_ref() == Some(b);
                        ui.label(RichText::new(format!("{} {b}", if current { icon::CHECK } else { icon::GIT_BRANCH })).color(if current { p.text } else { p.dim }));
                        if current {
                            chip(ui, "checked out", p.blue);
                        } else if let Some(w) = snap.worktrees.iter().find(|w| w.branch.as_ref() == Some(b)) {
                            tag(ui, "in a worktree", p.purple).on_hover_text(&w.path);
                        } else {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add_enabled_ui(!busy, |ui| theme::ghost(ui, "Switch")).inner.on_hover_text(format!("git switch {b}")).clicked() {
                                    let b = b.clone();
                                    run = Some(Box::new(move |r: &Path| (git::switch(r, &b), None)));
                                }
                            });
                        }
                    });
                }
            });
            ui.add_space(gap);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                let panel = app.git.panels.get_mut(root).expect("loaded");
                // The main worktree, the checkout shown here and locked ones are not offered for removal.
                let removable = |k: usize, w: &git::Worktree| k > 0 && !w.locked && !same(&w.path, &root.to_string_lossy());
                panel.wt_selected.retain(|s| snap.worktrees.iter().enumerate().any(|(k, w)| &w.path == s && removable(k, w)));
                ui.horizontal(|ui| {
                    ui.label(caps(format!("Worktrees   {}", snap.worktrees.len()), p.faint));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = panel.wt_selected.len();
                        if ui.add_enabled_ui(n > 0 && !busy, |ui| theme::secondary(ui, format!("{}  Remove {n}", icon::TRASH))).inner.on_hover_text("Remove the selected worktrees (asks first)").clicked() {
                            (panel.wt_confirm, panel.wt_force, panel.wt_branches) = (true, false, true);
                        }
                        if theme::ghost(ui, "Select merged").on_hover_text("Worktrees without changes, merged into the default branch, and without Claude activity in the last 10 minutes").clicked() {
                            panel.wt_selected = snap.worktrees.iter().enumerate().filter(|(k, w)| removable(*k, w) && idle(w)).map(|(_, w)| w.path.clone()).collect();
                        }
                    });
                });
                ui.add_space(4.0);
                for (k, wt) in snap.worktrees.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let mut on = panel.wt_selected.contains(&wt.path);
                        if ui.add_enabled(removable(k, wt), egui::Checkbox::without_text(&mut on)).changed() {
                            if on {
                                panel.wt_selected.insert(wt.path.clone());
                            } else {
                                panel.wt_selected.remove(&wt.path);
                            }
                        }
                        ui.add(egui::Label::new(RichText::new(folder_name(&wt.path)).monospace().size(12.5)).truncate()).on_hover_text(&wt.path);
                        tag(ui, wt.branch.as_deref().unwrap_or("detached"), p.dim);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(26.0);
                        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                        if k == 0 {
                            chip(ui, "main", p.blue);
                        }
                        if same(&wt.path, &root.to_string_lossy()) {
                            chip(ui, "this checkout", p.blue);
                        }
                        if !q.checkouts.iter().any(|c| same(&c.path, &wt.path)) {
                            chip(ui, "not tracked", p.orange).on_hover_text("Add it under Checkouts to see it in the app");
                        }
                        use_chips(ui, wt, p);
                    });
                    ui.add_space(2.0);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(theme::field(&mut panel.wt_branch, 170.0).hint_text("branch"));
                    theme::checkbox(ui, &mut panel.wt_new, "new").on_hover_text("Create the branch from the current commit");
                    if !panel.wt_path_typed {
                        panel.wt_path = git::worktree_path(root, &panel.wt_branch).display().to_string();
                    }
                });
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 130.0).max(160.0);
                    panel.wt_path_typed |= ui.add(theme::field(&mut panel.wt_path, w).hint_text("folder")).changed();
                    let ready = !panel.wt_branch.trim().is_empty() && !busy;
                    if ui.add_enabled_ui(ready, |ui| theme::secondary(ui, format!("{}  Worktree", icon::PLUS))).inner.on_hover_text("git worktree add; the folder becomes a checkout").clicked() {
                        let (branch, path, new) = (panel.wt_branch.trim().to_string(), PathBuf::from(panel.wt_path.trim()), panel.wt_new);
                        panel.wt_branch.clear();
                        panel.wt_path_typed = false;
                        run = Some(Box::new(move |r: &Path| (git::add_worktree(r, &path, &branch, new), Some(path.display().to_string()))));
                    }
                });
            });
            ui.add_space(gap);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(caps(format!("Tags   {}", snap.tags.len()), p.faint));
                ui.add_space(4.0);
                if snap.tags.is_empty() {
                    ui.label(RichText::new("No tags.").color(p.dim));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for t in snap.tags.iter().take(30) {
                        tag(ui, t, p.blue);
                    }
                    if snap.tags.len() > 30 {
                        ui.label(RichText::new(format!("+{}", snap.tags.len() - 30)).color(p.faint));
                    }
                });
            });
        });
    });

    let log = &app.git.panels[root].log;
    if !log.is_empty() {
        ui.add_space(gap);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(caps("Output", p.faint));
            ui.add_space(4.0);
            for out in log.iter().rev().take(10) {
                ui.horizontal(|ui| {
                    let (glyph, color) = if out.ok { (icon::CHECK_CIRCLE, p.green) } else { (icon::X_CIRCLE, p.red) };
                    ui.label(RichText::new(glyph).color(color));
                    ui.label(RichText::new(&out.command).monospace().size(12.5));
                });
                if !out.text.is_empty() {
                    egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(6).inner_margin(8).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(RichText::new(&out.text).monospace().size(12.0).color(p.dim)).wrap());
                    });
                }
                ui.add_space(4.0);
            }
        });
    }
    let panel = app.git.panels.get_mut(root).expect("loaded");
    let mut remove_now: Option<(Vec<String>, bool, bool)> = None;
    if panel.wt_confirm {
        let chosen: Vec<&git::Worktree> = snap.worktrees.iter().filter(|w| panel.wt_selected.contains(&w.path)).collect();
        let dirty = chosen.iter().filter(|w| w.changes.unwrap_or(0) > 0).count();
        let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Modal::new(egui::Id::new(("remove-worktrees", root))).show(&ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new(format!("Remove {} worktrees?", chosen.len())).font(theme::semibold(16.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("Their folders are deleted. Committed work stays on the branches.").color(p.dim));
            ui.add_space(8.0);
            for w in &chosen {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                    ui.label(RichText::new(folder_name(&w.path)).monospace().size(12.5));
                    tag(ui, w.branch.as_deref().unwrap_or("detached"), p.dim);
                    use_chips(ui, w, p);
                });
            }
            ui.add_space(8.0);
            theme::checkbox(ui, &mut panel.wt_branches, "Also delete their branches when merged (git branch -d keeps unmerged ones)");
            if dirty > 0 {
                theme::checkbox(ui, &mut panel.wt_force, format!("Also remove the {dirty} with uncommitted changes (those changes are lost)"))
                    .on_hover_text("Without this, git refuses them and they stay");
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, format!("Remove {}", chosen.len())).clicked() {
                    remove_now = Some((chosen.iter().map(|w| w.path.clone()).collect(), panel.wt_force, panel.wt_branches));
                    close = true;
                }
                close |= theme::ghost(ui, "Cancel").clicked();
            });
        });
        if close {
            panel.wt_confirm = false;
        }
    }
    if let Some((paths, force, branches)) = remove_now {
        remove_worktrees(app, &ctx, id, root, paths, force, branches);
    } else if let Some(work) = run {
        action(app, &ctx, id, root, work);
    }
}

/// Remove worktrees in the background; checkouts whose folder is gone leave the project.
fn remove_worktrees(app: &mut App, ctx: &egui::Context, project: i64, root: &Path, paths: Vec<String>, force: bool, branches: bool) {
    let panel = app.git.panels.entry(root.to_path_buf()).or_default();
    if panel.busy {
        return;
    }
    panel.busy = true;
    let (root, ctx2) = (root.to_path_buf(), ctx.clone());
    app.background(ctx, move || {
        let outs = git::remove_worktrees(&root, &paths, force, branches);
        Box::new(move |app: &mut App| {
            let gone: Vec<i64> = app
                .projects
                .iter()
                .filter(|q| q.id == project)
                .flat_map(|q| &q.checkouts)
                .filter(|c| paths.iter().any(|p| same(p, &c.path)) && !Path::new(&c.path).exists())
                .map(|c| c.id)
                .collect();
            for c in gone {
                let r = app.store.remove_checkout(c);
                app.ok(r);
            }
            app.reload();
            let removed = outs.iter().filter(|o| o.ok && o.command.starts_with("git worktree")).count();
            app.message = Some(format!("Removed {removed} of {} worktrees; see the Output card", paths.len()));
            let panel = app.git.panels.entry(root.clone()).or_default();
            panel.busy = false;
            panel.wt_selected.clear();
            panel.log.extend(outs);
            load(app, &ctx2, &root);
            app.refresh_project(&ctx2, project);
        }) as Job
    });
}

// --- GitHub ---------------------------------------------------------------------------------------------------

pub fn github(app: &mut App, ui: &mut egui::Ui, q: &Project, root: &Path, p: Palette) {
    let id = q.id;
    let ctx = ui.ctx().clone();
    let remote = q.checkouts.iter().find(|c| Path::new(&c.path) == root).and_then(|c| c.info.as_ref()).and_then(|i| i.git.as_ref()).and_then(|g| g.remote.clone());
    let on_github = remote.as_deref().and_then(pa_core::info::github_url);
    let Some(url) = on_github else {
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(caps("GitHub", p.faint));
            ui.add_space(4.0);
            if !root.join(".git").exists() {
                ui.label(RichText::new("This checkout is not a git repository, so it cannot be published.").color(p.dim));
                return;
            }
            match &remote {
                Some(r) => {
                    ui.label(RichText::new(format!("The origin remote is not on GitHub: {r}")).color(p.dim));
                }
                None => {
                    ui.label(RichText::new("This repository is only on this computer.").color(p.dim));
                    ui.add_space(6.0);
                    if theme::primary(ui, format!("{}  Create GitHub repo…", icon::GITHUB_LOGO)).on_hover_text("gh repo create: asks for owner and name, then confirms").clicked() {
                        open_create(app, &ctx, q, root);
                    }
                }
            }
        });
        return;
    };
    if let std::collections::hash_map::Entry::Vacant(e) = app.git.github.entry(id) {
        e.insert(None);
        let root = root.to_path_buf();
        app.background(&ctx, move || {
            let info = github::info(&root).map_err(|e| e.0);
            Box::new(move |app: &mut App| {
                app.git.github.insert(id, Some(info));
            })
        });
    }
    ui.horizontal_wrapped(|ui| {
        ui.hyperlink_to(RichText::new(format!("{}  {}", icon::GITHUB_LOGO, url.trim_start_matches("https://"))).font(theme::semibold(15.0)), &url);
        theme::right(ui, "git_ui.rs:546", egui::Align::Center, |ui| {
            match app.git.github.get(&id) {
                Some(None) => {
                    ui.spinner();
                }
                _ => {
                    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Ask GitHub again").clicked() {
                        app.git.github.remove(&id);
                    }
                }
            }
        });
    });
    ui.add_space(8.0);
    match app.git.github.get(&id).cloned().flatten() {
        Some(Ok(info)) => info_card(ui, &info, p),
        Some(Err(e)) => {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("gh failed: {e}")).color(p.red));
                ui.label(RichText::new("Is the GitHub CLI installed and signed in (gh auth status)? Offline?").size(12.5).color(p.dim));
            });
        }
        None => {}
    }
    ui.add_space(12.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps("Issues and TODOs", p.faint));
        ui.add_space(4.0);
        ui.label(RichText::new("A (#12) at the end of a TODO line links it to issue 12. In TODOs: hover an item to create its issue, “Import issues” adds open issues to a section, and ticking a linked item offers to close its issue.").color(p.dim));
        ui.add_space(6.0);
        let key = github::capture_key(id);
        let mut on = app.setting(&key, "false") == "true";
        if theme::checkbox(ui, &mut on, "Captures become GitHub issues").on_hover_text("Quick capture (Ctrl+Shift+K) of an idea, bug or TODO for this project also opens an issue (labels from the kind) and writes its number on the Inbox line. Offline, the capture just stays in the Inbox.").changed() {
            let r = app.store.set_setting(&key, if on { "true" } else { "false" });
            app.ok(r);
        }
    });
}

fn info_card(ui: &mut egui::Ui, info: &RepoInfo, p: Palette) {
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            let vis = info.visibility.to_lowercase();
            chip(ui, &vis, if vis == "public" { p.green } else { p.dim });
            let stat = |ui: &mut egui::Ui, glyph: &str, n: u64, what: &str, link: String| {
                ui.hyperlink_to(RichText::new(format!("{glyph} {n} {what}")).color(p.text), link);
            };
            stat(ui, icon::STAR, info.stars, "stars", format!("{}/stargazers", info.url));
            stat(ui, icon::CIRCLE_DASHED, info.open_issues, "open issues", format!("{}/issues", info.url));
            stat(ui, icon::GIT_PULL_REQUEST, info.open_prs, "open pull requests", format!("{}/pulls", info.url));
        });
        ui.add_space(8.0);
        egui::Grid::new(("gh_info", &info.slug)).num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Default branch").color(p.dim));
            ui.label(info.default_branch.as_deref().unwrap_or("—"));
            ui.end_row();
            ui.label(RichText::new("Latest release").color(p.dim));
            match &info.latest_release {
                Some(r) => {
                    ui.horizontal(|ui| {
                        ui.hyperlink_to(RichText::new(&r.tag).monospace(), &r.url);
                        if r.name != r.tag && !r.name.is_empty() {
                            ui.label(RichText::new(&r.name).color(p.dim));
                        }
                        ui.label(RichText::new(relative(&r.published)).color(p.faint));
                    });
                }
                None => {
                    ui.label(RichText::new("none").color(p.faint));
                }
            }
            ui.end_row();
            ui.label(RichText::new("CI").color(p.dim));
            match &info.ci {
                Some(ci) => {
                    ui.horizontal(|ui| {
                        let color = match ci.state() {
                            "success" => p.green,
                            "failure" | "startup_failure" | "timed_out" => p.red,
                            "in_progress" | "queued" | "waiting" | "pending" => p.blue,
                            _ => p.dim,
                        };
                        chip(ui, ci.state().replace('_', " "), color);
                        ui.hyperlink_to(&ci.workflow, &ci.url);
                    });
                }
                None => {
                    ui.label(RichText::new("no workflow runs").color(p.faint));
                }
            }
            ui.end_row();
        });
    });
}

fn open_create(app: &mut App, ctx: &egui::Context, q: &Project, root: &Path) {
    let name = root.file_name().map_or_else(|| q.name.clone(), |n| n.to_string_lossy().into_owned());
    app.git.create = Some(Create { project: q.id, root: root.to_path_buf(), owner: String::new(), name, private: true, push: true, running: false });
    app.background(ctx, || {
        let user = github::current_user();
        Box::new(move |app: &mut App| {
            if let (Some(c), Some(u)) = (app.git.create.as_mut(), user)
                && c.owner.is_empty()
            {
                c.owner = u;
            }
        })
    });
}

/// The "Create GitHub repo" confirmation: owner, name, private (default) and push, the exact command, then create.
pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(c) = app.git.create.as_mut() else { return };
    let p = crate::theme::pal(ctx);
    let (mut go, mut cancel) = (false, ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !c.running);
    egui::Modal::new(egui::Id::new("create_repo")).show(ctx, |ui| {
        let w = 520.0;
        ui.set_width(w);
        ui.label(RichText::new(format!("{}  Create GitHub repo", icon::GITHUB_LOGO)).font(theme::semibold(16.0)));
        ui.add_space(8.0);
        egui::Grid::new("create_repo_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Owner").color(p.dim));
            ui.add(theme::field(&mut c.owner, 300.0).hint_text("your user or an organisation"));
            ui.end_row();
            ui.label(RichText::new("Name").color(p.dim));
            ui.add(theme::field(&mut c.name, 300.0));
            ui.end_row();
        });
        theme::checkbox(ui, &mut c.private, "Private (only you and people you invite can see it)");
        theme::checkbox(ui, &mut c.push, "Push the current branch now");
        ui.add_space(6.0);
        let command = github::repo_create_command(&c.root, &c.owner, &c.name, c.private, c.push);
        ui.label(caps("Runs", p.faint));
        ui.add(egui::Label::new(RichText::new(&command).monospace().size(12.0)).wrap());
        ui.add_space(6.0);
        let what = if c.private { "a private repository" } else { "a PUBLIC repository, visible to everyone" };
        ui.label(RichText::new(format!("This creates {what} on GitHub and adds it as the origin remote{}.", if c.push { ", then uploads the code" } else { "" })).color(p.orange));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if c.running {
                ui.spinner();
                ui.label("Creating…");
            } else {
                let ready = !c.name.trim().is_empty();
                go = ui.add_enabled_ui(ready, |ui| theme::primary(ui, "Create repository")).inner.clicked();
                cancel |= theme::ghost(ui, "Cancel").clicked();
            }
        });
    });
    if cancel {
        app.git.create = None;
        return;
    }
    if !go {
        return;
    }
    c.running = true;
    let (root, owner, name, private, push, project) = (c.root.clone(), c.owner.clone(), c.name.clone(), c.private, c.push, c.project);
    let ctx2 = ctx.clone();
    app.background(ctx, move || {
        let result = github::create_repo(&root, &owner, &name, private, push);
        Box::new(move |app: &mut App| {
            match result {
                Ok(url) => {
                    app.message = Some(format!("Created {url}"));
                    app.git.create = None;
                    app.git.github.remove(&project);
                    app.refresh_project(&ctx2, project);
                }
                Err(e) => {
                    app.message = Some(format!("Not created: {e}"));
                    if let Some(c) = app.git.create.as_mut() {
                        c.running = false;
                    }
                }
            }
        })
    });
}
