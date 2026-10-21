# ProjectArchitect — specification (draft 2, 2026-10-16)

A local desktop app that tracks all of my coding projects, wherever they live on disk, and lets me
(and an AI agent over MCP) look at, organise and work on them. Spec only; nothing is implemented.

Decisions marked **(D-n)** were made in the planning session of 2026-10-16; the list is in § 12.

## 1. Goals and non-goals

**Goals**

- One overview of every project: name, group, purpose, status, languages, version, git state, GitHub link.
- A tab per opened project with TODOs, roadmap, code TODOs, git, tasks, notes and Claude config.
- Create projects from templates that I can edit and add to.
- Manage Claude Code configuration across projects and copy files, or parts of files, between them.
- A local MCP server through which an agent can do everything the UI can do (D-4).
- Lightweight: one binary, starts fast, works offline (only GitHub features need the network).

**Non-goals (for now)**

- No code editor, diff viewer or merge tool. The app launches the tools I already use.
- No multi-user, sync or cloud features. Everything stays on one machine.
- No macOS build. Linux (Fedora) arrives with M7 (§ 11.1); until then avoid Windows-only APIs where it costs nothing.

## 2. Stack

Same as my other egui apps:

- Rust 2024 workspace; `eframe`/`egui` 0.36 (wgpu), `egui_dock` for project tabs, `egui-phosphor` icons,
  `egui_commonmark` for markdown, `rfd` for folder pickers, `serde` + `toml`/`serde_json`.
- `rusqlite` (bundled) for the store, in WAL mode so the GUI and the MCP binary can share it.
- Git and GitHub through the **`git` and `gh` CLIs** (both installed) instead of libgit2/gix and our own
  token handling: `gh` already holds my login, so the app never stores a token.
- MCP: a stdio JSON-RPC server. Use the `rmcp` crate if it fits; otherwise hand-roll the small subset
  (initialize, tools/list, tools/call).

Workspace layout:

```
crates/core     # store (SQLite), scanner, parsers (TODO/roadmap/code comments), git/gh wrappers,
                #   templates, Claude config ops. No UI. Every operation the UI or MCP does lives here.
apps/pa         # GUI (egui); also the pa-cli console binary (src/bin/pa-cli.rs)
apps/pa-mcp     # stdio MCP server; a thin layer over core; its tool table is a library shared with pa-cli
```

Binaries: `pa.exe` (GUI), `pa-mcp.exe` (MCP server) (D-22) and `pa-cli.exe` (command line, D-47).

App data: `%APPDATA%\ProjectArchitect\` → `pa.db`, `templates\` (default location, § 9), `backups\`, `logs\`.

## 3. Concepts

| Concept | Meaning |
|---|---|
| **Group** | A named container for projects, optionally tied to a folder (e.g. `Code/Hobby`, which is not a git repo). One level only (D-2). A project is in at most one group. |
| **Project** | One logical project. Has one or more checkouts. May be git or a plain folder (e.g. `Pico`, `VTOZ`). |
| **Checkout** | A folder on disk belonging to a project: the main clone, a git worktree, or a second clone with the same remote / root commit (D-12). E.g. `trailmix`, `trailmix-second-worktree`, `Archive/trailmix` → one project, three checkouts. |
| **Purpose** | Why I have the project. A user-editable list (D-9), each entry with behaviour flags (§ 6). Seed list: *Developing*, *Reference / reverse engineering*, *Installed tool*, *Contributor* (others' repo I work in), *Archive*. |
| **Status** | Where the project stands, separate from purpose (D-16): *Active*, *Paused*, *Done*, *Abandoned*. E.g. Developing + Paused. |
| **Dependency** | "Project A uses project B at version V", stored in the DB (D-19, § 7.10). |
| **Tags** | Free labels, any number per project (language tags are automatic and separate). |

Not files in the repo: purpose, status, tags, group membership, dependencies, notes, task definitions and dashboard layout live in SQLite only (D-15).
The repo's own files (TODO.md, ROADMAP.md, Claude config) are the truth for their content (D-1).

## 4. Discovery and import (D-6)

- **Scan roots**: a list of folders (e.g. `Desktop/Code`). A scan walks them to a configurable depth (default 3),
  skipping `node_modules`, `target`, `.venv`, `venv`, `build`, `dist` and hidden folders, plus a user ignore list.
- A folder is a **candidate** when it has `.git` (dir or file), or a known manifest (`Cargo.toml`, `package.json`,
  `pyproject.toml`, `requirements.txt`, `go.mod`, `*.csproj`, `compose.yaml`/`docker-compose.yml`), or source files
  at the top level.
- Folders without git, a manifest or source files (e.g. `Ghidra`, `Self Hosting`, `USB Backup`) are not suggested (D-18);
  they can still be added manually.
- A folder that is not a candidate but contains two or more candidates is suggested as a **group** (Hobby).
- Candidates are shown in an **Import** view: accept / ignore / merge into existing project (as a checkout).
  Nothing is tracked without confirmation. Ignored paths are remembered.
- **Manual add**: pick any folder anywhere (other drives included).
- Missing folders (moved/deleted) are flagged, never removed automatically; "Relocate…" fixes the path.

## 5. Overview (main screen)

A table, one row per project, sortable and filterable:

| Column | Source |
|---|---|
| Name | folder name, renamable in the app |
| Group | DB |
| Purpose, status | DB |
| Languages | line/byte counts per extension of git-tracked files (or a walk with ignore rules for non-git); top 3 shown as chips |
| Version | first found: manifest version (`Cargo.toml` workspace/package, `package.json`, `pyproject.toml`), else latest git tag, else — |
| Git | branch, dirty count, ahead/behind upstream, last commit date |
| GitHub | link from the `origin` remote (opens the browser); open issues/PRs count when `gh` is available |
| TODOs | open count from TODO files (+ code TODO count, greyed) |
| Checkouts | count, when > 1 |
| Deps | warning icon when a dependency is behind its latest version (§ 7.10) |

- **Filters**: text search, group, purpose, tag, language, "has local changes", "not on GitHub", "stale (no commit in N months)".
- **Grouping toggle**: flat list, or grouped by Group or by Purpose.
- **Refresh**: on start (in the background, cached results shown at once) and on demand; per-project refresh when a tab is opened.
- Double-click / Enter opens the project in a tab.

## 6. Purpose behaviour (D-9)

Each purpose has flags that change defaults (all overridable per project):

| Flag | Effect | Example |
|---|---|---|
| `track_todos` | TODO/roadmap panels shown, counted in the overview | on for Developing |
| `watch_upstream` | `git fetch` on refresh; show "upstream has N new commits / new release" | on for Reference, Installed tool |
| `show_in_overview` | hidden unless "show all" | off for Archive |
| `default_tasks` | task presets offered (e.g. `git pull` + install command for Installed tool) | |
| `scan_code_todos` | scan for TODO/FIXME | off for Reference (someone else's comments) |

## 7. Project tab

Tabs are `egui_dock` tabs; several projects can be open side by side. Default layout of panels below; a
customisable dashboard (which panels, where, per project or per purpose) comes later (§ 11 M5).

### 7.1 Header
Name, group, purpose, status, tags (editable inline); checkout picker; version; GitHub link; **launchers** (D-8):
open in VS Code, file explorer, terminal (Windows Terminal) and a new Claude Code session, all in the selected checkout.
Launch commands are configurable in settings.

### 7.2 TODOs (files are the truth, D-1; per-project styles, D-23)

**The app's TODO format** (`TODO.md` at the project root; path configurable per project):

```markdown
# TODO

