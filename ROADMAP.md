# Roadmap

## M1 — Overview
Status: done

Core store, scanner, import view, overview table (languages, version, git, GitHub link), launchers. Groups,
purposes, statuses, tags.

- [x] Workspace, store, schema
- [x] Scanner and import view
- [x] Overview table with filters, sorting and grouping
- [x] Project tab header with launchers and checkouts
- [x] Settings: scan roots, ignore list, groups, purposes, launchers
- [x] Hand test on the real tree
- [x] UI redesign: sidebar, purpose tabs, table and grid views, light and dark themes

## M2 — Project tab
Status: done

TODO.md parser and writer (lossless), conversion flow, code TODOs and promote, roadmap, notes, file watcher.

- [x] Core: TODO and roadmap parsers and writers, conversion, code TODO scan, notes, descriptions
- [x] Project tab sections: overview, TODOs, roadmap, code TODOs, notes, checkouts, settings
- [x] Outside changes picked up (mtime check once a second while the tab is open)
- [x] Hand test: tick, edit, add, move, delete, convert, promote, notes
- [x] TODO styles per project (D-23), Change style dialog with branch commit and pull request
- [x] TODOs page across projects with remembered filters (D-24)
- [x] Lines of code per language (tokei, D-25)
- [x] Hover and press states on every button; consistent text fields

## M3 — MCP server
Status: done

`pa-mcp` over everything M1 and M2 expose.

- [x] stdio JSON-RPC server, 32 tools (projects, TODOs in every style, roadmap, code TODOs, notes, lines, git)
- [x] Shared operations in `pa_core::ops`
- [x] GUI reloads when the database changes outside it; Settings shows the registration command and the publish switch
- [x] Registered with Claude Code and used from a real agent session

## M8 — Roadmap v2: versions and views
Status: done

Versions (vX.Y.Z) next to milestones (themes), several views, a stricter Markdown convention. Brief:
`docs/briefs/next.md` § 2-3.

- [x] Leftovers first: GUI uses `pa_core::ops`; MCP `list_todos_all` with roadmap items
- [x] Core: parse and write versions and milestones (`Milestone:` / version headings, `Versions:` / `Milestone:` links, tag-based release suggestion)
- [x] Now / Next / Later board (default view), drag between lanes sets the status
- [x] Vertical timeline view; Gantt only when most entries have a target
- [x] Roadmap conversion with preview, backup, branch and PR (as Change style)
- [x] MCP: `get_roadmap` returns versions; tools to add and update versions

## M9 — Quick capture
Status: done

A Capture button and shortcut: ideas, bugs, TODOs and notes for this app (or any project) into the TODO file's
Inbox; GitHub issues once the repo is published. Brief: `docs/briefs/next.md` § 4.

- [x] Capture modal (kind, title, details, target project) and sidebar button + shortcut (Ctrl+Shift+K)
- [x] Inbox section in the target's TODO file, in its own style; notes into the app DB
- [x] MCP `capture` tool
- [x] Captures become GitHub issues (`gh issue create`, issue number on the TODO line) when the project has a GitHub remote

## M4 — Git, tasks, GitHub
Status: done

Git panel, task runner, GitHub via `gh`, dependencies (SPEC § 7.5-7.7, § 7.10, D-32 .. D-35).

- [x] Schema step 5: tasks and dependencies
- [x] Git panel: branch, changes, branches, worktrees, last 20 commits, tags; fetch, pull, push, switch, new worktree, with output
- [x] GitHub section: visibility, stars, issues, PRs, release, CI; Create GitHub repo (confirmed, private by default)
- [x] Issues and TODOs: `(#n)` links, create issue from an item, import issues, close the other side
- [x] Tasks: stored commands, manifest suggestions, live output, stop, exit code, last 5 runs, running chip in the overview
- [x] Dependencies: used and latest versions, numeric comparison, uses and used by, warning chip in the overview
- [x] MCP: 18 tools (git, GitHub, tasks, dependencies); publishing behind `mcp.allow_publish`
- [x] Worktree cleanup: how each worktree is used (changes, unmerged, Claude activity), bulk select and remove (D-40)

## M5 — Claude config and templates
Status: done

Inventory matrix, editors, copy whole or part, global page; templates and New Project (SPEC § 8, § 9, D-36 .. D-39).
The template option "create GitHub repo" is not built yet; M4's Create GitHub repo can back it (`TODO.md`).

- [x] Core `claude`: inventory per project and global, whole / section / JSON-key copies with conflicts, edits with JSON validation, diff preview, backup and re-read check
- [x] Claude section in the project tab: markdown editor with preview, settings form (permissions, env, hooks) with raw JSON
- [x] Claude page: global `~/.claude` and the all-projects matrix
- [x] Core `template`: `template.toml` prompts and steps, placeholders, streamed run, seeds, duplicate, make from a project
- [x] Templates page and New Project dialog; the templates folder is a tracked project
- [x] MCP: `list_claude_items`, `read_claude_item`, `copy_claude_item`, `edit_claude_item` (writes gated by `mcp.allow_claude_write`), `list_templates`, `create_project`

## M10 — Navigation, search, board and reports (after YouTrack)
Status: done

Ideas from JetBrains YouTrack that fit a one-person, files-first tracker (D-41 .. D-44).

- [x] Command palette (Ctrl+K): pages, projects, sections, launchers, actions, saved searches; F5 refreshes all
- [x] Query bar on the TODOs page (`project:`, `in:`, `is:`, `kind:`, `source:`, `has:issue`, `-term`) with completions; MCP `list_todos_all` takes it
- [x] Saved searches with counts in the sidebar
- [x] Board page: every project's roadmap in Now / Next / Later swimlanes, drag writes the project's ROADMAP.md, WIP limit for Now
- [x] Progress section per project and Reports page: items over time and roadmap entries from git history, commits and Claude sessions per week

## M6 — Dashboard
Status: done

Customisable panel layout per project or purpose; widgets: commit activity, language breakdown, recent Claude
sessions, disk size (D-20, D-45).

- [x] Project Overview as an `egui_dock` dashboard: drag widgets to split or tab them, close and add them back
- [x] Layout saved per project, or for every project of a purpose ("Use for all"); Reset falls back
- [x] Widgets: repository, open TODOs, roadmap, languages, commit activity (52 weeks), Claude sessions (date and first prompt), disk size (`target` / `node_modules` / `.venv` apart)

## M7 — Install and agent integrations
Status: in progress

Install like any program on Windows and Fedora, one shared `pa-mcp`, agents connected from Settings (SPEC § 11.1,
D-26 .. D-29).

- [x] Linux build: XDG data dir, fonts, `sh -c` launches, per-OS launcher defaults, wgpu backend; CI on Windows and Fedora (D-48)
- [x] `pa-cli`: every MCP tool as a command; updates set a running binary aside under its fixed path (D-47)
- [x] Windows: per-user MSI with WiX v5 (`%LOCALAPPDATA%\Programs\ProjectArchitect`, upgrade in place, also with pa-mcp running; D-48)
- [x] Fedora: RPM with cargo-generate-rpm (`/usr/bin`, .desktop file, icon)
- [x] Settings: "Add to Claude" and "Add to Codex" (state, add, update, remove; always the installed pa-mcp)
- [x] Updates from Settings: GitHub releases on `v*` tags, check / download / install / relaunch through `gh` (D-49)
- [ ] ChatGPT: revisit when its connectors accept local MCP servers
