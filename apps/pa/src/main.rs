//! ProjectArchitect GUI: the overview of every tracked project, a tab per opened project, the Import view
//! and Settings. All data goes through `pa_core::Store`; folder info is collected on background threads.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod board_page;
mod capture;
mod chart;
mod claude_ui;
mod dashboard_ui;
mod import;
mod overview;
mod palette;
mod progress_ui;
mod project;
mod restyle;
mod roadmap_ui;
mod settings;
mod templates_ui;
mod theme;
mod todos_page;
mod deps_ui;
mod git_ui;
mod tasks_ui;

use egui::{Id, RichText, WidgetText};
use egui_phosphor::regular as icon;
use theme::{caps, pal};
use egui_dock::{DockArea, DockState, TabViewer, tab_viewer::OnCloseResponse};
use pa_core::{Group, Info, Project, Purpose, Store, info};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Launchers (D-8): settings key, button label, default command (`{path}` = the checkout folder).
/// The order is used: [0] is the default open action, [1] the file manager.
#[cfg(windows)]
pub const LAUNCHERS: &[(&str, &str, &str)] = &[
    ("launch.code", "VS Code", r#"code "{path}""#),
    ("launch.explorer", "Explorer", r#"explorer "{path}""#),
    ("launch.terminal", "Terminal", r#"wt -d "{path}""#),
    ("launch.claude", "Claude", r#"wt -d "{path}" claude"#),
];
/// Fedora's terminal is Ptyxis (GNOME Terminal before Fedora 41).
#[cfg(not(windows))]
pub const LAUNCHERS: &[(&str, &str, &str)] = &[
    ("launch.code", "VS Code", r#"code "{path}""#),
    ("launch.explorer", "Files", r#"xdg-open "{path}""#),
    ("launch.terminal", "Terminal", r#"ptyxis --new-window --working-directory "{path}" || gnome-terminal --working-directory "{path}""#),
    ("launch.claude", "Claude", r#"ptyxis --new-window --working-directory "{path}" -- claude || gnome-terminal --working-directory "{path}" -- claude"#),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Overview,
    Todos,
    Import,
    Settings,
    Project(i64),
    Claude,
    Templates,
    Board,
    Reports,
}

pub struct App {
    pub store: Store,
    pub db_path: PathBuf,
    pub projects: Vec<Project>,
    pub groups: Vec<Group>,
    pub purposes: Vec<Purpose>,
    dock: DockState<Tab>,
    /// Tabs to open after the dock is drawn (the dock is borrowed while its tabs draw).
    pub open: Vec<Tab>,
    pub overview: overview::State,
    pub import: import::State,
    pub settings: settings::State,
    pub project: project::State,
    pub todos_page: todos_page::State,
    pub restyle: Option<restyle::Restyle>,
    pub capture: Option<capture::Capture>,
    pub palette: Option<palette::Palette>,
    pub progress: progress_ui::State,
    pub dashboard: dashboard_ui::State,
    pub claude: claude_ui::State,
    pub templates: templates_ui::State,
    pub new_project: Option<templates_ui::NewProject>,
    refresh_tx: Sender<(i64, Info)>,
    refresh_rx: Receiver<(i64, Info)>,
    pub refreshing: usize,
    /// When the last refresh finished (footer: "checked 4 min ago").
    checked: Option<Instant>,
    pub message: Option<String>,
    pub data_dir: PathBuf,
    /// `PRAGMA data_version` when last checked: it changes when another connection (the MCP server) writes.
    data_version: (i64, Instant),
    /// Finished background work, applied on the UI thread ([`App::background`]).
    jobs: (Sender<Job>, Receiver<Job>),
    /// Task runs per project; they outlive the project's tab (M4).
    pub runs: tasks_ui::Runs,
    pub git: git_ui::State,
    pub deps: deps_ui::State,
}

/// Work finished on a background thread, applied to the app on the UI thread.
pub type Job = Box<dyn FnOnce(&mut App) + Send>;

impl App {
    fn new(ctx: &egui::Context, data_dir: PathBuf, open: Option<String>, section: Option<String>, page: Option<String>) -> App {
        let db_path = data_dir.join("pa.db");
        theme::install_fonts(ctx);
        let store = Store::open(&db_path).unwrap_or_else(|e| panic!("cannot open {}: {e}", db_path.display()));
        let (refresh_tx, refresh_rx) = channel();
        let mut app = App {
            store,
            db_path,
            projects: Vec::new(),
            groups: Vec::new(),
            purposes: Vec::new(),
            dock: DockState::new(vec![Tab::Overview]),
            open: Vec::new(),
            overview: Default::default(),
            import: Default::default(),
            settings: Default::default(),
            project: Default::default(),
            todos_page: Default::default(),
            restyle: None,
            capture: None,
            palette: None,
            progress: Default::default(),
            dashboard: Default::default(),
            claude: Default::default(),
            templates: Default::default(),
            new_project: None,
            refresh_tx,
            refresh_rx,
            refreshing: 0,
            checked: None,
            message: None,
            data_dir,
            data_version: (0, Instant::now()),
            jobs: channel(),
            runs: Default::default(),
            git: Default::default(),
            deps: Default::default(),
        };
        app.data_version.0 = app.store.data_version().unwrap_or(0);
        theme::apply(ctx, &app.setting("theme", "system"));
        let view = app.setting("overview.view", "table");
        app.overview.set_view(&view);
        let favourites = app.setting(overview::FAVOURITES, "");
        app.overview.set_favourites(&favourites);
        app.reload();
        match page.as_deref() {
            Some("todos") => app.open.push(Tab::Todos),
            Some("import") => app.open.push(Tab::Import),
            Some("settings") => app.open.push(Tab::Settings),
            Some("capture") => capture::open(&mut app, None),
            Some("palette") => palette::open(&mut app),
            Some("claude") => app.open.push(Tab::Claude),
            Some("claude-matrix") => {
                app.claude.show_matrix();
                app.open.push(Tab::Claude);
            }
            Some("templates") => app.open.push(Tab::Templates),
            Some("board") => app.open.push(Tab::Board),
            Some("reports") => app.open.push(Tab::Reports),
            Some("new-project") => templates_ui::open_new(&mut app, None),
            _ => {}
        }
        if let Some(name) = open {
            match app.projects.iter().find(|p| p.name.eq_ignore_ascii_case(&name)) {
                Some(p) => {
                    app.open.push(Tab::Project(p.id));
                    if let Some(s) = section {
                        app.project.set_section(p.id, &s);
                    }
                }
                None => app.message = Some(format!("--open: no project named {name}")),
            }
        }
        // Cached info shows at once; fresh info replaces it as it comes in (SPEC § 5).
        let all: Vec<(i64, String)> = app.projects.iter().flat_map(|p| &p.checkouts).map(|c| (c.id, c.path.clone())).collect();
        app.refresh(ctx, all);
        deps_ui::recheck_all(&mut app, ctx);
        app
    }

    /// Run `work` on a background thread; the job it returns is applied to the app on the UI thread.
    pub fn background(&self, ctx: &egui::Context, work: impl FnOnce() -> Job + Send + 'static) {
        let (tx, ctx) = (self.jobs.0.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(job) = self.jobs.1.try_recv() {
            job(self);
        }
    }

    /// Re-read projects, groups and purposes from the store.
    pub fn reload(&mut self) {
        let r = self.store.projects().and_then(|p| Ok((p, self.store.groups()?, self.store.purposes()?)));
        if let Some((p, g, u)) = self.ok(r) {
            (self.projects, self.groups, self.purposes) = (p, g, u);
        }
    }

    /// The value of a store call, or `None` with the error shown in the status bar.
    pub fn ok<T, E: std::fmt::Display>(&mut self, r: Result<T, E>) -> Option<T> {
        r.map_err(|e| self.message = Some(format!("Error: {e}"))).ok()
    }

    /// Collect info for these checkouts (id, path) on background threads (mostly waiting on `git`, so one per core).
    pub fn refresh(&mut self, ctx: &egui::Context, jobs: Vec<(i64, String)>) {
        if jobs.is_empty() {
            return;
        }
        self.refreshing += jobs.len();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(jobs.len());
        let jobs = Arc::new(Mutex::new(jobs));
        for _ in 0..threads {
            let (jobs, tx, ctx) = (jobs.clone(), self.refresh_tx.clone(), ctx.clone());
            // `loop` + `let else`, not `while let`: the queue lock must be released before the slow `collect`.
            #[allow(clippy::while_let_loop)]
            std::thread::spawn(move || {
                loop {
                    let Some((id, path)) = jobs.lock().ok().and_then(|mut j| j.pop()) else { break };
                    let _ = tx.send((id, info::collect(Path::new(&path))));
                    ctx.request_repaint();
                }
            });
        }
    }

    /// Refresh every checkout and re-check the dependencies (sidebar button, F5, palette).
    pub fn refresh_all(&mut self, ctx: &egui::Context) {
        let all = self.projects.iter().flat_map(|p| &p.checkouts).map(|c| (c.id, c.path.clone())).collect();
        self.refresh(ctx, all);
        deps_ui::recheck_all(self, ctx);
    }

    pub fn refresh_project(&mut self, ctx: &egui::Context, project_id: i64) {
        let jobs = self.projects.iter().filter(|p| p.id == project_id).flat_map(|p| &p.checkouts).map(|c| (c.id, c.path.clone())).collect();
        self.refresh(ctx, jobs);
    }

    /// Reload when the database was changed outside the app (checked once a second).
    fn poll_outside_changes(&mut self, ctx: &egui::Context) {
        if self.data_version.1.elapsed().as_secs_f32() < 1.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
            return;
        }
        let v = self.store.data_version().unwrap_or(self.data_version.0);
        if v != self.data_version.0 {
            self.reload();
            self.project.forget_notes();
            self.runs.forget_tasks();
            self.deps.forget();
        }
        self.data_version = (v, Instant::now());
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }

    fn poll_refresh(&mut self) {
        while let Ok((id, info)) = self.refresh_rx.try_recv() {
            self.refreshing = self.refreshing.saturating_sub(1);
            if self.refreshing == 0 {
                self.checked = Some(Instant::now());
            }
            let r = self.store.set_info(id, &info);
            self.ok(r);
            if let Some(c) = self.projects.iter_mut().flat_map(|p| &mut p.checkouts).find(|c| c.id == id) {
                c.info = Some(info);
                c.info_at = Some(pa_core::now_secs());
            }
        }
    }

    pub fn setting(&self, key: &str, default: &str) -> String {
        self.store.setting(key).ok().flatten().unwrap_or_else(|| default.to_string())
    }

    pub fn launch(&mut self, key: &str, default: &str, path: &str) {
        let template = self.setting(key, default);
        if let Err(e) = pa_core::launch(&template, path) {
            self.message = Some(format!("Could not run `{template}`: {e}"));
        }
    }

    pub fn group_name(&self, id: Option<i64>) -> &str {
        id.and_then(|id| self.groups.iter().find(|g| g.id == id)).map_or("", |g| g.name.as_str())
    }

    pub fn purpose(&self, id: Option<i64>) -> Option<&Purpose> {
        id.and_then(|id| self.purposes.iter().find(|p| p.id == id))
    }

    /// Save an edited copy's name, group, purpose, status and tags when they changed. Only those fields are taken
    /// over: the copy's checkouts may be older than the ones in memory (a refresh or reload since it was cloned).
    pub fn save_project(&mut self, p: Project) {
        let fields = |q: &Project| {
            (q.name.clone(), q.group_id, q.purpose_id, q.status, q.tags.clone(), q.todo_path.clone(), q.roadmap_path.clone())
        };
        let Some(i) = self.projects.iter().position(|q| q.id == p.id) else { return };
        if fields(&self.projects[i]) != fields(&p) {
            let r = self.store.update_project(&p);
            if self.ok(r).is_some() {
                let q = &mut self.projects[i];
                (q.name, q.group_id, q.purpose_id, q.status, q.tags, q.todo_path, q.roadmap_path) = fields(&p);
            }
        }
    }

    /// Show `tab` (opened if needed) and focus its leaf, so `active_tab` knows it before any click.
    fn open_tab(&mut self, tab: Tab) {
        if self.dock.find_tab(&tab).is_none() {
            self.dock.push_to_focused_leaf(tab);
        }
        if let Some(path) = self.dock.find_tab(&tab) {
            let _ = self.dock.set_active_tab(path);
            self.dock.set_focused_node_and_surface(path.node_path());
        }
    }

    fn active_tab(&mut self) -> Option<Tab> {
        self.dock.find_active_focused().map(|(_, t)| *t)
    }

    /// Left navigation (DockHand style): brand, pages, groups, and the theme/refresh/settings controls at the bottom.
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui.ctx());
        let active = self.active_tab();
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new("Project").font(theme::semibold(17.0)));
            ui.label(RichText::new("Architect").font(theme::semibold(17.0)).color(p.blue));
        });
        ui.add_space(14.0);
        let w = ui.available_width();
        if theme::button(ui, &format!("{}  Jump to…\tCtrl+K", icon::MAGNIFYING_GLASS), egui::FontId::proportional(13.5), 0.0, theme::Kind::Secondary, egui::vec2(w, 32.0))
            .on_hover_text("Command palette: projects, sections, pages and actions")
            .clicked()
        {
            palette::open(self);
        }
        ui.add_space(14.0);
        let nav = |ui: &mut egui::Ui, glyph: &str, label: &str, on: bool| theme::nav(ui, format!("{glyph}   {label}"), on).clicked();
        let all_on = active == Some(Tab::Overview) && self.overview.group_filter().is_none();
        if nav(ui, icon::SQUARES_FOUR, "Projects", all_on) {
            self.overview.set_group(None);
            self.open.push(Tab::Overview);
        }
        let open = todos_page::open_total(self);
        let label = if open > 0 { format!("TODOs	{open}") } else { "TODOs".into() };
        if nav(ui, icon::LIST_CHECKS, &label, active == Some(Tab::Todos)) {
            self.open.push(Tab::Todos);
        }
        if nav(ui, icon::KANBAN, "Board", active == Some(Tab::Board)) {
            self.open.push(Tab::Board);
        }
        if nav(ui, icon::CHART_LINE, "Reports", active == Some(Tab::Reports)) {
            self.open.push(Tab::Reports);
        }
        if nav(ui, icon::ROBOT, "Claude", active == Some(Tab::Claude)) {
            self.open.push(Tab::Claude);
        }
        if nav(ui, icon::STACK, "Templates", active == Some(Tab::Templates)) {
            self.open.push(Tab::Templates);
        }
        // Favourites: click opens, drag reorders; local changes show as an orange "N changed" on the right.
        let favourites: Vec<(i64, String, u32)> = self
            .overview
            .favourites()
            .iter()
            .filter_map(|id| self.projects.iter().find(|q| q.id == *id))
            .map(|q| (q.id, q.name.clone(), q.info().and_then(|i| i.git.as_ref()).map_or(0, |g| g.dirty)))
            .collect();
        if !favourites.is_empty() {
            ui.add_space(18.0);
            ui.label(caps("   Favourites", p.faint));
            ui.add_space(2.0);
            let mut moved = None;
            for (k, (id, name, dirty)) in favourites.into_iter().enumerate() {
                // ponytail: cut by characters, not measured width; the sidebar is a fixed 220 px.
                let max = if dirty > 0 { 14 } else { 20 };
                let short = if name.chars().count() > max { format!("{}…", name.chars().take(max - 1).collect::<String>()) } else { name };
                // One widget that senses both click and drag (as the Overview's favourite tiles did): a drag area laid
                // over the button (dnd_drag_source) took its clicks.
                let r = theme::nav_draggable(ui, format!("{}   {short}", icon::STAR), active == Some(Tab::Project(id)));
                if r.drag_started() {
                    egui::DragAndDrop::set_payload(ui.ctx(), overview::FavDrag(k));
                }
                if r.clicked() {
                    self.open.push(Tab::Project(id));
                }
                if dirty > 0 {
                    let pos = egui::pos2(r.rect.right() - 12.0, r.rect.center().y);
                    ui.painter().text(pos, egui::Align2::RIGHT_CENTER, format!("{dirty} changed"), egui::FontId::proportional(12.0), p.orange);
                }
                if let Some(d) = r.dnd_release_payload::<overview::FavDrag>() {
                    moved = Some((d.0, k));
                }
            }
            if let Some((from, to)) = moved {
                overview::move_favourite(self, from, to);
            }
        }
        let searches = todos_page::sidebar_searches(self);
        if !searches.is_empty() {
            ui.add_space(18.0);
            ui.label(caps("   Saved searches", p.faint));
            ui.add_space(2.0);
            for (name, query, n) in searches {
                let on = active == Some(Tab::Todos) && todos_page::showing(self, &query);
                if nav(ui, icon::BOOKMARK_SIMPLE, &format!("{name}\t{n}"), on) {
                    todos_page::apply(self, &query);
                }
            }
        }
        if !self.groups.is_empty() {
            ui.add_space(18.0);
            ui.label(caps("   Groups", p.faint));
            ui.add_space(2.0);
            for g in self.groups.clone() {
                let n = self.projects.iter().filter(|q| q.group_id == Some(g.id)).count();
                let on = active == Some(Tab::Overview) && self.overview.group_filter() == Some(Some(g.id));
                if nav(ui, icon::FOLDERS, &format!("{}  ·  {n}", g.name), on) {
                    self.overview.set_group(Some(Some(g.id)));
                    self.open.push(Tab::Overview);
                }
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let pref = self.setting("theme", "system");
                let (glyph, next, tip) = match pref.as_str() {
                    "dark" => (icon::MOON, "light", "Dark theme (click: light)"),
                    "light" => (icon::SUN, "system", "Light theme (click: follow system)"),
                    _ => (icon::CIRCLE_HALF, "dark", "Follows the system theme (click: dark)"),
                };
                if theme::icon_button(ui, glyph, tip).clicked() {
                    let r = self.store.set_setting("theme", next);
                    self.ok(r);
                    theme::apply(ui.ctx(), next);
                }
                if self.refreshing > 0 {
                    ui.add(egui::Spinner::new().size(16.0));
                } else if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh all projects (F5)").clicked() {
                    self.refresh_all(ui.ctx());
                }
                // Selected look (as a nav entry) while the Settings page is in front.
                let kind = if active == Some(Tab::Settings) { theme::Kind::Nav(true) } else { theme::Kind::Ghost };
                if theme::button(ui, icon::GEAR_SIX, egui::FontId::proportional(16.0), 0.0, kind, egui::vec2(28.0, 28.0)).on_hover_text("Settings").clicked() {
                    self.open.push(Tab::Settings);
                }
            });
            ui.add_space(6.0);
            let w = ui.available_width();
            if theme::button(ui, &format!("{}  Capture", icon::LIGHTBULB), egui::FontId::proportional(14.0), 0.0, theme::Kind::Secondary, egui::vec2(w, 34.0))
                .on_hover_text("Jot down an idea, bug, TODO or note (Ctrl+Shift+K)")
                .clicked()
            {
                capture::open(self, active);
            }
        });
    }

    /// Footer in monospace caps (ArtCraft style): what was checked when, and messages.
    fn footer(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui.ctx());
        ui.horizontal(|ui| {
            ui.set_height(26.0);
            let changed = self.projects.iter().filter(|q| q.info().and_then(|i| i.git.as_ref()).is_some_and(|g| g.dirty > 0)).count();
            let checked = if self.refreshing > 0 {
                format!("checking {} folders…", self.refreshing)
            } else {
                match self.checked {
                    Some(t) if t.elapsed().as_secs() < 60 => "checked just now".into(),
                    Some(t) => format!("checked {} min ago", t.elapsed().as_secs() / 60),
                    None => "not checked yet".into(),
                }
            };
            // "Shown": what the Projects page lists under All (purposes hidden from the overview left out).
            let shown = self.projects.iter().filter(|q| self.purpose(q.purpose_id).is_none_or(|u| u.show_in_overview)).count();
            ui.label(caps(format!("{checked}  ·  {} tracked, {shown} shown  ·  {changed} with local changes", self.projects.len()), p.faint));
            if let Some(m) = self.message.clone() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::X, "Dismiss").clicked() {
                        self.message = None;
                    }
                    ui.label(RichText::new(m).size(12.0).color(p.orange));
                });
            }
        });
    }
}