## <Section>

- [ ] Item text, may wrap
      onto indented lines.
  - [ ] Sub-item
- [x] Done item
```

- Sections are `##` headings. Items are `- [ ]` / `- [x]`, nesting by two-space indent.
- Lines the app does not understand (prose, tables) are kept **verbatim** and shown read-only between items;
  edits change only the lines of the touched item (lossless round trip).
- App actions: add, edit, tick, reorder, move between sections, delete. "Clear done" moves ticked items into a
  `## Done` section (or deletes; setting).
- **Migration**: for files in another style (a prose `TODO.md`, a `TODOS.md` without a done section,
  a bare-line `TODO.txt`), "Convert to app format" shows a preview diff and writes only after I confirm, with a backup in
  `backups\`. Until converted, the file is shown read-only.
- Writes: atomic (`.tmp` + rename); the app re-reads the file before writing and refuses if it changed since it
  was read (shows "file changed outside the app — reload").
- File changes from outside (an agent, my editor) are picked up by a file watcher (`notify`) while the tab is open.

### 7.3 Code TODOs (D-14)
- Scan git-tracked files (or a walk honouring `.gitignore`) for `TODO`, `FIXME`, `HACK`, `XXX` in comments.
  ponytail: keyword match on lines, not a real comment parser; false positives in strings are acceptable.
- List: file:line, keyword, text; grouped by file. Click opens the file at that line in VS Code (`code -g file:line`).
- **Promote**: adds an item to a chosen TODO.md section with a `(src/foo.rs:42)` reference. The comment itself is not changed.

### 7.4 Roadmap (D-13)
`ROADMAP.md` (path configurable), milestone headings:

```markdown
# Roadmap

## v0.5 — UI foundation
Status: planned | in progress | done
Target: 2026-11   (optional)

Free text describing the milestone.

