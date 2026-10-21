//! Settings: scan roots, ignored folders, groups, purposes (with their behaviour flags, D-9) and launchers.

use crate::theme::{self, caps, card, pal};
use crate::{App, LAUNCHERS};

#[derive(Default)]
pub struct State {
    /// Launcher commands being edited (key → text), saved when the field loses focus.
    launchers: std::collections::HashMap<&'static str, String>,
    /// Registration of pa-mcp per agent (same order as `AGENTS`); None until read.
    agents: Option<Vec<Reg>>,
    agents_busy: bool,
    /// Command line waiting for the user's Run (agent index, line).
    pending: Option<(usize, String)>,
    /// Output of the last failed agent command.
    agent_error: Option<String>,
    /// The last update check: the newest release, or why there is none.
    latest: Option<Result<pa_core::update::Latest, String>>,
    /// What the update row is doing ("Checking…", "Downloading…"); None when idle.
    update_busy: Option<&'static str>,
    /// An install failure, or on Linux the command that installs the downloaded RPM.
    update_note: Option<Result<String, String>>,
}

/// Agents that can register pa-mcp: (name, program).
const AGENTS: [(&str, &str); 2] = [("Claude Code", "claude"), ("Codex CLI", "codex")];
const SERVER: &str = "projectarchitect";

/// pa-mcp's state in one agent's config.
#[derive(Debug, PartialEq)]
enum Reg {
    /// The agent is not on PATH.
    NoAgent,
    NotRegistered,
    /// Registered with the installed pa-mcp; the agent's status line, if it prints one.
    Registered(Option<String>),
    /// Registered with another command.
    Elsewhere(String),
}

#[derive(Clone, Copy)]
enum Op {
    Add,
    Update,
    Remove,
}

/// The installed pa-mcp (D-47), the fixed path agents should point at.
fn installed_mcp() -> std::path::PathBuf {
    if cfg!(windows) { pa_core::install::default_dir().unwrap_or_default().join("pa-mcp.exe") } else { "/usr/bin/pa-mcp".into() }
}