struct Tabs<'a> {
    app: &'a mut App,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        match tab {
            Tab::Overview => format!("{}  Projects", icon::SQUARES_FOUR).into(),
            Tab::Todos => format!("{}  TODOs", icon::LIST_CHECKS).into(),
            Tab::Import => format!("{}  Import", icon::DOWNLOAD_SIMPLE).into(),
            Tab::Settings => format!("{}  Settings", icon::GEAR_SIX).into(),
            Tab::Project(id) => {
                let name = self.app.projects.iter().find(|p| p.id == *id).map_or("(removed)", |p| p.name.as_str());
                format!("{}  {name}", icon::FOLDER_OPEN).into()
            }
            Tab::Claude => format!("{}  Claude", icon::ROBOT).into(),
            Tab::Templates => format!("{}  Templates", icon::STACK).into(),
            Tab::Board => format!("{}  Board", icon::KANBAN).into(),
            Tab::Reports => format!("{}  Reports", icon::CHART_LINE).into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match *tab {
            Tab::Overview => overview::show(self.app, ui),
            Tab::Todos => todos_page::show(self.app, ui),
            Tab::Import => import::show(self.app, ui),
            Tab::Settings => settings::show(self.app, ui),
            Tab::Project(id) => project::show(self.app, ui, id),
            Tab::Claude => claude_ui::page(self.app, ui),
            Tab::Templates => templates_ui::page(self.app, ui),
            Tab::Board => board_page::show(self.app, ui),
            Tab::Reports => progress_ui::page(self.app, ui),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Overview
    }

    fn on_close(&mut self, _tab: &mut Tab) -> OnCloseResponse {
        OnCloseResponse::Close
    }

    fn allowed_in_windows(&self, _tab: &mut Tab) -> bool {
        false
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_refresh();
        self.poll_jobs();
        self.poll_outside_changes(&ui.ctx().clone());
        import::poll(self);
        let p = pal(ui.ctx());
        egui::Panel::left("sidebar")
            .exact_size(220.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(10, 0)).stroke(egui::Stroke::new(1.0, p.border)))
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(16, 2)).stroke(egui::Stroke::new(1.0, p.border)))
            .show(ui, |ui| self.footer(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(p.bg)).show(ui, |ui| {
            let mut dock = std::mem::replace(&mut self.dock, DockState::new(Vec::new()));
            DockArea::new(&mut dock)
                .id(Id::new("dock"))
                .style(dock_style(ui, p))
                .show_add_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, &mut Tabs { app: self });
            self.dock = dock;
        });
        restyle::window(self, &ui.ctx().clone());
        capture::window(self, &ui.ctx().clone());
        palette::window(self, &ui.ctx().clone());
        git_ui::window(self, &ui.ctx().clone());
        claude_ui::window(self, &ui.ctx().clone());
        templates_ui::window(self, &ui.ctx().clone());
        for tab in std::mem::take(&mut self.open) {
            self.open_tab(tab);
        }
        // Tabs of projects removed from the app close themselves.
        let ids: Vec<i64> = self.projects.iter().map(|p| p.id).collect();
        self.dock.retain_tabs(|t| !matches!(t, Tab::Project(id) if !ids.contains(id)));
    }
}

