# Brief: what's next (2026-10-16)

Where the project stands, the open leftovers, and the design for the next two milestones the user asked for:
**M8 Roadmap v2** (versions next to milestones, several views, the storage question) and **M9 Quick capture**
(a feedback button for ideas about this app). Recommendations are marked **Rec**; open questions are in § 5.

## 1. State (updated end of session, 2026-10-16)

- Done: M1 overview, M2 project tab (hand-tested 2026-10-16), M3 MCP server (`pa-mcp`, registered with Claude Code
  at user scope from `%LOCALAPPDATA%\Programs\ProjectArchitect\pa-mcp.exe`), **M8 Roadmap v2** (this brief's § 2-3,
  built as decided in § 5; commit 7c687f8).
- The § 1 leftovers of the first draft are done (GUI uses `pa_core::ops` for the same-repository rule and file
  paths; MCP `list_todos_all` includes roadmap items). Remaining small items are in `TODO.md`.
- **M9 Quick capture** (§ 4) built 2026-10-17: Capture button + Ctrl+Shift+K, `## Inbox` in the TODO file, notes,
  MCP `capture` (D-31); captures become issues with M4 (`capture.issues.<id>`, D-33).
- **M4** (git panel, GitHub via `gh`, tasks, dependencies; D-32 .. D-35) and **M5** (Claude config, templates and
  New Project; D-36 .. D-39) built in parallel on 2026-10-17 and merged. What was not tried for real is in `TODO.md`.
- **M10** (after JetBrains YouTrack, 2026-10-19): command palette, TODOs query + saved searches, cross-project Board,
  Progress section + Reports page (D-41 .. D-44).
- Published 2026-10-19 as the private GitHub repository J08K/ProjectArchitect (`origin`, `main` tracked); the
  GitHub section, issues and captures-as-issues can now be tried on this project.
- **M6** dashboard built 2026-10-19 (D-45): the project Overview is a dock of widgets, layout per project or purpose.
- Consolidation pass 2026-10-19 (D-46): project tab 13 → 9 sections, Progress folded into the dashboard,
  favourites, remote pill, Import off the sidebar.
- **M7** built 2026-10-20 (D-47, D-48): `pa-cli`, Linux build, MSI and RPM from CI, agents in Settings; the MSI
  upgrade was tried with agents running pa-mcp. Open: ChatGPT (D-29) and what TODO.md lists as not tried.
- Items got drag handles and a ⋯ menu; Board and Timeline entries unfold to show their items (app-next-todos,
  merged 2026-10-17).

## 2. M8 Roadmap v2: versions and milestones