/// The shell command line for `op` (run through `cmd /C` or `sh -c`, so npm `.cmd` shims resolve).
fn agent_line(program: &str, op: Op, exe: &str, data_dir: Option<&str>) -> String {
    let mut target = format!(r#""{exe}""#);
    if let Some(d) = data_dir {
        target.push_str(&format!(r#" --data-dir "{d}""#));
    }
    let (add, remove) = if program == "claude" {
        (format!("claude mcp add --scope user {SERVER} -- {target}"), format!("claude mcp remove {SERVER} -s user"))
    } else {
        (format!("{program} mcp add {SERVER} -- {target}"), format!("{program} mcp remove {SERVER}"))
    };
    match op {
        Op::Add => add,
        Op::Update => format!("{remove} && {add}"),
        Op::Remove => remove,
    }
}

/// Run a command line through the shell (`cmd /C` / `sh -c`): (exit code, stdout + stderr). Code None: it did not start.
fn shell(line: &str) -> (Option<i32>, String) {
    #[cfg(windows)]
    let out = {
        use std::os::windows::process::CommandExt;
        pa_core::cmd("cmd").arg("/C").raw_arg(line).output()
    };
    #[cfg(not(windows))]
    let out = pa_core::cmd("sh").arg("-c").arg(line).output();
    match out {
        Ok(o) => (o.status.code(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(e) => (None, e.to_string()),
    }
}

/// Read `<agent> mcp get projectarchitect` output (Claude: `Command:` / `Status:` lines; Codex: `command:`).
fn parse_get(out: &str, exe: &str) -> Reg {
    let field = |key: &str| {
        out.lines().find_map(|l| {
            let (k, v) = l.trim().split_once(':')?;
            k.trim().eq_ignore_ascii_case(key).then(|| v.trim().trim_matches('"').to_string())
        })
    };
    let Some(command) = field("command") else { return Reg::NotRegistered };
    // ponytail: compares the exe only, not `--data-dir` in the args; re-add by hand when switching data dirs.
    let same = if cfg!(windows) { command.eq_ignore_ascii_case(exe) } else { command == exe };
    if same { Reg::Registered(field("status")) } else { Reg::Elsewhere(command) }
}

/// Re-read every agent's registration on a background thread (`claude mcp get` health-checks the server).
fn read_agents(app: &mut App, ctx: &egui::Context) {
    app.settings.agents_busy = true;
    let exe = installed_mcp().display().to_string();
    app.background(ctx, move || {
        let regs: Vec<Reg> = AGENTS
            .iter()
            .map(|(_, program)| {
                let on_path = if cfg!(windows) { format!("where {program}") } else { format!("command -v {program}") };
                if shell(&on_path).0 != Some(0) {
                    return Reg::NoAgent;
                }
                parse_get(&shell(&format!("{program} mcp get {SERVER}")).1, &exe)
            })
            .collect();
        Box::new(move |app: &mut App| {
            app.settings.agents = Some(regs);
            app.settings.agents_busy = false;
        })
    });
}

/// Run the pending command line, then re-read the registrations.
fn run_agent_line(app: &mut App, ctx: &egui::Context, line: String) {
    app.settings.agents_busy = true;
    app.settings.agent_error = None;
    let ui_ctx = ctx.clone();
    app.background(ctx, move || {
        let (code, out) = shell(&line);
        Box::new(move |app: &mut App| {
            if code != Some(0) {
                app.settings.agent_error = Some(format!("`{line}` failed:\n{}", out.trim()));
            }
            read_agents(app, &ui_ctx);
        })
    });
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        section(ui, "Appearance", |ui| appearance(app, ui));
        section(ui, "Scan roots", |ui| roots(app, ui));
        section(ui, "Ignored folders", |ui| ignored(app, ui));
        section(ui, "Groups", |ui| groups(app, ui));
        section(ui, "Purposes", |ui| purposes(app, ui));
        section(ui, "Launchers", |ui| launchers(app, ui));
        section(ui, "MCP server", |ui| mcp(app, ui));
        section(ui, "Updates", |ui| updates(app, ui));
        section(ui, "Data", |ui| {
            ui.horizontal(|ui| {
                ui.label("Database:");
                ui.monospace(app.db_path.display().to_string());
            });
        });
    });
}

/// Current version, "Check for updates", and when a newer release exists its Install (D-49).
fn updates(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    let ctx = ui.ctx().clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("ProjectArchitect v{}", env!("CARGO_PKG_VERSION")));
        match &app.settings.latest {
            Some(Ok(l)) if l.is_newer() => {
                theme::chip(ui, format!("v{} available", l.version()), p.green);
            }
            Some(Ok(_)) => {
                theme::chip(ui, "up to date", p.dim);
            }
            _ => {}
        }
        if let Some(what) = app.settings.update_busy {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(egui::RichText::new(what).color(p.dim));
            return;
        }
        if theme::secondary(ui, "Check for updates").clicked() {
            app.settings.update_busy = Some("Checking…");
            app.settings.update_note = None;
            app.background(&ctx, || {
                let r = pa_core::update::latest_release().map_err(|e| e.0);
                Box::new(move |app: &mut App| {
                    app.settings.latest = Some(r);
                    app.settings.update_busy = None;
                })
            });
        }
        if let Some(Ok(l)) = &app.settings.latest
            && l.is_newer()
            && theme::primary(ui, format!("Install {}", l.version())).clicked()
        {
            let l = l.clone();
            install_update(app, &ctx, l);
        }
    });
    match (&app.settings.latest, &app.settings.update_note) {
        (_, Some(Err(e))) | (Some(Err(e)), None) => {
            ui.label(egui::RichText::new(e).color(p.red));
        }
        (_, Some(Ok(line))) => {
            ui.label(egui::RichText::new("Downloaded. Install it from a terminal:").color(p.dim));
            ui.horizontal_wrapped(|ui| {
                ui.monospace(line);
                if theme::secondary(ui, "Copy").clicked() {
                    ui.ctx().copy_text(line.clone());
                }
            });
        }
        _ => {}
    }
}