/// Tabs as a quiet strip on the page background; the active one raised, bodies with generous margins.
fn dock_style(ui: &egui::Ui, p: theme::Palette) -> egui_dock::Style {
    let mut st = egui_dock::Style::from_egui(ui.style().as_ref());
    st.main_surface_border_stroke = egui::Stroke::NONE;
    st.tab_bar.bg_fill = p.bg;
    st.tab_bar.height = 38.0;
    st.tab_bar.hline_color = p.border;
    st.tab_bar.inner_margin = egui::Margin::symmetric(12, 0);
    st.tab.spacing = 4.0;
    st.tab.hline_below_active_tab_name = false;
    st.tab.minimum_width = Some(120.0);
    for (s, fill, text) in [
        (&mut st.tab.active, p.surface, p.text),
        (&mut st.tab.focused, p.surface, p.text),
        (&mut st.tab.active_with_kb_focus, p.surface, p.text),
        (&mut st.tab.focused_with_kb_focus, p.surface, p.text),
        (&mut st.tab.inactive, p.bg, p.dim),
        (&mut st.tab.inactive_with_kb_focus, p.bg, p.dim),
        (&mut st.tab.hovered, p.hover, p.text),
    ] {
        s.bg_fill = fill;
        s.text_color = text;
        s.outline_color = p.border;
        s.corner_radius = egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 };
    }
    st.tab.tab_body.bg_fill = p.bg;
    st.tab.tab_body.stroke = egui::Stroke::NONE;
    st.tab.tab_body.inner_margin = egui::Margin::symmetric(24, 18);
    st.buttons.close_tab_bg_fill = p.hover;
    st.buttons.close_tab_color = p.faint;
    st.buttons.close_tab_active_color = p.text;
    st
}