**What the user wants:** most software is versioned `vX.Y.Z`. A *milestone* is a theme ("UI revamp", "Vehicle
workspace"); a *version* is what ships. A milestone can span several versions, and one version can carry work of
several milestones.

**Model (Rec):** two kinds of roadmap entries.

| Entry | Is | Has |
|---|---|---|
| Version (`v0.5.0`) | a release; sorted by semver | status, target date, items, the milestone(s) it serves |
| Milestone | a theme; ordered by hand | status (derived from its versions unless set), description, items not tied to a version |

- Status of a version can follow git: a tag `vX.Y.Z` (or `<prefix>X.Y.Z`, per project) means **released**. The app
  suggests it (as today's "tag exists → done" idea in SPEC § 7.4), never edits silently.
- Milestone progress = done items of its own + of its versions.
- Projects without versions keep working: a roadmap with only milestones is the current M1/M2 style.

**Storage (Rec): keep Markdown in the repo** (`ROADMAP.md`), with a stricter convention, because it is diffable,
readable on GitHub, editable by hand and by agents (no tool needed), and the app already edits it losslessly.
Alternatives considered: TOML/YAML (structured, but long text and checklists read badly, merge conflicts on lists),
JSON (worse to hand-edit), SQLite (not in git, invisible to agents and GitHub), GitHub Projects/issues (online-only,
not every project is on GitHub). Proposed convention:

```markdown
# Roadmap

## Milestone: UI revamp
Status: in progress
Versions: v0.5.0, v0.5.1

Free text.

- [ ] Item not tied to a version

## v0.5.0 — UI foundation
Status: released            (planned | in progress | released; "done" accepted as released)
Target: 2026-11
Milestone: UI revamp        (optional; several separated by commas)

- [x] Command palette
- [ ] Docking
```

- A heading starting with `Milestone:` is a milestone; one starting with a version (`v1.2.3`, `1.2`, `v0.4.5-beta`)
  is a version; anything else stays a plain section (text kept verbatim, not shown as an entry: fixes the
  "## Direction" case).
- `Versions:` on a milestone and `Milestone:` on a version are both accepted; the app writes whichever side the
  user edits and shows the union.
- Today's `## M2 — Project tab` headings keep working as milestones (no `Milestone:` prefix needed when the file
  has no versions at all).
- Conversion: the existing "Change style" pattern (preview, backup, optional branch + PR) for upgrading a roadmap.

## 3. M8 Roadmap v2: views

**Use:** "the status of the project NOW and what's next; after that mostly long-term planning". So the default
view must answer *now / next / later* at a glance; dates are optional.

| View | Good for | Needs dates | Rec |
|---|---|---|---|
| **Now / Next / Later** board (kanban lanes by status: In progress · Next · Later · Released) | the main question, at a glance | no | **default** |
| **Timeline** (vertical, scrolling, versions under their milestone, released at the bottom or folded) | reading the story; long-term plan | no | second view |
| **List** (today's cards, editable items) | editing | no | keep as the edit view |
| Gantt | schedules | yes (targets on most entries) | only offered when ≥ half the entries have a `Target:`; else hidden |

- "Next" = the first N planned entries in file order (N = 3, setting); "Later" = the rest. Dragging a card between
  lanes sets its status; dragging within a lane reorders the file sections (lossless move of the whole section).
- The view choice is remembered per project (setting `roadmap.view.<project id>`).
- The overview timeline card (done) is the compact form of the Timeline view.

## 4. M9 Quick capture (feedback button)

**Want:** a button that opens a small form to jot down ideas/notes/TODOs about developing *this app*, saved
somewhere, later turned into GitHub issues once the repo is on GitHub.

**Rec:** make it general but default to ProjectArchitect:
- A **Capture** button in the sidebar footer and a global shortcut (Ctrl+Shift+K) open a modal: kind (Idea · Bug ·
  TODO · Note), one-line title, optional details, target project (default: ProjectArchitect, or the open project
  tab's), optional section.
- **Where it goes:** TODO / Bug / Idea → the target's TODO file in a `## Inbox` section (in that file's own style;
  the kind as a prefix, e.g. `Idea: …`); Note → the project's notes in the app DB. Files stay the truth (D-1), so
  agents see captures through `list_todos`.
- **GitHub (later, once published):** per project setting "Captures become issues": create the issue with `gh issue
  create` (labels from the kind), and write the issue number into the TODO line (`(#12)`), the issue ↔ TODO link
  already planned in SPEC § 7.6. Offline or no remote → stays in the Inbox, with a "Create issues" action later.
- MCP: `capture {kind, title, details?, project?}` so an agent can file ideas the same way.

## 5. Decisions (user, 2026-10-16)

1. Versions use **planned / in progress / released**; `done` on a version is read as released.
2. The **Next** lane holds everything planned up to and including the next unreleased version (no versions in
   the file: the first 3 planned entries); **Later** is the rest.
3. Release tags: a **per-project prefix, default `v`** (`spec-v` for a spec project, empty for bare `1.2.3`).
4. Milestone vs versions: **derive and warn**. A milestone without its own `Status:` takes it from its versions
   (all released → done; any in progress or released → in progress; else planned); with a `Status:` that
   disagrees, the app shows a warning.
5. (M9) Built with the recommendation, open to change: in-app shortcut **Ctrl+Shift+K** (not system-wide), default
   target the open project tab's project, else ProjectArchitect. The Inbox is created above the first section.

Simplification adopted while building: the `Milestone:` prefix is optional. A heading that starts with a version
is a version; any other heading is a milestone when it has `Milestone:`, a `Status:`/`Target:` line or items, and a
plain section otherwise. So existing roadmaps (`## M2 — Project tab`, `## v0.4.1 — Spec v7`)
need no conversion; only table roadmaps do.

## 6. Proposed order

1. Leftovers (§ 1) — one short session.
2. M8 Roadmap v2: parser/writer for versions + milestones in core (with tests on the user's real roadmaps, copies
   only), Now/Next/Later board, Timeline view, conversion; MCP tools updated (`get_roadmap` returns versions too).
3. M9 Quick capture (small): modal + Inbox + MCP `capture`; GitHub issues when the repo is published (with M4's
   `gh` work).
4. Then M5 (Claude config + templates), M4, M6, M7 as planned.