/// Download the release's installer into `<data dir>/updates`. Windows: run the MSI, start the installed pa.exe
/// with this window's `--data-dir` and close. Linux: show the `dnf` command (installing needs root).
fn install_update(app: &mut App, ctx: &egui::Context, latest: pa_core::update::Latest) {
    app.settings.update_busy = Some(if cfg!(windows) { "Downloading and installing…" } else { "Downloading…" });
    app.settings.update_note = None;
    let dir = app.data_dir.join("updates");
    let data = (app.data_dir != pa_core::default_data_dir()).then(|| app.data_dir.clone());
    let ui_ctx = ctx.clone();
    app.background(ctx, move || {
        let ext = if cfg!(windows) { ".msi" } else { ".rpm" };
        let r = (|| -> Result<String, String> {
            let asset = latest.asset(ext).ok_or_else(|| format!("{} has no {ext} attached", latest.tag))?;
            let file = pa_core::update::download(&latest.tag, asset, &dir).map_err(|e| e.0)?;
            if !cfg!(windows) {
                return Ok(format!(r#"sudo dnf install "{}""#, file.display()));
            }
            pa_core::update::install_msi(&file).map_err(|e| e.0)?;
            let exe = pa_core::install::default_dir().unwrap_or_default().join("pa.exe");
            let mut c = std::process::Command::new(&exe);
            if let Some(d) = &data {
                c.arg("--data-dir").arg(d);
            }
            c.spawn().map_err(|e| format!("Installed, but starting {} failed: {e}", exe.display()))?;
            Ok(String::new())
        })();
        Box::new(move |app: &mut App| {
            app.settings.update_busy = None;
            match r {
                Ok(_) if cfg!(windows) => ui_ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                r => app.settings.update_note = Some(r),
            }
        })
    });
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let p = pal(ui.ctx());
    ui.set_max_width(900.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(caps(title, p.dim));
        ui.add_space(6.0);
        body(ui);
    });
    ui.add_space(14.0);
}

fn appearance(app: &mut App, ui: &mut egui::Ui) {
    let current = app.setting("theme", "system");
    let options = ["system", "dark", "light"];
    let items: Vec<(String, Option<usize>)> = ["Follow system", "Dark", "Light"].iter().map(|s| (s.to_string(), None)).collect();
    let sel = options.iter().position(|o| *o == current).unwrap_or(0);
    if let Some(i) = theme::segmented(ui, &items, sel) {
        let r = app.store.set_setting("theme", options[i]);
        app.ok(r);
        theme::apply(ui.ctx(), options[i]);
    }
}

fn roots(app: &mut App, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Folders the Import view scans for projects, with how many folder levels below them it looks.").color(pal(ui.ctx()).dim));
    let roots = app.store.scan_roots().unwrap_or_default();
    egui::Grid::new("roots").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
        for (path, depth) in roots {
            ui.label(&path);
            let mut d = depth;
            if ui.add(egui::DragValue::new(&mut d).range(0..=8).prefix("depth ")).changed() {
                let r = app.store.set_scan_root(&path, d);
                app.ok(r);
            }
            if theme::danger_ghost(ui, "Remove").clicked() {
                let r = app.store.remove_scan_root(&path);
                app.ok(r);
            }
            ui.end_row();
        }
    });
    if theme::secondary(ui, "Add scan root…").clicked()
        && let Some(dir) = rfd::FileDialog::new().pick_folder()
    {
        let r = app.store.set_scan_root(&pa_core::norm(&dir), 3);
        app.ok(r);
    }
}