fn main() -> eframe::Result {
    pa_core::install::remove_old_self();
    let mut args = std::env::args().skip(1);
    let (mut data_dir, mut open, mut section, mut page) = (pa_core::default_data_dir(), None, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-dir" => data_dir = args.next().map(PathBuf::from).expect("--data-dir needs a folder"),
            "--open" => open = args.next(),
            "--section" => section = args.next(),
            "--page" => page = args.next(),
            other => panic!("unknown argument {other} (--data-dir <folder>, --open <project name>, --section <todos|roadmap|code|notes|checkouts|settings|git|github|tasks|deps|claude>, --page <todos|board|reports|import|settings|capture|palette|claude|claude-matrix|templates|new-project>)"),
        }
    }
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ProjectArchitect")
            // Wayland finds the icon through the .desktop file of this id (packaging/projectarchitect.desktop).
            .with_app_id("projectarchitect")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(egui::IconData { rgba: include_bytes!("../../../assets/icon-64.rgba").to_vec(), width: 64, height: 64 }),
        ..Default::default()
    };
    // DX12 on Windows (Vulkan flickers on some drivers); WGPU_BACKEND overrides.
    if cfg!(windows)
        && std::env::var_os("WGPU_BACKEND").is_none()
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
    {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
    // egui_wgpu defaults to HighPerformance: the discrete GPU (or eGPU), which wakes it, invites the NVIDIA/Discord
    // overlays and crashes the app when an eGPU is unplugged. A flat UI needs the integrated GPU; WGPU_POWER_PREF overrides.
    if std::env::var_os("WGPU_POWER_PREF").is_none()
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
    {
        setup.power_preference = eframe::wgpu::PowerPreference::LowPower;
    }
    eframe::run_native("ProjectArchitect", options, Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, data_dir, open, section, page)))))
}