- [ ] Item
- [x] Item
```

- Shown as a list of milestone cards with progress (ticked / total) and status; editable like TODOs, same lossless rules.
- A milestone can be linked to a git tag; when the tag exists, the app suggests marking it done.
- A table-style `docs/roadmap.md` is read-only until converted (same conversion flow as TODOs).

### 7.5 Git panel (D-8)
Per selected checkout: branch, status (changed files count and list), branches and worktrees, last 20 commits,
tags. Actions: fetch, pull, push, switch branch, create worktree. No staging/committing UI in v1 (launch VS Code or
an agent for that). All commands go through `git` and their output is shown.

### 7.6 GitHub (D-7, full integration via `gh`)
- Show: repo visibility, stars, open issues and PRs, latest release, CI status of the default branch.
- **Publish**: for a local-only repo, "Create GitHub repo" (`gh repo create`, private by default, asks owner/name)
  and push. Always asks before running, since this publishes code.
- **Issues ↔ TODOs**: an item can be linked to an issue (`(#12)` suffix in the TODO line). "Create issue from item",
  "Import issues into section", and closing one side offers to close the other. No automatic two-way sync in v1.

### 7.7 Tasks (D-8)
- Named commands per project (stored in DB): name, command line, working dir (checkout-relative), env.
- Suggestions from manifests: `cargo run/test/build`, `package.json` scripts, `docker compose up/down`.
- Run inside the tab with live output (stdout/stderr), stop button, exit code, last 5 runs kept in memory.
- Long-running tasks (dev servers) keep running when the tab is closed and show a running indicator in the overview.

### 7.8 Notes (D-8, D-15)
Markdown notes in SQLite, several per project, rendered with `egui_commonmark`. Also a global notes page.
Export to a file on request.

### 7.9 Claude config (D-3)
See § 8. The tab shows this project's part; a global **Claude** page shows `~/.claude` and a cross-project matrix.

### 7.10 Dependencies (D-19)
- A dependency: *this project* → *other tracked project*, plus how to read the used version and the latest one.
- **Used version**: a file in this project and a regex with one capture group (e.g. file `SPEC_VERSION`,
  pattern `spec-v([0-9.]+)`), or a fixed value.
- **Latest version**: the other project's version (§ 5 rules) or its latest git tag matching a glob (e.g. `spec-v*`).
- Shown as "uses spec v10.1 — latest v11" with a warning when behind; the tab lists dependencies and dependents.
  Comparison is numeric per dot-separated part; when that fails, plain "differs".
- Defined by hand in the tab (or over MCP). ponytail: no auto-detection from manifests; add it if hand entry gets tedious.

## 8. Claude configuration management (D-3)

**Scope** (per project and global `~/.claude`):

| Item | Files |
|---|---|
| Instructions | `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md`, nested `CLAUDE.md` in subfolders |
| Settings | `.claude/settings.json`, `.claude/settings.local.json`, `~/.claude/settings.json` |
| Skills / agents / commands | `.claude/skills/*/`, `.claude/agents/*.md`, `.claude/commands/*.md` (and the global ones) |
| MCP and launch | `.mcp.json`, `.claude/launch.json` |

Other agents' files (`opencode.json`) are listed but not edited in v1.

**Features**

- **Inventory**: which projects have which items. A matrix view: rows = projects, columns = item types / names
  (e.g. which projects have the `reviewer` agent, which have `launch.json`).
- **Edit**: text editor with markdown preview; settings.json through a structured form (permissions allow/deny lists,
  env, hooks) with a raw JSON fallback. JSON is validated before saving.
- **Copy**:
  - whole file or folder (a skill) from project A to project B or to/from global;
  - **part of a file**: a markdown section (heading + body) of CLAUDE.md, or a settings key (e.g. one permission rule,
    one hook, one MCP server entry). Merging JSON adds keys and asks on conflicts.
- Every copy/edit shows a diff preview and writes a backup first.
- **Global vs project**: e.g. move a group's agents from `~/.claude/agents` into that group's repos
  (today they live globally).
- Copies are plain copies (D-17): no link to the source, no drift tracking, no sync.

## 9. Templates (D-10)

A template wraps existing generators and adds file overlays. Templates live in a templates folder
(default `%APPDATA%\ProjectArchitect\templates\`, configurable), one subfolder per template. That folder is itself a
tracked project in the app (D-21), so it gets git, TODOs and notes like any other project. A template:

```
template.toml
files/            # overlay: copied after the commands run; {{placeholders}} replaced in text files
```

```toml
name = "Rust egui app"
description = "eframe/egui 0.36 workspace"
purpose = "Developing"
tags = ["rust"]

[[prompt]]          # asked in the New Project dialog
key = "name"
label = "Project name"
[[prompt]]
key = "description"
label = "One-line description"

[[step]]
run = "cargo new {{name}} --vcs git"    # runs in the chosen parent folder
[[step]]
overlay = "files"                       # copies files/ into the new project (CLAUDE.md, .gitignore, TODO.md, ROADMAP.md, …)
[[step]]
run = "git add -A && git commit -m \"Initial commit\""
cwd = "{{project_dir}}"
```

- Built-in placeholders: `name`, `project_dir`, `date`, `year`, `author` (from git config) and every prompt key.
- New Project dialog: pick template → fill prompts → pick parent folder and group → preview (commands and files) →
  create. Output streamed; on failure the partly created folder is left in place and reported, not deleted.
- Templates are plain folders: edit them in any editor, or from the app (template list, edit `template.toml`,
  open folder, duplicate, "make template from this project" copying selected files).
- An option "create GitHub repo" at the end (uses § 7.6).
- Seed templates: empty folder (git init + TODO.md/ROADMAP.md/CLAUDE.md), Rust egui app, Python (uv), SvelteKit
  (`npx sv create`).

## 10. MCP server (D-4, D-11)

`pa-mcp.exe`, a stdio server registered in Claude Code (`claude mcp add projectarchitect -- pa-mcp`) globally or per project.
It opens the same `pa.db` (WAL; busy timeout) and calls the same `core` functions as the GUI, so behaviour is identical.
The GUI notices DB changes (SQLite `PRAGMA data_version` checked every second) and refreshes.

Tools (names indicative):

| Area | Tools |
|---|---|
| Projects | `list_projects(filter)`, `get_project(id\|path)`, `update_project(purpose, status, tags, group, name)`, `list_dependencies`, `set_dependency`, `scan_roots()`, `import_folder(path)`, `current_project(cwd)` (resolve the project for the agent's working dir) |
| TODOs | `list_todos(project, section?, include_done?)`, `add_todo`, `update_todo`, `complete_todo`, `move_todo`, `list_code_todos`, `promote_code_todo`, `capture(project, kind, title, details?)` |
| Roadmap | `get_roadmap`, `add_milestone`, `update_milestone`, `add_milestone_item`, `complete_milestone_item` |
| Git / GitHub | `git_status`, `git_log`, `git_fetch`, `github_info`, `create_issue`, `create_github_repo` |
| Tasks | `list_tasks`, `run_task` (returns output when finished, with a timeout), `add_task` |
| Templates | `list_templates`, `create_project(template, prompts, parent_dir)` |
| Claude config | `list_claude_items(project\|global)`, `read_claude_item`, `copy_claude_item(from, to, part?)`, `edit_claude_item` |
| Notes | `list_notes`, `read_note`, `write_note` |

Rules:

- Tools return structured JSON plus a short text summary.
- Destructive or outward-facing tools (`create_github_repo`, `git push`, overwriting a Claude config file, `run_task`)
  are marked in their description; an app setting can require that they are enabled explicitly (off = tool not listed).
- Never deletes project folders or files other than through the TODO/roadmap/config editing above (backups first).
- Item ids for TODOs are stable across calls as long as the text is unchanged (hash of section + text + occurrence),
  and writes fail cleanly with "changed since read" instead of editing the wrong line.

## 11. Milestones

| Version | Contents |
|---|---|
| M1 | Core store + scanner + import view + overview table (languages, version, git info, GitHub link), launchers. Groups, purposes, tags. |
| M2 | Project tab: TODO.md parser/writer (lossless), conversion flow, code TODOs + promote, roadmap, notes, file watcher. |
| M3 | MCP server over everything M1–M2 exposes. (Early, so agents can use the app while the rest is built.) |
| M4 | Git panel, tasks runner, GitHub via `gh` (info, publish, issues ↔ TODOs), dependencies (§ 7.10). MCP tools for these. |
| M5 | Claude config: inventory matrix, editors, copy whole/part, global page. Templates + New Project. MCP tools. |
| M6 | Customisable dashboard per project/purpose (panel choice and layout saved as `egui_dock` layout). Widgets (D-20): commit activity (commits per week, last 12 months, from `git log`), language breakdown bar, recent Claude sessions (from `~/.claude/projects/<encoded path>/*.jsonl`: date and first prompt, read-only), disk size (total, with `target`/`node_modules`/`.venv` shown separately, computed in the background). |
| M9 | Quick capture (D-31): Capture button and Ctrl+Shift+K; ideas, bugs and TODOs into the TODO file's `## Inbox`, notes into the app; MCP `capture`; GitHub issues once the project has a GitHub remote. |
| M7 | Installation and agent integrations (§ 11.1): Linux (Fedora) build, per-user MSI on Windows, RPM on Fedora, "Add to Claude" and "Add to Codex" buttons. |

### 11.1 M7: installation and agent integrations (D-26 .. D-29)

**Goal:** install the app like any other program on Windows and Fedora, with `pa` and `pa-mcp` in one fixed place,
and connect agents to that one `pa-mcp` from the app instead of by hand.

**Linux build (prerequisite).** Today's Windows-only spots, each behind `cfg` or a per-OS default:

| Spot | Windows | Linux |
|---|---|---|
| App data | `%APPDATA%\ProjectArchitect` | `$XDG_DATA_HOME/projectarchitect` (`~/.local/share/projectarchitect`) |
| Fonts | Segoe UI, Cascadia Mono from `C:\Windows\Fonts` | egui's fonts, or system fonts found via `fc-match` (Cantarell/Inter, a monospace) |
| Launch commands | `cmd /S /C`, no console window | `sh -c`, detached |
| Launcher defaults | `code`, `explorer`, `wt -d`, `wt -d … claude` | `code`, `xdg-open`, `ptyxis`/`gnome-terminal --working-directory`, same + `claude` |
| Renderer | DX12 | wgpu default (Vulkan), `WGPU_BACKEND` overrides |
| Paths in the DB | `C:\…` (canonical, no `\\?\`) | absolute, canonical |

CI (`.github/workflows/ci.yml`): test and clippy on `windows-latest` and in a `fedora:latest` container, on pushes to
main and pull requests. `release.yml` builds the MSI and RPM as artifacts on a `v*` tag, by hand, or on a pull request
that changes the packaging (D-48); a `v*` tag also gets a GitHub release with both attached (D-49).

**Windows: per-user MSI** (D-26, D-48), `wix/main.wxs` built with the WiX v5 dotnet tool (`wix build`):
- Installs `pa.exe`, `pa-mcp.exe` and `pa-cli.exe` to `%LOCALAPPDATA%\Programs\ProjectArchitect` (the folder used for the hand
  registration today), no admin rights; Start-menu shortcut; upgrade in place (stable UpgradeCode); uninstall from
  Settings → Apps. App data is left in place on uninstall.
- A running `pa-mcp.exe` (held open by an agent session) blocks overwriting, not renaming: the installed path stays
  fixed and the MSI sets running exes aside before its files-in-use check (D-47, D-48), so agents keep their server
  through an install, upgrade or uninstall.

**Fedora: RPM** (D-27), built with `cargo-generate-rpm` (metadata in `apps/pa/Cargo.toml`, requires `git`): `/usr/bin/pa`, `/usr/bin/pa-mcp`, `/usr/bin/pa-cli`,
`/usr/share/applications/projectarchitect.desktop`, an icon in `hicolor`; `dnf install ./projectarchitect-<v>.rpm`.
No sandbox: scanning any folder and running `git`, `gh`, `code` work as on Windows. (Flatpak was rejected for that
reason.)

**Agent integrations** (D-28): Settings → MCP server lists each supported agent with its state and one button.
| Agent | Detect | Add | State |
|---|---|---|---|
| Claude Code | `claude` on PATH | `claude mcp add --scope user projectarchitect -- <installed pa-mcp>` | `claude mcp get projectarchitect` (connected / missing / points elsewhere) |
| Codex CLI | `codex` on PATH | `codex mcp add projectarchitect -- <installed pa-mcp>` (writes `[mcp_servers.projectarchitect]` in `~/.codex/config.toml`) | `codex mcp get projectarchitect` |
- Buttons always register the installed `pa-mcp` path (never `target\…`), show the exact command before running it,
  and offer "Update" when a registration points at another path and "Remove".
- `--data-dir` is passed only when the app runs with a non-default data folder.

**Updates from the app** (D-49): Settings → Updates shows the running version and "Check for updates"
(`gh release view --repo J08K/ProjectArchitect`, the newest non-draft, non-pre-release). When the tag is newer,
"Install x.y.z" downloads the installer with `gh release download` into `<data dir>/updates`, then:
- Windows: `msiexec /i <msi> /passive /norestart` and waits; the MSI sets the running exes aside (D-48), so this
  window and the agents' pa-mcp keep running through it. Then it starts the installed `pa.exe` (with `--data-dir`
  when not the default) and closes this window.
- Linux: shows `sudo dnf install "<rpm>"` to run in a terminal (no pkexec).
- Errors shown on the row: gh missing, gh not signed in, no release yet, no installer attached, msiexec's exit code.
- ChatGPT (D-29): its connectors reach only remote HTTPS MCP servers (Developer Mode, paid plans), not local stdio
  ones. Not supported until that changes; revisit then (an HTTP transport plus a tunnel would expose the machine's
  projects to the internet and needs its own design).


## 12. Decisions

| # | Decision |
|---|---|
| D-1 | TODO and roadmap files in the repo are the truth; the app reads and writes them. |
| D-2 | Groups are one level deep. |
| D-3 | Claude config scope: CLAUDE.md/AGENTS.md, settings(.local).json, skills/agents/commands, .mcp.json and launch.json. |
| D-4 | MCP can do everything the UI can do. |
| D-5 | One app TODO format; other styles are converted on request and read-only until then. |
| D-6 | Discovery: scan roots with confirmation, plus manual add. |
| D-7 | Full GitHub integration via `gh`. |
| D-8 | Project tab: launchers, task runner, git panel, notes. |
| D-9 | Purposes are a user-defined list with behaviour flags. |
| D-10 | Templates wrap existing generators and add file overlays. |
| D-11 | MCP is a separate stdio binary sharing the SQLite file. |
| D-12 | Worktrees and clones are checkouts of one project. |
| D-13 | Roadmap format: milestone headings with checkbox items. |
| D-14 | Code TODO comments are scanned and can be promoted to TODO.md items. |
| D-15 | App metadata and notes live only in SQLite. |
| D-16 | Status is a separate field from purpose. |
| D-17 | Claude config copies are plain copies, not linked. |
| D-18 | Scans suggest only folders with git, a manifest or source files. |
| D-19 | Cross-project dependencies are generic, stored in the DB, with version comparison. |
| D-20 | Dashboard widgets: commit activity, language breakdown, recent Claude sessions, disk size. |
| D-21 | The templates folder is a tracked project; its location is configurable. |
| D-22 | Binaries are `pa` and `pa-mcp`. |
| D-23 | Supersedes D-5: each project keeps its own TODO style (checkboxes, bullets, numbered or plain lines; done = ticked, moved to a section, or deleted), detected or set per project. "Change style" rewrites the file in the working copy or commits it to a chosen branch, optionally pushing it and opening a PR. |
| D-24 | A TODOs page in the sidebar lists the items of every project's TODO file and roadmap, with filters (status, project, group, purpose, source), sorting and grouping that are remembered. |
| D-25 | Lines of code per language are counted with tokei, on demand in the project tab. |
| D-26 | Windows install: a per-user MSI (cargo-wix) into `%LOCALAPPDATA%\Programs\ProjectArchitect`, no admin rights. |
| D-27 | Fedora install: an RPM (cargo-generate-rpm) into `/usr/bin`, with a .desktop file; no Flatpak (it would sandbox scanning and launching). |
| D-28 | Settings gets "Add to Claude" and "Add to Codex": user-scope registration of the installed `pa-mcp`, with state, update and remove. |
| D-30 | Roadmaps (M8): versions (`## v1.2.3 — Title`, status planned / in progress / released) next to milestones (themes; `## Milestone: X` or any heading with a Status/Target line or items; other headings are plain sections). Links both ways (`Versions:`, `Milestone:`); milestone status derived from its versions unless set, with a warning on disagreement; release suggestions from git tags with a per-project prefix (default `v`). Views: Now/Next/Later board (default; Next = up to the next planned version), timeline, list, Gantt when most entries have a target. Markdown stays the storage (`docs/briefs/next.md`). |
| D-31 | Quick capture (M9): a sidebar Capture button and the in-app shortcut Ctrl+Shift+K open a modal (Idea · Bug · TODO · Note, title, details, project). Default project: the open project tab's, else ProjectArchitect. Idea/Bug/TODO become an item of `## Inbox` (created above the first section; `TODO.md` created if missing) in the file's style, prefixed `Idea:` / `Bug:`; Note becomes a project note. |
| D-40 | Worktree cleanup (Git panel): each linked worktree shows its uncommitted changes, commits the default branch lacks (or "merged"), its last commit, a lock or missing folder, and when a Claude Code session there last wrote its transcript (`~/.claude/projects/<folder with non-alphanumerics as '-'>/*.jsonl`; "active" within 10 minutes). Worktrees are ticked ("Select merged" picks clean, merged, idle ones; never the main worktree, the checkout shown or a locked one) and removed after a confirmation: `git worktree remove` (with `--force` only when the user ticks it for worktrees with changes), `git worktree prune` for a missing folder, and optionally `git branch -d` for their merged branches. Tracked checkouts whose folder is gone leave the project. GUI only: the MCP server does not delete folders (§ 10). |
| D-41 | Command palette (M10, `palette.rs`): Ctrl+K or the sidebar's "Jump to…" opens a modal that finds pages, projects, every project's sections, the open project's launchers, actions (capture, refresh, new project, theme) and saved searches by words in any order; arrows and Enter run, Esc closes. F5 refreshes all projects. Ctrl+K is checked after Ctrl+Shift+K (egui matches it too). |
| D-42 | TODOs query (M10, `pa_core::query`): words that must all appear plus `project:`, `in:` (section or milestone), `is:open/done`, `kind:<word>` (text starts with `<word>:`, as captures write), `source:todo/roadmap`, `has:issue`; `-` negates, quotes keep spaces, unknown `key:value` is a word. Completions are chips under the bar. Saved searches (`todos.saved`, `name<TAB>query` lines) show in the sidebar with counts (open items unless the query says `is:`, recounted every 2 s) and in the palette. MCP `list_todos_all` takes the same `query` (with `is:` the status defaults to all). |
| D-43 | Board page (M10, `board_page.rs`): Now / Next / Later over every project's roadmap, a collapsible swimlane per project (work in progress first), compact cards (4 per lane, then "n more"). A card moves only within its own swimlane (it lives in that ROADMAP.md); drop logic and writes are shared with the project board (`roadmap_ui::drop_act` / `apply`, drag payload `(project, entry)`). WIP limit for Now: setting `board.wip_now` (0 = off), the lane header turns orange above it. |
| D-44 | Progress and Reports (M10, `pa_core::history`, `progress_ui.rs`, `chart.rs`): open and done items at past commits of the TODO and roadmap files (one point per day the file changed, its last commit; at most 60 days; the path as git tracks it, `:(icase)`), each roadmap entry's progress, commits per week on all branches, and Claude Code sessions per week (transcripts of the checkouts and of Claude worktrees under them, by last write). A project's Progress section shows items over time, today's Now and Next entries as % done over time, and 12 weeks of activity; the Reports page lists every project with open items now, their trend, the change over 30 days, and 4 weeks of commits and sessions. Counted on background threads and kept per session (Recount). Charts are painted with egui's painter. |
| D-45 | Dashboard (M6, `dashboard_ui.rs`): a project's Overview is an `egui_dock` layout of widgets (Repository, Open TODOs, Roadmap, Languages, Commit activity, Claude sessions, Disk size); tabs drag to split or stack, close, and come back through "Add widget". The layout is saved as JSON in `dashboard.<project id>` on every change; a project without one uses `dashboard.purpose.<purpose id>` ("Use for all <purpose>"), else the default; Reset clears the project's. Commit activity is 52 weeks of `git log --all`; Claude sessions are the 12 newest transcripts of the checkouts and their Claude worktrees with the first typed prompt (leading `<tag>…</tag>` blocks skipped), read-only; disk size walks the checkout without following links and shows `target`, `node_modules` and `.venv` (anywhere, outermost) apart. Read on background threads, kept per session. |
| D-46 | Consolidation (2026-10-19): the project tab has nine sections; related ones share a tab with a second bar: TODOs (the TODO file, TODOs in code), Git (repository, GitHub, checkouts). The Progress section is gone: its charts are dashboard widgets (TODOs over time, Roadmap progress) and the dashboard's Activity widget shows commits and Claude sessions per week; Reports links to the dashboard. Projects page: favourites (`projects.favourites`, ids in order) as tiles above the list regardless of filters (click opens, drag reorders, star or context menu toggles) and in the sidebar; a pill names the `origin` host (GitHub, GitLab, else "remote"), nothing for a local-only repository; row and card text never takes the click that opens a project. Import left the sidebar for a + button on the Projects page (and the palette). |
| D-47 | Command line and updates (2026-10-20). `pa-cli` is a console binary in `apps/pa` (the release `pa.exe` is a GUI-subsystem exe that terminals neither wait for nor print from): every MCP tool is a command, run through the same tool table (`pa_mcp::tools`, a library), so the CLI and MCP never drift and `mcp.allow_publish` / `mcp.allow_claude_write` apply to both. `pa-cli <tool> key=value …` types values by the tool's schema (lists as `a,b` or JSON), `--json '<object>'` passes arguments as they are, a required `project` defaults to the current folder; `pa-cli tools [<tool>]` lists tools and parameters. JSON result on stdout, summary on stderr; exit 0 / 1 tool failed / 2 usage. Updates keep one fixed path per binary (registrations never change): `pa_core::install::replace` copies the new file in beside it, renames the installed one to `<name>.<unix secs>.old` (Windows renames a running exe but will not overwrite it) and moves the new one into place; every binary deletes its own unlocked `.old` copies at start. `pa-cli install-mcp [--to <file>]` does this for the `pa-mcp` next to it. Versioned binaries were rejected: each update would rewrite Claude Code's and Codex's config, and running sessions keep their copy either way. |
| D-49 | In-app updates (2026-10-20). A `v*` tag's release job (`release.yml`, `GITHUB_TOKEN` with `contents: write`) runs `gh release create` with the MSI and RPM; pull request and manual runs keep only artifacts. The app reads releases through `gh` (`pa_core::update`), since the repository is private and the app stores no token; the repository is a constant (`update::REPO`), not the checkout's origin, because the installed app has no checkout. Versions compare as x.y.z numbers (`v` optional, pre-release suffixes ignored). Windows installs with `/passive /norestart` (a progress bar, never a reboot) and relaunches the installed path, not the running exe, which may be a dev build. Linux stops at the `dnf` command: root needs a terminal, and a polkit prompt from the GUI was not worth its own design. No automatic checks: the user presses Check. Tried 2026-10-20 with v0.1.1 (first release): a 0.1.0 build found it, downloaded the MSI and installed it with the user's GUI and 11 agent pa-mcp processes running (all set aside, none stopped), then relaunched the installed 0.1.1 with its `--data-dir` and closed, about 45 s in all; 0.1.1's Check then offered nothing. |
| D-48 | M7 as built (2026-10-20). Linux: data in `$XDG_DATA_HOME/projectarchitect`; fonts from `fc-match` (`sans-serif`, `sans-serif:weight=semibold`, `monospace`) with the face index only (egui cannot pick a variable font's named instance), egui's own when fontconfig answers with the sans-serif file again; launchers `xdg-open` and Ptyxis, falling back to GNOME Terminal; the window's app id `projectarchitect` matches `packaging/projectarchitect.desktop` (Wayland takes the icon from there). Icon: `tools/icon.py` draws `assets/` (svg, png sizes, ico, a raw 64 px RGBA window icon so the GUI needs no PNG decoder). MSI: WiX v5 (`dotnet tool install --global wix --version 5.0.2`; v6+ asks for an EULA) instead of cargo-wix, which drives WiX v3, gone from current runners; per-user components keep an HKCU value as key path; fixed UpgradeCode, `MajorUpgrade`. Installers are workflow artifacts, not GitHub releases. Files in use (tried 2026-10-20 with agents running pa-mcp): Restart Manager shut the agents' pa-mcp.exe down; with it off (`MSIRESTARTMANAGERCONTROL=Disable`) a per-user install retried for minutes and failed (1310, no replace-at-reboot without admin). So the custom action `SetAside` (after CostFinalize, before InstallValidate) renames `pa.exe`, `pa-mcp.exe`, `pa-cli.exe` to `<name>.<n>.old` (`cmd /c move`); install and major upgrade then finish in seconds with the agents still running. Windows Installer also never overwrites an unversioned file it did not install whose modified time differs from its creation time (a hand-copied `pa-mcp.exe`); setting aside covers that too. Leftovers: `.old` copies stay until that binary starts again, and after an uninstall the folder keeps them. |
| D-29 | ChatGPT is not supported for now: its connectors need a remote HTTPS MCP server; revisit when it supports local servers. |
| D-32 | Git panel (M4): a project tab section for the selected checkout (branch and upstream, changed files, branches, worktrees, last 20 commits, tags newest version first). Fetch is `fetch --all --prune`; pull is `pull --ff-only` (never a merge commit); push sets the upstream on a branch's first push (`-u origin`); switch is `git switch`. A new worktree (default folder `<checkout>-<branch>` next to it) becomes a checkout of the project. Commands run on a background thread and each one's output is shown. MCP: `git_fetch`, `git_pull`, `git_push`, `switch_branch`, `create_worktree`. |
| D-33 | GitHub (M4, M9): every `gh` call names the repository from the `origin` remote (`-R owner/name`). The GitHub section shows visibility, stars, open issues and PRs, the latest release and CI (the newest run on the default branch, push-triggered runs first). "Create GitHub repo" only for a checkout without `origin`: owner (default the signed-in user), name (default the folder), private by default, push by default; a dialog shows the exact `gh repo create` command and confirms. Issue links are `(#n)` at the end of an item's first line: "Create issue" from an item (title = first line, body = the rest), "Import issues" adds open unlinked issues as `title (#n)`; ticking a linked item offers to close the issue, and items whose issue was closed on GitHub offer to be ticked; no automatic sync. Captures become issues when the per-project setting `capture.issues.<id>` is on and `origin` is on GitHub: labels Bug → `bug`, Idea → `enhancement` (GitHub's default labels; created without labels when the repository lacks them), then `(#n)` on the Inbox line; offline or without a GitHub remote the capture stays in the Inbox. Over MCP, `git_push`, `create_issue`, `close_issue`, `create_github_repo` and a capture's issue need `mcp.allow_publish`; `github_info`, `list_issues`, `import_issues` do not. |
| D-34 | Tasks (M4): stored in the DB (name, one shell command line, folder relative to the checkout, `KEY=VALUE` env lines); run by `cmd /S /C` (Windows) or `sh -c`; stop kills the process tree (`taskkill /T /F`, a process group elsewhere). Runs belong to the app, not the tab: they go on when the tab closes, the overview shows "n running", and closing the app stops them. The last 5 runs per project stay in memory (running ones are never dropped), each with its last 5,000 output lines. Suggestions: `cargo build/test/run`, every `package.json` script through the package manager its lock file names, `docker compose up/down`. MCP `run_task` waits up to `timeout_secs` (default 120, at most 600), then stops the run, and returns the exit code and the last 300 lines; MCP runs are not shown in the GUI. The purpose flag `default_tasks` (§ 6) is not built. |
| D-35 | Dependencies (M4): one per (project, other project), in the DB. Used version: a fixed value, or a file in the project with a regex (first group, else the whole match). Latest: the other project's newest tag matching a glob (git's version sort) or, without a glob, its version (§ 5). Comparison takes the dot-separated numbers after any prefix and before any suffix (`spec-v10.1` vs `spec-v11-b25122525` → behind); equal numbers with different suffixes "differ". Checked on a background thread at start, on "refresh all" and after edits; the overview shows "⚠ deps" in the Status column (no separate Deps column). |
| D-36 | Claude config (M5, `pa_core::claude`): items are named by their path in a project (`CLAUDE.md`, `.claude/agents/x.md`, `.claude/skills/<name>`); the global scope maps `x` and `.claude/x` to `~/.claude/x` and holds only CLAUDE.md, settings.json, agents, skills and commands (user MCP servers in `~/.claude.json` are not touched). The global folder is `claude.home` (setting) or the user's home. Every save and copy is a plan shown as a diff, then a backup and an atomic write after a re-read check. Whole files and skill folders replace an existing different target only with "overwrite"; a markdown section is appended, or replaces the section with the same heading with overwrite; a JSON key (JSON Pointer) is merged: objects by key, arrays gain missing elements, different values are conflicts, replaced only with overwrite. JSON is written as two-space JSON with its key order kept. Other agents' files (opencode.json, GEMINI.md, .cursorrules, .windsurfrules, copilot instructions) are listed read-only. |
| D-37 | MCP writes to Claude config are gated by the setting `mcp.allow_claude_write` (off by default, a checkbox in Settings → MCP server): `copy_claude_item` and `edit_claude_item` return the diff preview, and `apply: true` writes only with the setting on. Reading tools are always on. Like the publish tools, the gated tools stay listed and refuse with a message. `create_project` is not gated: it only creates a folder that does not exist yet, with the user's own templates. |
| D-38 | Templates (M5, `pa_core::template`): `template.toml` has `name`, `description`, `purpose`, `tags`, `[[prompt]]` (`key`, `label`, `default`) and `[[step]]` (`run` with optional `cwd`, or `overlay`); `name` is always asked first. Unknown `{{placeholders}}` stay as they are; names in the overlay are filled too. Steps run through the shell (`cmd /S /C`, `sh -c`) with stdout and stderr streamed; the project folder must not exist; a failed step stops and leaves the folder. A created project is tracked with the template's purpose (when the app has it) and tags, in the chosen group (default: the parent folder's group). The folder is `templates.dir` (default `<data dir>\templates`); the seeds (empty, rust-egui, python-uv, sveltekit) are written only when their folder is missing, and the folder is tracked as the project "Templates" once (`templates.tracked`), so removing it from the app sticks. "Make template from a project" copies chosen top-level files as they are (no placeholders added). The "create GitHub repo" option is not built yet (M4's `gh repo create`, D-33, can back it). |
| D-39 | Claude GUI: a Claude section in the project tab (item list by kind, markdown Edit/Preview, settings.json Form/Raw: permissions allow/deny/ask, env and hooks as rows, other keys in Raw) and a Claude page in the sidebar with two views, the global `~/.claude` (same editor) and the all-projects matrix (rows: global + projects at their primary checkout; columns: item names; filter by kind; a click opens the project's Claude section). Templates get their own sidebar page; New Project is a dialog (template → prompts → folder and group → preview → create). |

No open questions remain for M1. New ones go here as they come up.