fn ignored(app: &mut App, ui: &mut egui::Ui) {
    let list = app.store.ignored().unwrap_or_default();
    if list.is_empty() {
        ui.label(egui::RichText::new("None. Folders you ignore in the Import view are listed here.").color(pal(ui.ctx()).dim));
    }
    for path in list {
        ui.horizontal(|ui| {
            ui.label(&path);
            if theme::ghost(ui, "Unignore").clicked() {
                let r = app.store.unignore(&path);
                app.ok(r);
            }
        });
    }
}

fn groups(app: &mut App, ui: &mut egui::Ui) {
    let mut changed = false;
    egui::Grid::new("groups").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
        for g in app.groups.clone() {
            let mut name = g.name.clone();
            if ui.add(theme::field(&mut name, 220.0)).lost_focus() && name.trim() != g.name && !name.trim().is_empty() {
                let r = app.store.update_group(&pa_core::Group { name: name.trim().to_string(), ..g.clone() });
                changed |= app.ok(r).is_some();
            }
            ui.label(egui::RichText::new(g.path.as_deref().unwrap_or("(no folder)")).color(pal(ui.ctx()).dim));
            ui.label(format!("{} projects", app.projects.iter().filter(|p| p.group_id == Some(g.id)).count()));
            if theme::danger_ghost(ui, "Delete").on_hover_text("Its projects stay, without a group").clicked() {
                let r = app.store.delete_group(g.id);
                changed |= app.ok(r).is_some();
            }
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        if theme::secondary(ui, "New group").clicked() {
            let r = app.store.add_group("New group", None);
            changed |= app.ok(r).is_some();
        }
        if theme::secondary(ui, "New group from folder…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            let path = pa_core::norm(&dir);
            let r = app.store.add_group(&pa_core::folder_name(&path), Some(&path));
            changed |= app.ok(r).is_some();
        }
    });
    if changed {
        app.reload();
    }
}

fn purposes(app: &mut App, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Why you have a project. The flags set what the app does for projects with that purpose.").color(pal(ui.ctx()).dim));
    let mut changed = false;
    egui::Grid::new("purposes").num_columns(7).spacing([12.0, 4.0]).show(ui, |ui| {
        for h in ["Name", "Order", "Track TODOs", "Watch upstream", "Show in overview", "Scan code TODOs", ""] {
            ui.strong(h);
        }
        ui.end_row();
        for orig in app.purposes.clone() {
            let mut p = orig.clone();
            let name_done = ui.add(theme::field(&mut p.name, 170.0)).lost_focus();
            ui.add(egui::DragValue::new(&mut p.sort).range(0..=99));
            theme::checkbox(ui, &mut p.track_todos, "");
            theme::checkbox(ui, &mut p.watch_upstream, "").on_hover_text("Fetch and show new upstream commits (M4)");
            theme::checkbox(ui, &mut p.show_in_overview, "");
            theme::checkbox(ui, &mut p.scan_code_todos, "");
            let delete = theme::danger_ghost(ui, "Delete").on_hover_text("Its projects stay, without a purpose").clicked();
            ui.end_row();
            if delete {
                let r = app.store.delete_purpose(p.id);
                changed |= app.ok(r).is_some();
                continue;
            }
            // The name is saved when its field loses focus; the other fields at once.
            if !name_done || p.name.trim().is_empty() {
                p.name = orig.name.clone();
            }
            if p != orig {
                p.name = p.name.trim().to_string();
                let r = app.store.update_purpose(&p);
                changed |= app.ok(r).is_some();
            }
        }
    });
    if theme::secondary(ui, "New purpose").clicked() {
        let r = app.store.add_purpose("New purpose");
        changed |= app.ok(r).is_some();
    }
    if changed {
        app.reload();
    }
}