/// Small shared widgets.
pub mod widgets {
    use pa_core::{Group, Purpose, Status};

    /// Group picker; `None` = no group.
    pub fn group_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, groups: &[Group], value: &mut Option<i64>) -> bool {
        let text = value.and_then(|v| groups.iter().find(|g| g.id == v)).map_or("—", |g| g.name.as_str()).to_string();
        let mut changed = false;
        crate::theme::select(ui, id, 160.0, text, |ui| {
            changed |= ui.selectable_value(value, None, "—").changed();
            for g in groups {
                changed |= ui.selectable_value(value, Some(g.id), &g.name).changed();
            }
        });
        changed
    }

    /// Purpose picker; `None` = no purpose.
    pub fn purpose_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, purposes: &[Purpose], value: &mut Option<i64>) -> bool {
        let text = value.and_then(|v| purposes.iter().find(|p| p.id == v)).map_or("—", |p| p.name.as_str()).to_string();
        let mut changed = false;
        crate::theme::select(ui, id, 160.0, text, |ui| {
            changed |= ui.selectable_value(value, None, "—").changed();
            for p in purposes {
                changed |= ui.selectable_value(value, Some(p.id), &p.name).changed();
            }
        });
        changed
    }

    pub fn status_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut Status) -> bool {
        let mut changed = false;
        crate::theme::select(ui, id, 160.0, value.as_str(), |ui| {
            for s in Status::ALL {
                changed |= ui.selectable_value(value, s, s.as_str()).changed();
            }
        });
        changed
    }

    /// "↑2 ↓5", or "" when even or without an upstream.
    pub fn ahead_behind(ab: Option<(u32, u32)>) -> String {
        match ab {
            Some((0, 0)) | None => String::new(),
            Some((a, 0)) => format!("↑{a}"),
            Some((0, b)) => format!("↓{b}"),
            Some((a, b)) => format!("↑{a} ↓{b}"),
        }
    }

    /// A version as shown everywhere: "v1.2.3" whether it came from a manifest ("1.2.3") or a tag ("v1.2.3").
    pub fn version(v: &str) -> String {
        if v.starts_with(|c: char| c.is_ascii_digit()) { format!("v{v}") } else { v.to_string() }
    }

    #[cfg(test)]
    #[test]
    fn version_has_one_v() {
        assert_eq!(version("0.1.1"), "v0.1.1");
        assert_eq!(version("v0.6.0"), "v0.6.0");
        assert_eq!(version("nightly"), "nightly");
    }
}
