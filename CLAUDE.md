# CLAUDE.md

ProjectArchitect: a local desktop app (Rust, egui/eframe 0.36, egui_dock, rusqlite) that tracks all my coding
projects wherever they are on disk. The spec is [SPEC.md](SPEC.md); decisions are its § 12 (D-n). Milestones are in
[ROADMAP.md](ROADMAP.md), open work is in [TODO.md](TODO.md) (both in the app's own formats, SPEC § 7.2 / § 7.4).
What comes next and why: [docs/briefs/next.md](docs/briefs/next.md).

## Layout

- `crates/core` (`pa-core`), no UI:
  - `db` store + migrations, `scan` discovery, `info` per-checkout info (git, languages, version, description,
    TODO counts), `loc` lines of code (tokei), `git` branch commits / push / PR and the Git panel (snapshot, fetch,
    pull, push, switch, worktree) via the CLI;
  - `github` `gh` wrappers (info, issues, repo create), `(#n)` issue links on TODO lines, captures → issues;
    `tasks` stored commands, manifest suggestions, `Run` (live output, tree-kill stop); `deps` dependencies
    between projects (used / latest version, comparison);
  - `todo` TODO files in per-project styles (`Style`: checkbox / bullet / numbered / line; done = tick / move to a
    section / delete), lossless edits, `save` (re-read check), `backup`, `convert_to`;
  - `roadmap` versions + milestones + plain sections, board lanes, release suggestions, `convert_roadmap` (tables);
  - `query` the TODOs query (`project:`, `in:`, `is:`, `kind:`, `source:`, `has:issue`, `-`), completions;
    `history` items over a file's git history, roadmap entries' progress, commits / Claude sessions per week;
  - `codetodo` TODO/FIXME comments; `ops` operations shared by the GUI and the MCP server (project lookup,
    same-repository rule, TODO/roadmap file paths and edits, style changes, notes, quick capture, git);
  - `claude` Claude Code config: inventory (project / global `~/.claude`), plans for whole / section / JSON-key copies
    and edits (diff, conflicts), `apply` (backup + `todo::save`), `classify` (what may be written at all);
  - `template` project templates: `template.toml`, placeholders, `plan` / `run` (streamed shell steps) / `finish`
    (track), seeds, duplicate, make from a project; `install` replacing a running binary under its fixed path
    (set aside as `.old`, removed at the next start);
    `update` the app's own GitHub releases via `gh` (latest, version compare, download, msiexec; D-49).
- `apps/pa`: the GUI. `main.rs` shell (sidebar, dock, footer, background refresh, outside-change reload),
  `overview.rs` table/grid, `project.rs` project tab, `dashboard_ui.rs` its Overview as a dock of widgets, `roadmap_ui.rs` roadmap views, `todos_page.rs` TODOs page,
  `restyle.rs` Change style / roadmap conversion dialog, `capture.rs` Quick capture modal (Ctrl+Shift+K),
  `claude_ui.rs` Claude section + Claude page (editor, settings form, copy and review windows, matrix),
  `templates_ui.rs` Templates page + New Project dialog, `import.rs`, `settings.rs` (also Updates: check / install / relaunch), `theme.rs` look + widgets;
  `git_ui.rs` Git and GitHub sections + Create repo dialog, `tasks_ui.rs` Tasks section and the app's task runs,
  `deps_ui.rs` Dependencies section, `palette.rs` command palette (Ctrl+K, F5), `board_page.rs` cross-project
  board, `progress_ui.rs` Progress section + Reports page, `chart.rs` painted line / bar / spark charts. Background work: `App::background(ctx, work)` runs `work` on a thread and
  applies the job it returns on the UI thread.
- `apps/pa-mcp`: the stdio MCP server (JSON-RPC over stdin/stdout, logs on stderr only). Tools are a table in
  `apps/pa-mcp/src/tools.rs` (library `pa_mcp`, also used by `pa-cli`); shared logic lives in `pa_core::ops`. Publishing (push, PR, GitHub repos and issues)
  needs the setting `mcp.allow_publish` = true. Registered with Claude Code (user scope) as
  `%LOCALAPPDATA%\Programs\ProjectArchitect\pa-mcp.exe`, a copy of the release build: Claude Code keeps it running,
  which would lock `target\release\pa-mcp.exe` on Windows. After changing pa-mcp:
  `cargo build --release -p pa-mcp -p pa`, then `target\release\pa-cli install-mcp` (works while sessions hold the
  old copy, which they keep until they restart).
- `apps/pa/src/bin/pa-cli.rs`: the command line, a console binary (D-47): `pa-cli tools [<tool>]`,
  `pa-cli <tool> key=value …`; every MCP tool is a command.
- Packaging (D-48): `wix/main.wxs` (MSI), `[package.metadata.generate-rpm]` in `apps/pa/Cargo.toml` and
  `packaging/projectarchitect.desktop` (RPM), `assets/` icons drawn by `tools/icon.py`. `.github/workflows/ci.yml`
  (Windows + Fedora tests), `release.yml` (installers as artifacts; a GitHub release on `v*` tags, D-49). Linux-only code paths: `cfg(not(windows))` in
  `pa_core` (launch, tasks, templates, data dir), `theme::font_files`, `LAUNCHERS`.

## Rules

- Never modify, move or delete anything inside a tracked project, except where a feature says so (TODO.md,
  ROADMAP.md, Claude config) and then losslessly and atomically after a re-read check (`todo::save`); conversions
  and config overwrites keep a backup first (`todo::backup`, `%APPDATA%\ProjectArchitect\backups`).
- Never write the user's other repositories while developing or testing; use copies (scratch dirs) of their files.
- UI look: `apps/pa/src/theme.rs` (palettes, caps labels, chips, tags, cards, segmented control, `button` kinds with
  animated hover, `field` for text inputs); reuse it, no ad-hoc colours or plain `egui::Button`s.
- Design file: `design/app.figcraft` (FigCraft). `Theme` variables (Light/Dark) mirror `theme::LIGHT` / `DARK`; keep
  them in sync when either changes. Pages: Components (theme.rs widgets) and Screens: every page, the project tab's
  sections and roadmap views, and the Capture / palette / New Project dialogs, each Light and Dark (a frame's Theme
  mode). Two empty shells (app, project tab) are the starting point for new screens. PNGs in `design/exports/`.
- App data: `%APPDATA%\ProjectArchitect` (`pa.db`); `--data-dir <dir>` overrides it. Tests use temp dirs or
  in-memory databases, never the real app data.
- Git and GitHub only through the `git` and `gh` CLIs (`pa_core::cmd`, which hides console windows); the app never
  stores tokens.
- Schema changes: a new step in `db::MIGRATIONS`, never an edit to an existing step (5 steps so far).

## Settings keys (table `settings`)

`theme` (system/dark/light), `overview.view` (table/grid), `projects.favourites` (favourite project ids in order), `todos.page` (TODOs page filters, `k=v;…`),
`todos.saved` (saved searches, `name<TAB>query` lines), `board.wip_now` (WIP limit of the Board's Now lane, 0 = off),
`roadmap.view.<project id>` (board/timeline/list/gantt), `dashboard.<project id>` / `dashboard.purpose.<purpose id>`
(Overview widget layout, `egui_dock` JSON; empty = none), `mcp.allow_publish` (true/false), `capture.issues.<project id>`
(true/false: captures become GitHub issues), `mcp.allow_claude_write` (true/false: MCP may write Claude config),
`claude.home` (folder holding the global `.claude`; default the user's home), `templates.dir` (default
`<data dir>\templates`), `templates.tracked` (templates folder tracked once), `templates.parent` (last New Project
folder), `launch.code`, `launch.explorer`, `launch.terminal`, `launch.claude` (`{path}` = checkout folder). Per-project columns:
`todo_path`, `roadmap_path`, `todo_style` (encoded `Style`), `tag_prefix` (None = "v").

## Checks

```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p pa -- --data-dir target/pa-data
cargo run -p pa --bin pa-cli -- --data-dir target/pa-data tools
```

Linux from Windows (Docker; Git Bash needs `MSYS_NO_PATHCONV=1`): `docker run --rm -v "$(pwd -W):/src" -v pa-target:/target
-e CARGO_TARGET_DIR=/target fedora:latest` with `dnf install gcc git fontconfig`, rustup, then `cargo test --workspace`.
The GUI runs there under `Xvfb` with `mesa-vulkan-drivers` (screenshot with ImageMagick `import -window root`).

UI checks without clicking: start options `--open <project> --section <todos|roadmap|code|notes|checkouts|settings|git|github|tasks|deps|claude>`
and `--page <todos|board|reports|import|settings|capture|palette|claude|claude-matrix|templates|new-project>`, then `tools/shot.ps1` (PowerShell; DPI-aware window capture) on a COPY of
`%APPDATA%\ProjectArchitect` as `--data-dir`. Settings such as `theme` or `roadmap.view.<id>` can be preset in the
copy's `settings` table (Python `sqlite3`) to capture a given view; point `claude.home` at a scratch folder before
trying Claude config writes.

## egui 0.36 notes (things that bit)

- `eframe::App::ui(&mut self, ui, frame)`, panels are `egui::Panel::top/left/bottom(..).show(ui, ..)`.
- `TextEdit::frame` takes an `egui::Frame` (`Frame::NONE`), not a bool; inside a `Grid` a `TextEdit` collapses to
  ~40 px unless it has `min_size` (use `theme::field`).
- `ctx.style()` is gone: `ctx.global_style()`, `ctx.theme()`.
- Text painted with a colour chosen at paint time needs `Color32::PLACEHOLDER` in its `RichText`, or the theme's
  `override_text_color` is baked into the galley (invisible text on inverted buttons).
- Every widget in a `Grid` is its own cell: wrap multi-widget cells in `ui.horizontal`.
- Widget ids from content (titles) collide when a file repeats a heading: add the index.
- `ui.horizontal` inherits a right-to-left parent's direction: `theme::segmented` there shows its segments reversed.
- Toolbar rows (left content, buttons on the right): `ui.horizontal_wrapped` + `theme::right`, never `ui.horizontal` +
  `with_layout(right_to_left)`, which draws the buttons over the left side in a narrow pane. In a wrapping layout
  `available_width()` is the whole row; the space left on the row is `available_size_before_wrap().x`.