/// How to connect an agent (Claude Code) to `pa-mcp`, and whether it may publish (push, open pull requests).
fn mcp(app: &mut App, ui: &mut egui::Ui) {
    let p = pal(ui.ctx());
    let ctx = ui.ctx().clone();
    let exe = installed_mcp();
    let exe_s = exe.display().to_string();
    ui.label(egui::RichText::new(format!("Agents reach the app through pa-mcp, a local stdio MCP server installed at {exe_s}.")).color(p.dim));
    if !exe.exists() {
        ui.label(
            egui::RichText::new(r"pa-mcp is not installed there: build it with `cargo build --release -p pa-mcp -p pa`, then run `target\release\pa-cli install-mcp`.")
                .color(p.orange),
        );
    }
    if app.settings.agents.is_none() && !app.settings.agents_busy {
        read_agents(app, &ctx);
    }
    let data = (app.data_dir != pa_core::default_data_dir()).then(|| app.data_dir.display().to_string());
    let busy = app.settings.agents_busy;
    let mut ask = None;
    egui::Grid::new("agents").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
        for (i, (name, program)) in AGENTS.iter().enumerate() {
            ui.label(*name);
            let reg = app.settings.agents.as_ref().and_then(|a| a.get(i));
            ui.horizontal(|ui| match reg {
                None => {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                Some(Reg::NoAgent) => {
                    theme::chip(ui, "not installed", p.dim).on_hover_text(format!("`{program}` is not on PATH"));
                }
                Some(Reg::NotRegistered) => {
                    theme::chip(ui, "not registered", p.dim);
                }
                Some(Reg::Registered(status)) => {
                    let ok = status.as_deref().is_none_or(|s| s.contains("Connected"));
                    let text = if status.is_some() && ok { "connected" } else { "registered" };
                    theme::chip(ui, text, if ok { p.green } else { p.orange });
                    if !ok {
                        ui.label(egui::RichText::new(status.clone().unwrap_or_default()).color(p.orange));
                    }
                }
                Some(Reg::Elsewhere(other)) => {
                    theme::chip(ui, "points elsewhere", p.orange);
                    ui.monospace(other);
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let ops: &[(&str, Op)] = match reg {
                        Some(Reg::NotRegistered) => &[("Add", Op::Add)],
                        Some(Reg::Registered(_)) => &[("Remove", Op::Remove)],
                        Some(Reg::Elsewhere(_)) => &[("Update", Op::Update), ("Remove", Op::Remove)],
                        _ => &[],
                    };
                    for (label, op) in ops {
                        let b = if matches!(op, Op::Remove) { theme::danger_ghost(ui, *label) } else { theme::secondary(ui, *label) };
                        if b.clicked() {
                            ask = Some((i, agent_line(program, *op, &exe_s, data.as_deref())));
                        }
                    }
                });
            });
            ui.end_row();
        }
    });
    if ask.is_some() {
        app.settings.pending = ask;
    }
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            if theme::ghost(ui, "Refresh").clicked() {
                read_agents(app, &ctx);
            }
        });
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    if let Some((i, line)) = app.settings.pending.clone() {
        ui.label(egui::RichText::new(format!("{} will run:", AGENTS[i].0)).color(p.dim));
        ui.horizontal_wrapped(|ui| {
            // A &str buffer: selectable, not editable.
            let w = (ui.available_width() - 200.0).max(300.0);
            ui.add(egui::TextEdit::singleline(&mut line.as_str()).font(egui::TextStyle::Monospace).desired_width(w).min_size(egui::vec2(w, 32.0)).margin(egui::vec2(10.0, 7.0)));
            if theme::secondary(ui, "Copy").clicked() {
                ui.ctx().copy_text(line.clone());
            }
            if theme::primary(ui, "Run").clicked() {
                app.settings.pending = None;
                run_agent_line(app, &ctx, line.clone());
            }
            if theme::ghost(ui, "Cancel").clicked() {
                app.settings.pending = None;
            }
        });
    }
    if let Some(err) = &app.settings.agent_error {
        ui.label(egui::RichText::new(err).color(p.red).monospace());
    }
    ui.add_space(4.0);
    let mut publish = app.setting("mcp.allow_publish", "false") == "true";
    if theme::checkbox(ui, &mut publish, "Allow agents to push, open pull requests and create GitHub repositories and issues").on_hover_text("Off: the MCP tools that publish (git_push, create_issue, close_issue, create_github_repo, change_todo_style with push_and_pr) are refused").changed() {
        let r = app.store.set_setting("mcp.allow_publish", if publish { "true" } else { "false" });
        app.ok(r);
    }
    let key = pa_core::claude::ALLOW_WRITE;
    let mut claude_write = app.setting(key, "false") == "true";
    if theme::checkbox(ui, &mut claude_write, "Allow agents to write Claude config (copy_claude_item, edit_claude_item with apply)")
        .on_hover_text("Off: those tools only return the diff preview. On: they overwrite CLAUDE.md, settings, agents… after a backup")
        .changed()
    {
        let r = app.store.set_setting(key, if claude_write { "true" } else { "false" });
        app.ok(r);
    }
}

