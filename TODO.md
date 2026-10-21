# TODO

## Outside this repo


## Not yet tried for real

- [ ] Drag and drop of items in the TODO and roadmap lists (handle on the left; drop on a row = before it, on the "Add" row = end of that section), and the row's ⋯ menu.
- [ ] Capture (Ctrl+Shift+K) into a real project's Inbox; the MCP `capture` tool from an agent (needs the rebuilt pa-mcp copied over the registered one).
- [ ] Change style / roadmap conversion to a branch with "push and open a pull request" against a real remote.
- [ ] Board drag and drop between lanes, and the Gantt view (none of the real roadmaps has `Target:` dates).
- [ ] MCP write tools against real projects (tested on scratch projects only).
- [ ] Refresh speed in a release build (several seconds in debug with code TODO scanning).
- [ ] Capture from the GUI into a real project (tested on scratch projects; the modal only seen in a screenshot).
- [ ] GitHub writes never ran against real GitHub: Create GitHub repo, create / close issue, Import issues, captures becoming issues, MCP `git_push`. Tested: argument builders, JSON parsing, the refusals; the reads (`gh repo view`, `gh run list`, `gh issue list`) on real repositories.
- [ ] Git panel against a network remote (fetch, pull, push tested on a local bare remote only); new worktree from the GUI (tested through MCP and core tests).
- [ ] Removing worktrees from the Git panel (core tested on temp repositories; the confirmation dialog not yet clicked through), including a folder held open by another program.
- [ ] Board page: dragging a card between lanes (the per-project board's drop logic, shared; not clicked through).
- [ ] Saved searches: saving and deleting from the Saved menu, and the completion chips (seen in screenshots only).
- [ ] Progress / Reports on a long history: every point is a `git show`; check the time on a repository with years of commits.
- [ ] Tasks with a real dev server (`npm run dev`, `cargo run`): Stop killing the whole tree, long output; MCP `run_task` from a real agent session.
- [ ] Seed templates other than "Empty folder" (that one was run from the GUI into a scratch folder): Rust egui app (`cargo new` + an eframe 0.36 `main.rs` that was never built), Python (`uv init`), SvelteKit (the `npx --yes sv create … --template minimal --types ts --no-add-ons --install npm` flags are unverified).
- [ ] Claude config writes against real projects and the real `~/.claude` (GUI copy, save and settings form tried on scratch copies with `claude.home` pointing at a scratch folder; MCP writes in tests only).
- [ ] Templates page: editing `template.toml`, Duplicate and "Make a template from a project" in the GUI (core and MCP tested; the page only seen in screenshots).

- [ ] Linux on a real Fedora desktop: the GUI ran only in a container (Xvfb, software Vulkan); launchers (Files, Ptyxis), the .desktop entry and its icon on Wayland, `code` and `claude` from the app.
- [ ] MSI: the Settings → Apps entry and an install by double-click (full UI) not seen; tested with `msiexec /passive`. A real version bump (Cargo.toml) through the release workflow not run yet (the upgrade test used a copy with its version changed).
- [ ] Settings → MCP server: Add / Update / Remove never clicked (the real `projectarchitect` registration was left alone); Codex not installed here.

- [ ] In-app update (D-49) on Linux: the `dnf` command row not looked at, and the RPM from a release not installed by hand. (Windows tried 2026-10-20, see D-49.)

## Gaps

- [ ] MSI uninstall leaves the set-aside `.old` copies (and so the install folder); they cannot be removed while an agent runs them.

- [ ] Project Settings has no field for the release tag prefix (`tag_prefix`, default `v`); only MCP `update_project` sets it.
- [ ] Capture writes to the primary checkout's TODO file; a project tab showing another checkout does not see it.
- [ ] Delete-style TODO files (a TODOS.md, TODO.txt): ticking an item removes it from the file; consider an undo.
- [ ] Overview and sidebar TODO counts come from cached info: after an edit made outside the GUI they lag until the next refresh.
- [ ] Board and Timeline show an entry's items read-only when unfolded; ticking happens in the List view.
- [ ] Drag and drop into a numbered TODO list keeps the old numbers (as adding does).
- [ ] Board: no reordering within a lane (only between lanes); use the List view or the file.
- [ ] Version titles keep annotations from the heading (`v0.8 (was v0.10; v0.8.0 done) — Virtual parts` → title "(was v0.10; …) — Virtual parts").
- [ ] Roadmap edits on an unknown entry report "no item with id …" (shared `TodoError::NotFound` text).
- [ ] Issue links in the GUI edit the selected checkout's TODO file, but captures (and MCP) link in the primary checkout's.
- [ ] Issue states (closed on GitHub → tick the item) are read once per app session and after closing an issue here; no refresh button.
- [ ] Not built: purpose flags `default_tasks` and `watch_upstream` (fetch on refresh, "upstream has N new commits"), and the overview's open issue / PR counts (SPEC § 5).
- [ ] GUI tasks run in the selected checkout, MCP `run_task` in the primary one; MCP runs do not show in the app.
- [ ] The Git panel lists local branches only; a remote-only branch needs a fetch and `git switch <name>` (MCP `switch_branch` does the same).
- [ ] The GUI keeps its own cached load/edit/save for TODO and roadmap files (it edits what the user saw, with the re-read check); `pa_core::ops` re-reads first. Both use `todo::save`; merge if they drift.
- [ ] Templates: the "create GitHub repo" option at the end of New Project (SPEC § 9) is not built; M4's Create GitHub repo (`pa_core::github`, D-33) can back it now.
- [ ] Claude config: user-scope MCP servers live in `~/.claude.json` (with much other state) and are not listed or copied; plugins' skills under `~/.claude/plugins` neither.
- [ ] Settings form saves rewrite the file as two-space JSON (key order kept); the hand formatting of a compact file is lost (the review shows it).
- [ ] Claude section: on long settings forms the header with Save scrolls out of view; a sticky header would help.
- [ ] `theme::segmented` inside a right-to-left layout draws its segments in reverse order (the notes view's Edit / Preview does); the Claude views keep it out of right-to-left layouts.
- [ ] MCP `create_project` has no timeout: a template command that waits or hangs blocks the server (stdin is closed, so most prompts fail fast).
- [ ] A skill folder copy that fails part way (a file changed outside) leaves the files written before it; each file is whole.

## Design debt

- [ ] Settings → MCP server: agent rows (state chip, Add / Update / Remove, the "will run" command line) built without a design (apps/pa/src/settings.rs, `mcp`).
- [ ] Settings updates row (version, chip, Check / Install, error and `dnf` lines) built without a design (apps/pa/src/settings.rs, `updates`).
- [ ] App icon (three roadmap bars on a dark tile) drawn without a design (tools/icon.py, assets/).
- [ ] Select menus highlight the chosen row instead of the designed check mark at its right (apps/pa/src/theme.rs, `select`; callers use `selectable_value`).

## Later

- [ ] Bulk edit in the overview (select several rows, set purpose or status).