fn launchers(app: &mut App, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Commands run through the shell; {path} is the checkout folder.").color(pal(ui.ctx()).dim));
    egui::Grid::new("launchers").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
        for &(key, label, default) in LAUNCHERS {
            ui.label(label);
            let current = app.setting(key, default);
            let text = app.settings.launchers.entry(key).or_insert(current);
            if ui.add(theme::field(text, 380.0).font(egui::TextStyle::Monospace)).lost_focus() {
                let r = app.store.set_setting(key, &text.clone());
                app.ok(r);
            }
            if theme::ghost(ui, "Reset").clicked() {
                app.settings.launchers.insert(key, default.to_string());
                let r = app.store.set_setting(key, default);
                app.ok(r);
            }
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\me\AppData\Local\Programs\ProjectArchitect\pa-mcp.exe";

    #[test]
    fn parse_get_output() {
        let connected = format!(
            "projectarchitect:\n  Scope: User config (available in all your projects)\n  Status: ✔ Connected\n  Type: stdio\n  Command: {EXE}\n  Args:\n  Environment:\n\nTo remove this server, run: claude mcp remove projectarchitect -s user\n"
        );
        assert_eq!(parse_get(&connected, EXE), Reg::Registered(Some("✔ Connected".into())));
        let missing = "No MCP server named \"projectarchitect\". Configured servers: a, b\n";
        assert_eq!(parse_get(missing, EXE), Reg::NotRegistered);
        let other = "projectarchitect:\n  Status: ✘ Failed to connect\n  Command: D:\\dev\\target\\release\\pa-mcp.exe\n";
        assert_eq!(parse_get(other, EXE), Reg::Elsewhere(r"D:\dev\target\release\pa-mcp.exe".into()));
        // Codex: lowercase keys, no status line.
        assert_eq!(parse_get(&format!("projectarchitect\n  enabled: true\n  command: {EXE}\n  args: -\n"), EXE), Reg::Registered(None));
    }

    #[test]
    fn command_lines() {
        assert_eq!(agent_line("claude", Op::Add, EXE, None), format!(r#"claude mcp add --scope user projectarchitect -- "{EXE}""#));
        assert_eq!(agent_line("claude", Op::Remove, EXE, None), "claude mcp remove projectarchitect -s user");
        assert_eq!(
            agent_line("claude", Op::Update, EXE, Some(r"D:\data")),
            format!(r#"claude mcp remove projectarchitect -s user && claude mcp add --scope user projectarchitect -- "{EXE}" --data-dir "D:\data""#)
        );
        assert_eq!(agent_line("codex", Op::Add, EXE, None), format!(r#"codex mcp add projectarchitect -- "{EXE}""#));
        assert_eq!(agent_line("codex", Op::Remove, EXE, None), "codex mcp remove projectarchitect");
    }
}
