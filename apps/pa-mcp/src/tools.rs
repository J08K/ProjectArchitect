//! The tool table (SPEC § 10, D-4): names, JSON Schemas, argument parsing and dispatch. The logic lives in
//! `pa_core::ops` and the core modules; this file only turns arguments into calls and results into JSON.

use crate::Server;
use pa_core::codetodo;
use pa_core::db::{Project, Status, Store};
use pa_core::deps::{self, Dependency, Standing};
use pa_core::info;
use pa_core::tasks::{self, Task};
use pa_core::{git, github};
use pa_core::loc::{self, LocRow};
use pa_core::ops::{self, Catalog, ProjectEdit, StyleTarget};
use pa_core::roadmap::{self, EntryKind, Milestone, MilestoneStatus};
use pa_core::todo::{Style, TodoDoc, TodoItem};
use pa_core::{claude, template};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A successful call: a one-line summary for the text content, and the structured data (always an object).
pub struct Out {
    pub summary: String,
    pub data: Value,
}

/// A failed call. `Params` is a protocol error (-32602); `Tool` is a result with `isError: true`.
#[derive(Debug)]
pub enum Failure {
    Params(String),
    Tool(String),
}

impl<E: std::error::Error> From<E> for Failure {
    fn from(e: E) -> Self {
        Failure::Tool(e.to_string())
    }
}

// --- the tool table -------------------------------------------------------------------------------------------

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

fn text(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn nullable(description: &str) -> Value {
    json!({"type": ["string", "null"], "description": description})
}

fn int(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}

fn flag(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn texts(description: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": description})
}

fn choice(description: &str, values: &[&str]) -> Value {
    json!({"type": "string", "enum": values, "description": description})
}

fn project() -> Value {
    json!({"type": ["integer", "string"], "description": "Project id, name (case-insensitive) or a path inside one of its checkouts."})
}

const STATUSES: [&str; 4] = ["Active", "Paused", "Done", "Abandoned"];
const STYLE_HELP: &str = "A TODO style as stored: checkbox, bullet:delete, bullet:move:<section>, numbered:delete, line:delete";

/// The tools, for `tools/list`. Every name here must be handled by [`call`].
pub fn list() -> Vec<Value> {
    vec![
        tool(
            "current_project",
            "The project whose checkout contains a path (longest match). Call it first with the working directory.",
            json!({"path": text("A folder or file path, e.g. the working directory.")}),
            &["path"],
        ),
        tool(
            "list_projects",
            "Projects with their cached info (no refresh). Filters combine.",
            json!({
                "query": text("Text to find in the name, tags or path."),
                "group": text("Group name."),
                "purpose": text("Purpose name."),
                "status": choice("Project status.", &STATUSES),
                "has_changes": flag("true: only projects with uncommitted changes; false: only clean ones."),
            }),
            &[],
        ),
        tool(
            "get_project",
            "One project in full: checkouts, description, TODO and roadmap files, TODO style. refresh: true re-reads git and manifests first (slower).",
            json!({"project": project(), "refresh": flag("Re-collect and store the info of every checkout.")}),
            &["project"],
        ),
        tool(
            "update_project",
            "Change a project's name, group, purpose, status, tags, TODO style or release tag prefix. Group and purpose by name; null clears them.",
            json!({
                "project": project(),
                "name": text("New name."),
                "group": nullable("Group name, or null to clear."),
                "purpose": nullable("Purpose name, or null to clear."),
                "status": choice("Project status.", &STATUSES),
                "tags": texts("All tags (replaces the list)."),
                "todo_style": nullable("A stored TODO style (see change_todo_style), \"detect\" to detect it again, or null."),
                "tag_prefix": nullable("Prefix of the release tags: \"v\" (default, v0.5.0), \"\" for bare 0.5.0, or null for the default."),
            }),
            &["project"],
        ),
        tool("list_groups", "Groups with the number of projects in each.", json!({}), &[]),
        tool("list_purposes", "Purposes with their behaviour flags.", json!({}), &[]),
        tool(
            "scan",
            "Folders under the scan roots that look like projects, and folders that look like groups. Lists only; nothing is tracked (import_folder does that).",
            json!({"roots": texts("Folders to scan at depth 3 instead of the configured scan roots.")}),
            &[],
        ),
        tool(
            "import_folder",
            "Track a folder as a project, or as a checkout of an existing project when the repository (root commit or GitHub remote) matches.",
            json!({
                "path": text("The folder."),
                "name": text("Project name (default: the folder name)."),
                "group": text("Group name (default: the group whose folder is the parent)."),
                "purpose": text("Purpose name."),
            }),
            &["path"],
        ),
        tool(
            "list_todos",
            "Items of the project's TODO file. Line numbers are 1-based.",
            json!({
                "project": project(),
                "section": text("Only items under this ## section."),
                "include_done": flag("Include ticked or done items (default false)."),
            }),
            &["project"],
        ),
        tool(
            "add_todo",
            "Add an item to the TODO file. Writes the file (the app re-reads it first and refuses if it changed).",
            json!({
                "project": project(),
                "text": text("The item text."),
                "section": text("The ## section (default: the first section)."),
                "parent_id": text("Add as a child of this item id (section is then ignored)."),
            }),
            &["project", "text"],
        ),
        tool(
            "update_todo",
            "Replace an item's text, keeping its marker and children. Writes the TODO file. The id changes with the text.",
            json!({"project": project(), "id": text("Item id from list_todos."), "text": text("The new text.")}),
            &["project", "id", "text"],
        ),
        tool(
            "complete_todo",
            "Tick or untick an item, or move/remove it, as the file's style says. Writes the TODO file.",
            json!({"project": project(), "id": text("Item id."), "done": flag("true to complete, false to reopen.")}),
            &["project", "id", "done"],
        ),
        tool(
            "move_todo",
            "Move an item to another ## section (appended there), or up/down among its siblings with delta. Writes the TODO file. The id changes when the section changes.",
            json!({
                "project": project(),
                "id": text("Item id."),
                "section": text("Target ## section."),
                "delta": int("Positions to move: negative up, positive down."),
            }),
            &["project", "id"],
        ),
        tool(
            "remove_todo",
            "Delete an item and its children from the TODO file. Writes the file.",
            json!({"project": project(), "id": text("Item id.")}),
            &["project", "id"],
        ),
        tool(
            "capture",
            "File an idea, bug, TODO or note for a project, as the app's Capture button does. Idea, Bug and TODO go to the ## Inbox section of the TODO file (created on top; TODO.md is created if missing), prefixed with the kind; Note becomes a project note.",
            json!({
                "project": project(),
                "kind": choice("What it is.", &["Idea", "Bug", "TODO", "Note"]),
                "title": text("One line."),
                "details": text("More text: continuation lines of the item, or the note's body."),
            }),
            &["project", "kind", "title"],
        ),
        tool(
            "create_todo_file",
            "Create TODO.md in the app format (## Open) in the primary checkout. Refused when a TODO file exists.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "list_todos_all",
            "TODO and roadmap items across projects, like the TODOs page. Each item names its project and its source (todo or roadmap); roadmap items also name their entry.",
            json!({
                "status": choice("open (default), done or all.", &["open", "done", "all"]),
                "source": choice("Only TODO-file items or only roadmap items (default both).", &["todo", "roadmap"]),
                "project": project(),
                "group": text("Group name."),
                "purpose": text("Purpose name."),
                "query": text("A query as on the TODOs page: words that must all appear, project:<name>, in:<section>, is:open|done, kind:idea|bug, source:todo|roadmap, has:issue; -term negates; quotes keep spaces. With is: in it, status defaults to all."),
            }),
            &[],
        ),
        tool(
            "change_todo_style",
            "Rewrite the TODO file in another style. mode working_copy backs up and rewrites the file here. mode branch commits it to a branch without touching the working tree. push_and_pr PUBLISHES: it pushes the branch and opens a pull request on GitHub, and only works when publishing is enabled in the app settings.",
            json!({
                "project": project(),
                "style": text(STYLE_HELP),
                "mode": choice("working_copy or branch.", &["working_copy", "branch"]),
                "branch": text("Branch to commit to (branch mode)."),
                "base": text("Branch to create it from (default: the default branch)."),
                "push_and_pr": flag("PUBLISHES: push the branch and open a pull request (branch mode only; needs publishing enabled)."),
            }),
            &["project", "style", "mode"],
        ),
        tool(
            "get_roadmap",
            "The roadmap's entries in file order: versions (kind version, e.g. v0.5.0) and milestones, with status (effective, derived from the versions unless set), explicit_status, warning, links, target, progress and items. Also board (now, next, later, done: entry labels) and release_suggestions (unreleased versions whose tag exists). editable is false for table roadmaps.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "add_milestone",
            "Append a milestone (## heading with a Status line) to ROADMAP.md.",
            json!({
                "project": project(),
                "title": text("Milestone title."),
                "status": choice("Status (default planned).", &["planned", "in progress", "done"]),
            }),
            &["project", "title"],
        ),
        tool(
            "set_milestone_status",
            "Set an entry's Status line. Find the entry by version (v0.5.0) or title. A version's done is written as released.",
            json!({
                "project": project(),
                "title": text("Milestone title, version (v0.5.0) or heading."),
                "status": choice("New status.", &["planned", "in progress", "done", "released"]),
            }),
            &["project", "title", "status"],
        ),
        tool(
            "add_version",
            "Add a version (## v0.5.0 — title, Status line, optional Milestone line) to ROADMAP.md, in semver order.",
            json!({
                "project": project(),
                "version": text("The version, such as v0.5.0 or 1.2."),
                "title": text("What it ships (optional)."),
                "status": choice("Status (default planned).", &["planned", "in progress", "released"]),
                "milestone": text("Milestone it serves (optional)."),
            }),
            &["project", "version"],
        ),
        tool(
            "link_version",
            "Name a milestone in a version's Milestone line (the milestone then counts the version's items).",
            json!({
                "project": project(),
                "version": text("The version."),
                "milestone": text("Milestone title."),
            }),
            &["project", "version", "milestone"],
        ),
        tool(
            "move_roadmap_entry",
            "Move a version or milestone section (with its items) before another entry; without before, to the end.",
            json!({
                "project": project(),
                "entry": text("Version or title of the entry to move."),
                "before": nullable("Version or title of the entry to move in front of; null = to the end."),
            }),
            &["project", "entry"],
        ),
        tool(
            "add_milestone_item",
            "Add a checkbox item under a milestone.",
            json!({"project": project(), "milestone": text("Milestone title."), "text": text("Item text.")}),
            &["project", "milestone", "text"],
        ),
        tool(
            "complete_milestone_item",
            "Tick or untick a milestone item.",
            json!({"project": project(), "id": text("Item id from get_roadmap."), "done": flag("true to complete.")}),
            &["project", "id", "done"],
        ),
        tool(
            "list_code_todos",
            "TODO, FIXME, HACK and XXX comments in the tracked source files (read-only).",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "promote_code_todo",
            "Add a code TODO to the TODO file as \"text (file:line)\". The source comment is not changed.",
            json!({
                "project": project(),
                "file": text("File relative to the checkout, as list_code_todos shows it."),
                "line": int("1-based line."),
                "section": text("Target ## section (default: Code TODOs)."),
            }),
            &["project", "file", "line"],
        ),
        tool("count_lines", "Lines of code per language in the primary checkout (tokei).", json!({"project": project()}), &["project"]),
        tool("list_notes", "Notes without bodies. Without a project, the global notes.", json!({"project": project()}), &[]),
        tool("read_note", "One note with its body.", json!({"id": int("Note id.")}), &["id"]),
        tool(
            "write_note",
            "Create a note (no id; project optional, global without it) or replace the title and body of one.",
            json!({
                "id": int("Note id to update; omit to create."),
                "project": project(),
                "title": text("Title."),
                "body": text("Markdown body; \"\" to empty it."),
            }),
            &["title", "body"],
        ),
        tool("delete_note", "Delete a note by id.", json!({"id": int("Note id.")}), &["id"]),
        tool(
            "git_status",
            "Fresh git state of the primary checkout: branch, changed files, ahead/behind, last commit.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "git_log",
            "The latest commits of the primary checkout, newest first.",
            json!({"project": project(), "limit": int("How many (default 20, at most 200).")}),
            &["project"],
        ),
        tool("list_branches", "The checked-out branch and the local branches.", json!({"project": project()}), &["project"]),
        // --- M4: git, GitHub, tasks, dependencies ---
        tool(
            "git_fetch",
            "git fetch --all --prune in the primary checkout; returns git's output.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "git_pull",
            "git pull --ff-only in the primary checkout (never creates a merge commit; a diverged branch is reported).",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "git_push",
            "PUBLISHES: git push of the checked-out branch (with -u origin on its first push). Only works when publishing is enabled in the app settings.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "switch_branch",
            "git switch <branch> in the primary checkout; git refuses when local changes would be lost.",
            json!({"project": project(), "branch": text("Branch name.")}),
            &["project", "branch"],
        ),
        tool(
            "create_worktree",
            "git worktree add: a new folder with the branch checked out, tracked as a checkout of the project.",
            json!({
                "project": project(),
                "branch": text("Branch to check out there."),
                "new": flag("Create the branch from the current commit (default false: an existing branch)."),
                "path": text("Full path of the new folder (default: next to the checkout, <folder>-<branch>)."),
            }),
            &["project", "branch"],
        ),
        tool(
            "github_info",
            "The project's GitHub repository through gh: visibility, stars, open issues and pull requests, default branch, latest release, CI state of the default branch.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "list_issues",
            "GitHub issues of the project (gh), with linked: the TODO item ids whose line ends with (#number).",
            json!({"project": project(), "state": choice("open (default), closed or all.", &["open", "closed", "all"])}),
            &["project"],
        ),
        tool(
            "create_issue",
            "PUBLISHES: open a GitHub issue. With item_id, from that TODO item (title = its first line, body = the rest) and (#n) is written on the item's line; else from title and body. Only works when publishing is enabled in the app settings.",
            json!({
                "project": project(),
                "item_id": text("TODO item id from list_todos."),
                "title": text("Issue title (without item_id)."),
                "body": text("Issue body (without item_id)."),
                "labels": texts("Labels; dropped when the repository lacks them."),
            }),
            &["project"],
        ),
        tool(
            "close_issue",
            "PUBLISHES: close a GitHub issue as completed; complete_todo also completes the TODO items linked to it. Only works when publishing is enabled in the app settings.",
            json!({"project": project(), "number": int("Issue number."), "complete_todo": flag("Also complete the linked TODO items (default false).")}),
            &["project", "number"],
        ),
        tool(
            "import_issues",
            "Add the open GitHub issues that no TODO item links to yet as items \"title (#n)\" to a section of the TODO file.",
            json!({"project": project(), "section": text("Target ## section (default Issues).")}),
            &["project"],
        ),
        tool(
            "create_github_repo",
            "PUBLISHES: gh repo create from the primary checkout (private unless private is false), added as origin, pushed unless push is false. Only for a repository without an origin remote; only works when publishing is enabled in the app settings. Confirm with the user first.",
            json!({
                "project": project(),
                "name": text("Repository name (default: the folder name)."),
                "owner": text("User or organisation (default: the signed-in user)."),
                "private": flag("Default true."),
                "push": flag("Push the current branch (default true)."),
            }),
            &["project"],
        ),
        tool(
            "list_tasks",
            "The project's tasks (named commands kept in the app) and commands its manifests suggest (cargo, package.json scripts, docker compose).",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "add_task",
            "Add a named task (a shell command line) to the project.",
            json!({
                "project": project(),
                "name": text("Task name."),
                "command": text("Command line, run by cmd /C on Windows (sh -c elsewhere)."),
                "cwd": text("Folder relative to the checkout (default the checkout)."),
                "env": text("KEY=VALUE lines."),
            }),
            &["project", "name", "command"],
        ),
        tool(
            "remove_task",
            "Delete a task of the project by id.",
            json!({"project": project(), "id": int("Task id from list_tasks.")}),
            &["project", "id"],
        ),
        tool(
            "run_task",
            "RUNS A COMMAND on this computer: a task by id or name in the primary checkout. Waits until it ends or timeout_secs (default 120, at most 600) pass, then stops it; returns the exit code and the last 300 output lines.",
            json!({"project": project(), "task": json!({"type": ["integer", "string"], "description": "Task id or name."}), "timeout_secs": int("Seconds to wait (default 120).")}),
            &["project", "task"],
        ),
        tool(
            "list_dependencies",
            "What the project uses (other tracked projects at a version) and what uses it, with the used and latest versions read now and standing: current, behind, ahead, differs or unknown.",
            json!({"project": project()}),
            &["project"],
        ),
        tool(
            "set_dependency",
            "Add or replace \"project uses depends_on\": the used version from used_fixed, or from used_file read with used_pattern (a regex, first group = version); the latest from latest_tag (a tag glob such as spec-v*) or else the other project's version.",
            json!({
                "project": project(),
                "depends_on": project(),
                "used_file": text("File in this project's checkout."),
                "used_pattern": text("Regex with one capture group, e.g. spec-v([0-9.]+)."),
                "used_fixed": text("A fixed used version instead of file and pattern."),
                "latest_tag": text("Glob of the other project's tags; omit for its version."),
            }),
            &["project", "depends_on"],
        ),
        tool(
            "remove_dependency",
            "Remove the dependency of project on depends_on.",
            json!({"project": project(), "depends_on": project()}),
            &["project", "depends_on"],
        ),
        tool(
            "list_claude_items",
            "Claude Code config of a project or of the global ~/.claude: CLAUDE.md, CLAUDE.local.md, AGENTS.md, nested CLAUDE.md, .claude/settings(.local).json, agents, skills (folders), commands, .mcp.json, .claude/launch.json, and other agents' files (read_only). item is the name the other Claude tools take.",
            json!({"project": claude_scope()}),
            &["project"],
        ),
        tool(
            "read_claude_item",
            "One Claude config item: its text (or one markdown section or JSON key of it), with its sections (markdown) or keys (JSON Pointers) to copy parts by. A skill lists its files and shows SKILL.md.",
            json!({
                "project": claude_scope(),
                "item": text("Item name from list_claude_items, e.g. CLAUDE.md, .claude/settings.json, .claude/agents/reviewer.md, .claude/skills/pdf."),
                "section": text("Only this markdown section (heading text without #)."),
                "key": text("Only this JSON value (JSON Pointer, e.g. /permissions/allow/0)."),
            }),
            &["project", "item"],
        ),
        tool(
            "copy_claude_item",
            "Copy a Claude config item, or part of it, between projects and the global ~/.claude (a plain copy, not linked). Whole files and skill folders replace the target's only with overwrite; a section is appended (or replaces the same heading with overwrite); a JSON key is merged (objects by key, arrays gain missing elements; different values are conflicts, replaced only with overwrite). Without apply it returns the diff preview and writes nothing. apply: true OVERWRITES Claude config files (backups first) and only works when the app setting mcp.allow_claude_write is on.",
            json!({
                "from": claude_scope(),
                "to": claude_scope(),
                "item": text("Item name in the source."),
                "to_item": text("Item name in the target when it differs (e.g. .claude/settings.local.json → .claude/settings.json)."),
                "section": text("Copy only this markdown section (heading text)."),
                "key": text("Copy only this JSON value (JSON Pointer from read_claude_item)."),
                "overwrite": flag("Replace what differs in the target (default false: keep it and report a conflict)."),
                "apply": flag("Write the change (default false: preview only). Needs mcp.allow_claude_write."),
            }),
            &["from", "to", "item"],
        ),
        tool(
            "edit_claude_item",
            "Replace the text of a Claude config file (created when missing). JSON files must parse. Without apply it returns the diff preview. apply: true OVERWRITES the file (backup first) and only works when the app setting mcp.allow_claude_write is on.",
            json!({
                "project": claude_scope(),
                "item": text("Item name, e.g. CLAUDE.md or .claude/agents/reviewer.md (a skill's file: .claude/skills/pdf/SKILL.md)."),
                "text": text("The whole new text."),
                "apply": flag("Write it (default false: preview only). Needs mcp.allow_claude_write."),
            }),
            &["project", "item", "text"],
        ),
        tool(
            "list_templates",
            "Project templates in the templates folder (the built-in ones are added when missing): name, description, prompts to answer and steps.",
            json!({}),
            &[],
        ),
        tool(
            "create_project",
            "Create a project from a template: fills the placeholders, RUNS the template's commands (e.g. cargo new, git init) in parent_dir, copies its files, then tracks the new folder. Refused when the folder exists. On a failed step the partly created folder is left in place.",
            json!({
                "template": text("Template folder or name from list_templates."),
                "prompts": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Answers by prompt key; name is required (the new folder's name)."},
                "parent_dir": text("Folder to create the project in."),
                "group": text("Group name (default: the group whose folder is parent_dir)."),
            }),
            &["template", "prompts", "parent_dir"],
        ),
    ]
}

fn claude_scope() -> Value {
    json!({"type": ["integer", "string"], "description": "\"global\" for ~/.claude, or a project id, name or a path inside one of its checkouts."})
}

/// The part of a Claude item named by `section` or `key` (neither: the whole file).
fn claude_part(a: &Args) -> Result<claude::Part, Failure> {
    match (a.opt_str("section")?, a.opt_str("key")?) {
        (Some(_), Some(_)) => Err(Failure::Params("give section or key, not both".into())),
        (Some(s), None) => Ok(claude::Part::Section(s.to_string())),
        (None, Some(k)) => Ok(claude::Part::Key(k.to_string())),
        (None, None) => Ok(claude::Part::Whole),
    }
}

/// A Claude config plan as JSON (diff per file, conflicts); written only with `apply` and the write setting on.
fn claude_write(server: &Server, plan: &claude::Plan, apply: bool, overwrite: bool) -> Result<Out, Failure> {
    let changes: Vec<Value> = plan.changes.iter().map(|c| json!({"file": c.path.display().to_string(), "new_file": !c.existed, "diff": c.diff()})).collect();
    let mut data = json!({"changes": changes, "conflicts": plan.conflicts, "applied": false, "backups": []});
    let kept = match (plan.conflicts.len(), overwrite) {
        (0, _) => String::new(),
        (n, true) => format!("; {n} conflict(s) replaced"),
        (n, false) => format!("; {n} conflict(s) kept (overwrite: true replaces them)"),
    };
    if plan.changes.is_empty() {
        return out(format!("nothing to change{kept}"), data);
    }
    if !apply {
        return out(format!("preview: {} file(s) would change{kept}; apply: true writes", plan.changes.len()), data);
    }
    if !claude::write_allowed(&server.store)? {
        return Err(Failure::Tool(format!(
            "writing Claude config over MCP is off: set the app setting {} to true (Settings in ProjectArchitect) to allow it; nothing was written",
            claude::ALLOW_WRITE
        )));
    }
    let backups = claude::apply(&server.data_dir, plan)?;
    data["applied"] = json!(true);
    data["backups"] = json!(backups.iter().map(|b| b.display().to_string()).collect::<Vec<_>>());
    out(format!("wrote {} file(s){kept}", plan.changes.len()), data)
}

fn template_json(t: &template::Template) -> Value {
    json!({
        "folder": t.folder(),
        "name": t.name,
        "description": t.description,
        "purpose": t.purpose,
        "tags": t.tags,
        "prompts": t.questions().iter().map(|p| json!({"key": p.key, "label": p.label, "default": p.default})).collect::<Vec<_>>(),
        "steps": t.steps.iter().map(|s| match &s.run {
            Some(r) => json!({"run": r, "cwd": s.cwd}),
            None => json!({"overlay": s.overlay}),
        }).collect::<Vec<_>>(),
    })
}

// --- arguments ------------------------------------------------------------------------------------------------

struct Args<'a>(&'a Map<String, Value>);

fn required(k: &str) -> Failure {
    Failure::Params(format!("\"{k}\" is required"))
}

impl Args<'_> {
    /// A project reference as a string (numbers are ids).
    fn key(&self, k: &str) -> Result<String, Failure> {
        match self.0.get(k) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(Value::Number(n)) => Ok(n.to_string()),
            _ => Err(Failure::Params(format!("\"{k}\" must be a project id, name or path"))),
        }
    }

    /// `None` when absent, `Some(None)` when null, `Some(Some(s))` for a string.
    fn nullable(&self, k: &str) -> Result<Option<Option<&str>>, Failure> {
        match self.0.get(k) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(s)) => Ok(Some(Some(s.as_str()))),
            Some(_) => Err(Failure::Params(format!("\"{k}\" must be a string"))),
        }
    }

    fn opt_str(&self, k: &str) -> Result<Option<&str>, Failure> {
        Ok(self.nullable(k)?.flatten())
    }

    fn req_str(&self, k: &str) -> Result<&str, Failure> {
        self.opt_str(k)?.ok_or_else(|| required(k))
    }

    fn opt_bool(&self, k: &str) -> Result<Option<bool>, Failure> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(Failure::Params(format!("\"{k}\" must be true or false"))),
        }
    }

    fn req_bool(&self, k: &str) -> Result<bool, Failure> {
        self.opt_bool(k)?.ok_or_else(|| required(k))
    }

    fn opt_int(&self, k: &str) -> Result<Option<i64>, Failure> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_i64().map(Some).ok_or_else(|| Failure::Params(format!("\"{k}\" must be an integer"))),
        }
    }

    fn req_int(&self, k: &str) -> Result<i64, Failure> {
        self.opt_int(k)?.ok_or_else(|| required(k))
    }

    fn opt_strings(&self, k: &str) -> Result<Option<Vec<String>>, Failure> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or_else(|| Failure::Params(format!("\"{k}\" must be a list of strings"))))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(Failure::Params(format!("\"{k}\" must be a list of strings"))),
        }
    }
}

fn parse_style(s: &str) -> Result<Style, Failure> {
    Style::decode(s).ok_or_else(|| Failure::Params(format!("unknown TODO style {s:?}. {STYLE_HELP}")))
}

fn parse_status(s: &str) -> Result<Status, Failure> {
    Status::parse(s).ok_or_else(|| Failure::Params(format!("status must be one of {}", STATUSES.join(", "))))
}

fn parse_milestone_status(s: &str) -> Result<MilestoneStatus, Failure> {
    match s {
        "planned" => Ok(MilestoneStatus::Planned),
        "in progress" => Ok(MilestoneStatus::InProgress),
        "done" => Ok(MilestoneStatus::Done),
        "released" => Ok(MilestoneStatus::Released),
        _ => Err(Failure::Params("status must be planned, in progress, done or released".into())),
    }
}

fn project_of(server: &Server, a: &Args) -> Result<Project, Failure> {
    Ok(ops::find_project(&server.store, &a.key("project")?)?)
}

// --- JSON shapes ----------------------------------------------------------------------------------------------

fn out(summary: impl Into<String>, data: Value) -> Result<Out, Failure> {
    Ok(Out { summary: summary.into(), data })
}

fn summary(p: &Project, cat: &Catalog) -> Value {
    let info = p.info();
    let git = info.and_then(|i| i.git.as_ref());
    json!({
        "id": p.id,
        "name": p.name,
        "group": cat.group_name(p.group_id),
        "purpose": cat.purpose(p.purpose_id).map(|x| x.name.as_str()),
        "status": p.status.as_str(),
        "tags": p.tags,
        "path": p.primary().map(|c| c.path.as_str()),
        "version": info.and_then(|i| i.version.as_deref()),
        "languages": info.map(|i| i.languages.iter().take(3).map(|(l, _)| l.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
        "branch": git.and_then(|g| g.branch.as_deref()),
        "dirty": git.map(|g| g.dirty),
        "last_commit": git.and_then(|g| g.last_commit.as_deref()),
        "github_url": git.and_then(|g| g.remote.as_deref()).and_then(info::github_url),
        "todo_open": info.and_then(|i| i.todo_open),
    })
}

fn item_json(i: &TodoItem) -> Value {
    json!({"id": i.id, "section": i.section, "depth": i.depth, "done": i.done, "text": i.text, "line": i.line + 1})
}

/// The item whose text is `text` (ids change with text and section, so writes re-find items this way).
// ponytail: the first item with that text wins when two share it.
fn by_text<'a>(doc: &'a TodoDoc, text: &str) -> Option<&'a TodoItem> {
    doc.items.iter().find(|i| i.text == text.trim())
}

fn text_of(doc: &TodoDoc, id: &str) -> ops::Result<String> {
    doc.item(id).map(|i| i.text.clone()).ok_or_else(|| ops::Error(format!("no item with id {id}")))
}

fn todo_out(what: &str, doc: &TodoDoc, item: Option<&TodoItem>) -> Result<Out, Failure> {
    let open = doc.open_count();
    out(format!("{what}; {open} open"), json!({"item": item.map_or(Value::Null, item_json), "open": open}))
}

/// An entry's name in lists: its version if it has one, else its title.
fn entry_label(m: &Milestone) -> String {
    m.version.clone().unwrap_or_else(|| m.title.clone())
}

fn entry_json(m: &Milestone) -> Value {
    json!({
        "kind": match m.kind { EntryKind::Version => "version", EntryKind::Milestone => "milestone" },
        "version": m.version,
        "title": m.title,
        "status": m.status.label(),
        "status_note": m.status_note,
        "effective": m.effective.label(),
        "explicit_status": m.explicit_status,
        "status_note": m.status_note,
        "warning": m.warning,
        "links": m.links,
        "target": m.target,
        "progress": {"done": m.done, "total": m.total},
        "body": m.body,
        "items": m.items.iter().map(item_json).collect::<Vec<_>>(),
    })
}

/// The entry's JSON, or `null` when no entry has that name.
fn entry_named(r: &roadmap::Roadmap, name: &str) -> Value {
    r.index_of(name).map_or(Value::Null, |i| entry_json(&r.milestones[i]))
}

fn loc_json(r: &LocRow) -> Value {
    json!({"language": r.language, "files": r.files, "code": r.code, "comments": r.comments, "blanks": r.blanks})
}

fn note_json(n: &pa_core::Note) -> Value {
    json!({"id": n.id, "project_id": n.project_id, "title": n.title, "updated_at": n.updated_at})
}

/// Publishing tools (push, repositories, issues) run only when the user enabled publishing.
fn publishing(store: &Store) -> Result<(), Failure> {
    if ops::publish_allowed(store)? {
        return Ok(());
    }
    Err(Failure::Tool(format!(
        "publishing is off: set the app setting {} to true (Settings in ProjectArchitect) to allow pushing and creating repositories or issues",
        ops::ALLOW_PUBLISH
    )))
}

/// A git command's result: its output, or a tool failure carrying it.
fn git_out(o: git::Output) -> Result<Out, Failure> {
    if !o.ok {
        return Err(Failure::Tool(format!("{} failed: {}", o.command, o.text)));
    }
    let first = o.text.lines().last().unwrap_or("done").to_string();
    out(format!("{}: {first}", o.command), json!({"command": o.command, "output": o.text}))
}

fn task_json(t: &Task) -> Value {
    json!({"id": t.id, "name": t.name, "command": t.command, "cwd": t.cwd, "env": t.env})
}

/// A task of the project by id or name (case-insensitive).
fn find_task(store: &Store, p: &Project, key: &str) -> Result<Task, Failure> {
    let key = key.trim();
    let list = store.tasks(p.id)?;
    list.iter()
        .find(|t| t.id.to_string() == key)
        .or_else(|| list.iter().find(|t| t.name.eq_ignore_ascii_case(key)))
        .cloned()
        .ok_or_else(|| Failure::Tool(format!("{} has no task {key:?}; list_tasks lists them", p.name)))
}

fn standing(s: Standing) -> &'static str {
    match s {
        Standing::Current => "current",
        Standing::Behind => "behind",
        Standing::Ahead => "ahead",
        Standing::Differs => "differs",
        Standing::Unknown => "unknown",
    }
}

// --- dispatch -------------------------------------------------------------------------------------------------

/// Run a tool with its arguments.
pub fn call(server: &Server, name: &str, args: &Map<String, Value>) -> Result<Out, Failure> {
    let a = Args(args);
    let store = &server.store;
    match name {
        "current_project" => {
            let path = a.req_str("path")?;
            let projects = store.projects()?;
            let p = ops::project_for_path(&projects, Path::new(path))
                .ok_or_else(|| Failure::Tool(format!("no tracked project has a checkout containing {path}")))?;
            out(format!("{} (#{})", p.name, p.id), json!({"project": summary(p, &Catalog::load(store)?)}))
        }
        "list_projects" => {
            let query = a.opt_str("query")?.map(str::to_lowercase);
            let group = a.opt_str("group")?;
            let purpose = a.opt_str("purpose")?;
            let status = a.opt_str("status")?.map(parse_status).transpose()?;
            let changes = a.opt_bool("has_changes")?;
            let cat = Catalog::load(store)?;
            let mut rows = Vec::new();
            for p in store.projects()? {
                let hay = format!("{} {} {}", p.name, p.tags.join(" "), p.primary().map_or("", |c| c.path.as_str())).to_lowercase();
                let dirty = p.info().and_then(|i| i.git.as_ref()).is_some_and(|g| g.dirty > 0);
                let in_group = group.is_none_or(|g| cat.group_name(p.group_id).is_some_and(|n| n.eq_ignore_ascii_case(g)));
                let in_purpose = purpose.is_none_or(|x| cat.purpose(p.purpose_id).is_some_and(|n| n.name.eq_ignore_ascii_case(x)));
                let keep = query.as_deref().is_none_or(|q| hay.contains(q))
                    && in_group
                    && in_purpose
                    && status.is_none_or(|s| s == p.status)
                    && changes.is_none_or(|c| c == dirty);
                if keep {
                    rows.push(summary(&p, &cat));
                }
            }
            out(format!("{} projects", rows.len()), json!({"projects": rows}))
        }
        "get_project" => {
            let key = a.key("project")?;
            let mut p = ops::find_project(store, &key)?;
            if a.opt_bool("refresh")? == Some(true) {
                for c in &p.checkouts {
                    store.set_info(c.id, &info::collect(Path::new(&c.path)))?;
                }
                p = ops::find_project(store, &p.id.to_string())?;
            }
            let mut v = summary(&p, &Catalog::load(store)?);
            v["description"] = json!(p.info().and_then(|i| i.description.as_deref()));
            v["checkouts"] = json!(
                p.checkouts
                    .iter()
                    .map(|c| json!({
                        "path": c.path,
                        "primary": c.primary,
                        "info_at": c.info_at,
                        "branch": c.info.as_ref().and_then(|i| i.git.as_ref()).and_then(|g| g.branch.clone()),
                    }))
                    .collect::<Vec<_>>()
            );
            v["todo_file"] = json!(ops::todo_path(&p)?.map(|x| x.display().to_string()));
            v["roadmap_file"] = json!(ops::roadmap_path(&p)?.map(|x| x.display().to_string()));
            v["tag_prefix"] = json!(ops::tag_prefix(&p));
            v["todo_style"] = match p.todo_style.as_deref().and_then(Style::decode) {
                Some(s) => json!({"label": s.label(), "source": "stored"}),
                None => ops::load_todo(&p).map_or(Value::Null, |f| json!({"label": f.doc.style.label(), "source": "detected"})),
            };
            out(format!("{} (#{}), {} checkout(s)", p.name, p.id, p.checkouts.len()), v)
        }
        "update_project" => {
            let mut p = project_of(server, &a)?;
            let group_id = match a.nullable("group")? {
                Some(Some(g)) => Some(Some(ops::group_named(store, g)?)),
                Some(None) => Some(None),
                None => None,
            };
            let purpose_id = match a.nullable("purpose")? {
                Some(Some(x)) => Some(Some(ops::purpose_named(store, x)?)),
                Some(None) => Some(None),
                None => None,
            };
            let todo_style = match a.nullable("todo_style")? {
                None => None,
                Some(None) | Some(Some("detect")) => Some(None),
                Some(Some(s)) => Some(Some(parse_style(s)?.encode())),
            };
            let edit = ProjectEdit {
                name: a.opt_str("name")?.map(str::to_string),
                group_id,
                purpose_id,
                status: a.opt_str("status")?.map(parse_status).transpose()?,
                tags: a.opt_strings("tags")?,
                todo_style,
                tag_prefix: a.nullable("tag_prefix")?.map(|v| v.map(str::to_string)),
            };
            ops::edit_project(store, &mut p, edit)?;
            out(format!("updated {} (#{})", p.name, p.id), json!({"project": summary(&p, &Catalog::load(store)?)}))
        }
        "list_groups" => {
            let projects = store.projects()?;
            let rows: Vec<Value> = store
                .groups()?
                .iter()
                .map(|g| {
                    let count = projects.iter().filter(|p| p.group_id == Some(g.id)).count();
                    json!({"id": g.id, "name": g.name, "path": g.path, "projects": count})
                })
                .collect();
            out(format!("{} groups", rows.len()), json!({"groups": rows}))
        }
        "list_purposes" => {
            let rows: Vec<Value> = store
                .purposes()?
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "name": p.name,
                        "track_todos": p.track_todos,
                        "watch_upstream": p.watch_upstream,
                        "show_in_overview": p.show_in_overview,
                        "scan_code_todos": p.scan_code_todos,
                    })
                })
                .collect();
            out(format!("{} purposes", rows.len()), json!({"purposes": rows}))
        }
        "scan" => {
            let roots: Option<Vec<(String, u32)>> = a.opt_strings("roots")?.map(|r| r.into_iter().map(|p| (p, 3)).collect());
            let found = ops::scan_folders(store, roots)?;
            let candidates: Vec<Value> = found
                .candidates
                .iter()
                .map(|c| json!({"path": c.path, "name": c.name, "git": c.git, "remote": c.remote, "root_commit": c.root_commit}))
                .collect();
            let groups: Vec<Value> =
                found.groups.iter().map(|g| json!({"path": g.path, "name": g.name, "curated": g.curated, "members": g.members})).collect();
            out(
                format!("{} candidates, {} group suggestions; nothing imported", candidates.len(), groups.len()),
                json!({"candidates": candidates, "groups": groups}),
            )
        }
        "import_folder" => {
            let path = a.req_str("path")?;
            let imported = ops::import_folder(store, Path::new(path), a.opt_str("name")?, a.opt_str("group")?, a.opt_str("purpose")?)?;
            let p = ops::find_project(store, &imported.project_id.to_string())?;
            let verb = if imported.created { "tracked as a new project" } else { "added as a checkout" };
            let cat = Catalog::load(store)?;
            out(format!("{path} {verb}: {} (#{})", p.name, p.id), json!({"created": imported.created, "project": summary(&p, &cat)}))
        }
        "list_todos" => {
            let p = project_of(server, &a)?;
            let file = ops::load_todo(&p)?;
            let section = a.opt_str("section")?;
            let include_done = a.opt_bool("include_done")?.unwrap_or(false);
            let items: Vec<Value> =
                file.doc.items.iter().filter(|i| (include_done || !i.done) && section.is_none_or(|s| i.section == s)).map(item_json).collect();
            let open = file.doc.open_count();
            out(
                format!("{} items ({open} open) in {}", items.len(), file.path.display()),
                json!({"file": file.path.display().to_string(), "style": file.doc.style.label(), "open": open, "items": items}),
            )
        }
        "add_todo" => {
            let p = project_of(server, &a)?;
            let item = a.req_str("text")?;
            let section = a.opt_str("section")?;
            let parent = a.opt_str("parent_id")?;
            let (id, doc) = ops::edit_todo(&p, |doc| {
                let id = match parent {
                    Some(pid) => doc.add_child(pid, item)?,
                    None => {
                        // Default: where the open items are; never the "move to section" style's done section,
                        // or the new item would be born done.
                        let done = match &doc.style.completion {
                            pa_core::todo::Completion::MoveTo(s) => Some(s.clone()),
                            _ => None,
                        };
                        let first = doc
                            .items
                            .iter()
                            .find(|i| !i.done)
                            .map(|i| i.section.clone())
                            .or_else(|| doc.sections.iter().map(|s| s.title.clone()).find(|t| Some(t) != done.as_ref()))
                            .unwrap_or_default();
                        let section = section.map_or(first, str::to_string);
                        doc.add(&section, item)?
                    }
                };
                Ok(id)
            })?;
            todo_out("added", &doc, doc.item(&id))
        }
        "update_todo" => {
            let p = project_of(server, &a)?;
            let id = a.req_str("id")?;
            let item = a.req_str("text")?;
            let ((), doc) = ops::edit_todo(&p, |doc| {
                doc.set_text(id, item)?;
                Ok(())
            })?;
            todo_out("updated", &doc, by_text(&doc, item))
        }
        "complete_todo" => {
            let p = project_of(server, &a)?;
            let id = a.req_str("id")?;
            let done = a.req_bool("done")?;
            let (text, doc) = ops::edit_todo(&p, |doc| {
                let text = text_of(doc, id)?;
                doc.set_done(id, done)?;
                Ok(text)
            })?;
            let what = match github::issue_ref(&text).filter(|_| done) {
                Some(n) => format!("completed (linked to issue #{n}; close_issue closes it)"),
                None => (if done { "completed" } else { "reopened" }).to_string(),
            };
            todo_out(&what, &doc, by_text(&doc, &text))
        }
        "move_todo" => {
            let p = project_of(server, &a)?;
            let id = a.req_str("id")?;
            let section = a.opt_str("section")?;
            let delta = a.opt_int("delta")?;
            if section.is_none() && delta.is_none() {
                return Err(Failure::Params("give section or delta".into()));
            }
            let delta = delta.map(|d| i32::try_from(d).map_err(|_| Failure::Params("delta is out of range".into()))).transpose()?;
            let (text, doc) = ops::edit_todo(&p, |doc| {
                let text = text_of(doc, id)?;
                if let Some(s) = section {
                    doc.move_to(id, s)?;
                } else if let Some(d) = delta {
                    doc.move_by(id, d)?;
                }
                Ok(text)
            })?;
            todo_out("moved", &doc, by_text(&doc, &text))
        }
        "remove_todo" => {
            let p = project_of(server, &a)?;
            let id = a.req_str("id")?;
            let (text, doc) = ops::edit_todo(&p, |doc| {
                let text = text_of(doc, id)?;
                doc.remove(id)?;
                Ok(text)
            })?;
            out(format!("removed {text:?}; {} open", doc.open_count()), json!({"removed": text, "open": doc.open_count()}))
        }
        "capture" => {
            let p = project_of(server, &a)?;
            let kind = a.req_str("kind")?;
            let kind = ops::CaptureKind::parse(kind).ok_or_else(|| Failure::Params(format!("unknown kind {kind:?}: Idea, Bug, TODO or Note")))?;
            let (captured, issue) = github::capture(store, &p, kind, a.req_str("title")?, a.opt_str("details")?.unwrap_or(""), ops::publish_allowed(store)?)?;
            let also = match &issue {
                Some(Ok(n)) => format!(" and as issue #{n}"),
                Some(Err(e)) => format!("; no issue: {e}"),
                None => String::new(),
            };
            let (number, error) = match issue {
                Some(Ok(n)) => (Some(n), None),
                Some(Err(e)) => (None, Some(e)),
                None => (None, None),
            };
            match captured {
                ops::Captured::Todo { file, id } => out(
                    format!("captured in {}{also}", file.display()),
                    json!({"file": file.display().to_string(), "id": id, "issue": number, "issue_error": error}),
                ),
                ops::Captured::Note(n) => out(format!("captured as note {}: {}", n.id, n.title), note_json(&n)),
            }
        }
        "create_todo_file" => {
            let p = project_of(server, &a)?;
            let path = ops::create_todo_file(&p)?;
            out(format!("created {}", path.display()), json!({"file": path.display().to_string()}))
        }
        "list_todos_all" => {
            let query = pa_core::query::Query::parse(a.opt_str("query")?.unwrap_or(""));
            let status = a.opt_str("status")?.unwrap_or(if query.has_status() { "all" } else { "open" });
            if !matches!(status, "open" | "done" | "all") {
                return Err(Failure::Params("status must be open, done or all".into()));
            }
            let source = a.opt_str("source")?;
            if !matches!(source, None | Some("todo" | "roadmap")) {
                return Err(Failure::Params("source must be todo or roadmap".into()));
            }
            let only = a.opt_str("project")?.map(|k| ops::find_project(store, k).map(|p| p.id)).transpose()?;
            let group = a.opt_str("group")?;
            let purpose = a.opt_str("purpose")?;
            let cat = Catalog::load(store)?;
            let wanted = |item: &TodoItem, project: &str, roadmap: bool| {
                let status_ok = match status {
                    "open" => !item.done,
                    "done" => item.done,
                    _ => true,
                };
                let view = pa_core::query::Item { project, section: &item.section, text: &item.text, done: item.done, roadmap };
                status_ok && query.matches(&view)
            };
            let mut rows = Vec::new();
            for p in store.projects()? {
                let in_group = group.is_none_or(|g| cat.group_name(p.group_id).is_some_and(|n| n.eq_ignore_ascii_case(g)));
                let in_purpose = purpose.is_none_or(|x| cat.purpose(p.purpose_id).is_some_and(|n| n.name.eq_ignore_ascii_case(x)));
                if !(only.is_none_or(|id| id == p.id) && in_group && in_purpose) {
                    continue;
                }
                // A project without a readable file has no items to list from it.
                if source != Some("roadmap")
                    && let Ok(file) = ops::load_todo(&p)
                {
                    for item in file.doc.items.iter().filter(|i| wanted(i, &p.name, false)) {
                        let mut v = item_json(item);
                        v["source"] = json!("todo");
                        v["entry"] = Value::Null;
                        v["project"] = json!(p.name);
                        v["project_id"] = json!(p.id);
                        rows.push(v);
                    }
                }
                if source != Some("todo")
                    && let Ok(file) = ops::load_roadmap(&p)
                {
                    for entry in &file.roadmap.milestones {
                        for item in entry.items.iter().filter(|i| wanted(i, &p.name, true)) {
                            let mut v = item_json(item);
                            v["source"] = json!("roadmap");
                            v["entry"] = json!(entry_label(entry));
                            v["project"] = json!(p.name);
                            v["project_id"] = json!(p.id);
                            rows.push(v);
                        }
                    }
                }
            }
            out(format!("{} items", rows.len()), json!({"items": rows}))
        }
        "change_todo_style" => {
            let mut p = project_of(server, &a)?;
            let style = parse_style(a.req_str("style")?)?;
            let target = match a.req_str("mode")? {
                "working_copy" => StyleTarget::WorkingCopy,
                "branch" => StyleTarget::Branch {
                    name: a.req_str("branch")?.to_string(),
                    base: a.opt_str("base")?.map(str::to_string),
                    push_and_pr: a.opt_bool("push_and_pr")?.unwrap_or(false),
                },
                other => return Err(Failure::Params(format!("mode must be working_copy or branch, not {other:?}"))),
            };
            let c = ops::change_todo_style(store, &server.data_dir, &mut p, &style, target)?;
            let what = match &c.branch {
                Some(b) => format!("committed to {b}"),
                None => "rewritten in the working copy".to_string(),
            };
            let pr = c.pr_url.as_deref().map(|u| format!(", pull request {u}")).unwrap_or_default();
            out(
                format!("TODO style is now {}: {what}{pr}", style.label()),
                json!({
                    "file": c.file.display().to_string(),
                    "style": c.style,
                    "backup": c.backup.map(|b| b.display().to_string()),
                    "branch": c.branch,
                    "commit": c.commit,
                    "pr_url": c.pr_url,
                }),
            )
        }
        "get_roadmap" => {
            let p = project_of(server, &a)?;
            let file = ops::load_roadmap(&p)?;
            let r = &file.roadmap;
            let entries: Vec<Value> = r.milestones.iter().map(entry_json).collect();
            let lanes = roadmap::board(r);
            let lane = |ix: &[usize]| -> Vec<String> { ix.iter().map(|&i| entry_label(&r.milestones[i])).collect() };
            let tags = ops::tags(&p);
            let suggestions = roadmap::release_suggestions(r, &tags, ops::tag_prefix(&p));
            out(
                format!("{} entries in {}", entries.len(), file.path.display()),
                json!({
                    "file": file.path.display().to_string(),
                    "editable": r.editable,
                    "milestones": entries,
                    "board": {"now": lane(&lanes.now), "next": lane(&lanes.next), "later": lane(&lanes.later), "done": lane(&lanes.done)},
                    "release_suggestions": suggestions,
                }),
            )
        }
        "add_milestone" => {
            let p = project_of(server, &a)?;
            let title = a.req_str("title")?;
            let status = parse_milestone_status(a.opt_str("status")?.unwrap_or("planned"))?;
            let ((), r) = ops::edit_roadmap(&p, |r| {
                r.add_milestone(title, status)?;
                Ok(())
            })?;
            out(format!("added milestone {title:?}"), json!({"milestone": entry_named(&r, title)}))
        }
        "add_version" => {
            let p = project_of(server, &a)?;
            let version = a.req_str("version")?;
            let title = a.opt_str("title")?.unwrap_or("");
            let status = parse_milestone_status(a.opt_str("status")?.unwrap_or("planned"))?;
            let milestone = a.opt_str("milestone")?;
            let ((), r) = ops::edit_roadmap(&p, |r| {
                r.add_version(version, title, status, milestone)?;
                Ok(())
            })?;
            out(format!("added version {version}"), json!({"entry": entry_named(&r, version)}))
        }
        "link_version" => {
            let p = project_of(server, &a)?;
            let version = a.req_str("version")?;
            let milestone = a.req_str("milestone")?;
            let ((), r) = ops::edit_roadmap(&p, |r| {
                r.link(version, milestone)?;
                Ok(())
            })?;
            out(format!("{version} is in {milestone:?}"), json!({"entry": entry_named(&r, version)}))
        }
        "move_roadmap_entry" => {
            let p = project_of(server, &a)?;
            let entry = a.req_str("entry")?;
            let before = a.opt_str("before")?;
            let ((), r) = ops::edit_roadmap(&p, |r| {
                r.move_entry(entry, before)?;
                Ok(())
            })?;
            let order: Vec<String> = r.milestones.iter().map(entry_label).collect();
            out(format!("moved {entry:?}"), json!({"order": order}))
        }
        "set_milestone_status" => {
            let p = project_of(server, &a)?;
            let title = a.req_str("title")?;
            let status = parse_milestone_status(a.req_str("status")?)?;
            let label = status.label().to_string();
            let ((), r) = ops::edit_roadmap(&p, |r| {
                r.set_status(title, status)?;
                Ok(())
            })?;
            out(format!("{title:?} is now {label}"), json!({"milestone": entry_named(&r, title)}))
        }
        "add_milestone_item" => {
            let p = project_of(server, &a)?;
            let milestone = a.req_str("milestone")?;
            let item = a.req_str("text")?;
            let (id, r) = ops::edit_roadmap(&p, |r| {
                // Items go under the entry's heading (a version's heading is not its title).
                let heading = match r.index_of(milestone) {
                    Some(i) => r.milestones[i].heading.clone(),
                    None => return Err(ops::Error(format!("no milestone titled {milestone:?}"))),
                };
                let id = r.doc.add(&heading, item)?;
                Ok(id)
            })?;
            out(format!("added to {milestone:?}"), json!({"item": r.doc.item(&id).map(item_json)}))
        }
        "complete_milestone_item" => {
            let p = project_of(server, &a)?;
            let id = a.req_str("id")?;
            let done = a.req_bool("done")?;
            let ((), r) = ops::edit_roadmap(&p, |r| {
                r.doc.set_done(id, done)?;
                Ok(())
            })?;
            out(if done { "completed" } else { "reopened" }, json!({"item": r.doc.item(id).map(item_json)}))
        }
        "list_code_todos" => {
            let p = project_of(server, &a)?;
            let rows: Vec<Value> = codetodo::scan(&ops::primary_root(&p)?)
                .iter()
                .map(|c| json!({"file": c.file, "line": c.line, "keyword": c.keyword, "text": c.text}))
                .collect();
            out(format!("{} code TODOs", rows.len()), json!({"todos": rows}))
        }
        "promote_code_todo" => {
            let p = project_of(server, &a)?;
            let file = a.req_str("file")?;
            let line = usize::try_from(a.req_int("line")?).map_err(|_| Failure::Params("line must be 1 or more".into()))?;
            let section = a.opt_str("section")?.unwrap_or("Code TODOs");
            let found = ops::code_todo_at(&p, file, line)?
                .ok_or_else(|| Failure::Tool(format!("no code TODO at {file}:{line}; list_code_todos lists them")))?;
            let what = if found.text.is_empty() { found.keyword.as_str() } else { found.text.as_str() };
            let item = format!("{what} ({file}:{line})");
            let (id, doc) = ops::edit_todo(&p, |doc| {
                let id = doc.add(section, &item)?;
                Ok(id)
            })?;
            todo_out("promoted", &doc, doc.item(&id))
        }
        "count_lines" => {
            let p = project_of(server, &a)?;
            let rows = loc::count(&ops::primary_root(&p)?);
            let total = loc::total(&rows);
            let languages: Vec<Value> = rows.iter().map(loc_json).collect();
            out(format!("{} lines of code in {} languages", total.code, rows.len()), json!({"languages": languages, "total": loc_json(&total)}))
        }
        "list_notes" => {
            let scope = match a.opt_str("project")? {
                Some(k) => Some(ops::find_project(store, k)?.id),
                None => None,
            };
            let rows: Vec<Value> = store.notes(scope)?.iter().map(note_json).collect();
            out(format!("{} notes", rows.len()), json!({"notes": rows}))
        }
        "read_note" => {
            let n = ops::find_note(store, a.req_int("id")?)?;
            let mut v = note_json(&n);
            v["body"] = json!(n.body);
            out(format!("note {}: {}", n.id, n.title), v)
        }
        "write_note" => {
            let id = a.opt_int("id")?;
            let project_id = match a.opt_str("project")? {
                Some(k) => Some(ops::find_project(store, k)?.id),
                None => None,
            };
            let n = ops::write_note(store, id, project_id, a.req_str("title")?, a.req_str("body")?)?;
            let mut v = note_json(&n);
            v["body"] = json!(n.body);
            out(format!("saved note {}: {}", n.id, n.title), v)
        }
        "delete_note" => {
            let id = a.req_int("id")?;
            ops::delete_note(store, id)?;
            out(format!("deleted note {id}"), json!({"deleted": id}))
        }
        "git_status" => {
            let p = project_of(server, &a)?;
            let g = ops::git_status(&p)?;
            out(
                format!("{}: {} changed file(s)", g.branch.as_deref().unwrap_or("no branch"), g.dirty),
                json!({
                    "branch": g.branch,
                    "dirty": g.dirty,
                    "ahead_behind": g.ahead_behind.map(|(ahead, behind)| json!([ahead, behind])),
                    "last_commit": g.last_commit,
                    "remote": g.remote,
                    "latest_tag": g.latest_tag,
                    "linked_worktree": g.linked,
                }),
            )
        }
        "git_log" => {
            let p = project_of(server, &a)?;
            let limit = a.opt_int("limit")?.unwrap_or(20).clamp(1, 200) as usize;
            let rows: Vec<Value> =
                ops::git_log(&p, limit)?.iter().map(|c| json!({"hash": c.hash, "date": c.date, "author": c.author, "subject": c.subject})).collect();
            out(format!("{} commits", rows.len()), json!({"commits": rows}))
        }
        "list_branches" => {
            let p = project_of(server, &a)?;
            let (current, branches) = ops::branches(&p)?;
            out(
                format!("{} branches, current {}", branches.len(), current.as_deref().unwrap_or("none")),
                json!({"current": current, "branches": branches}),
            )
        }
        // --- M4: git, GitHub, tasks, dependencies ---
        "git_fetch" => git_out(git::fetch(&ops::primary_root(&project_of(server, &a)?)?)),
        "git_pull" => git_out(git::pull(&ops::primary_root(&project_of(server, &a)?)?)),
        "git_push" => {
            let p = project_of(server, &a)?;
            publishing(store)?;
            git_out(git::push_current(&ops::primary_root(&p)?))
        }
        "switch_branch" => {
            let p = project_of(server, &a)?;
            git_out(git::switch(&ops::primary_root(&p)?, a.req_str("branch")?))
        }
        "create_worktree" => {
            let p = project_of(server, &a)?;
            let root = ops::primary_root(&p)?;
            let branch = a.req_str("branch")?;
            let path = a.opt_str("path")?.map_or_else(|| git::worktree_path(&root, branch), PathBuf::from);
            let o = git::add_worktree(&root, &path, branch, a.opt_bool("new")?.unwrap_or(false));
            if !o.ok {
                return Err(Failure::Tool(format!("{} failed: {}", o.command, o.text)));
            }
            let folder = pa_core::norm(&path);
            store.add_checkout(p.id, &folder)?;
            out(format!("worktree {folder} on {branch}, tracked as a checkout of {}", p.name), json!({"path": folder, "branch": branch, "output": o.text}))
        }
        "github_info" => {
            let p = project_of(server, &a)?;
            let i = github::info(&ops::primary_root(&p)?)?;
            out(
                format!("{}: {}, {} stars, {} open issues, {} open pull requests", i.slug, i.visibility.to_lowercase(), i.stars, i.open_issues, i.open_prs),
                json!({
                    "repository": i.slug,
                    "url": i.url,
                    "visibility": i.visibility,
                    "stars": i.stars,
                    "open_issues": i.open_issues,
                    "open_pull_requests": i.open_prs,
                    "default_branch": i.default_branch,
                    "latest_release": i.latest_release.map(|r| json!({"tag": r.tag, "name": r.name, "url": r.url, "published": r.published})),
                    "ci": i.ci.map(|c| json!({"workflow": c.workflow, "state": c.state(), "url": c.url})),
                }),
            )
        }
        "list_issues" => {
            let p = project_of(server, &a)?;
            let state = a.opt_str("state")?.unwrap_or("open");
            if !matches!(state, "open" | "closed" | "all") {
                return Err(Failure::Params("state must be open, closed or all".into()));
            }
            let issues = github::issues(&ops::primary_root(&p)?, state)?;
            let items = ops::load_todo(&p).map(|f| f.doc.items).unwrap_or_default();
            let rows: Vec<Value> = issues
                .iter()
                .map(|i| {
                    let linked: Vec<&str> = items.iter().filter(|t| github::issue_ref(&t.text) == Some(i.number)).map(|t| t.id.as_str()).collect();
                    json!({"number": i.number, "title": i.title, "state": i.state, "url": i.url, "labels": i.labels, "linked": linked})
                })
                .collect();
            out(format!("{} {state} issues", rows.len()), json!({"issues": rows}))
        }
        "create_issue" => {
            let p = project_of(server, &a)?;
            publishing(store)?;
            let (n, url) = match a.opt_str("item_id")? {
                Some(item) => github::issue_from_item(&p, item)?,
                None => {
                    let labels = a.opt_strings("labels")?.unwrap_or_default();
                    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                    github::create_issue(&ops::primary_root(&p)?, a.req_str("title")?, a.opt_str("body")?.unwrap_or(""), &labels)?
                }
            };
            out(format!("created issue #{n}: {url}"), json!({"number": n, "url": url}))
        }
        "close_issue" => {
            let p = project_of(server, &a)?;
            publishing(store)?;
            let n = u64::try_from(a.req_int("number")?).map_err(|_| Failure::Params("number must be positive".into()))?;
            github::close_issue(&ops::primary_root(&p)?, n)?;
            let mut completed = 0;
            if a.opt_bool("complete_todo")? == Some(true) && ops::todo_path(&p)?.is_some() {
                (completed, _) = ops::edit_todo(&p, |doc| {
                    let mut done = 0;
                    // Ids change when a style moves done items, so find the next linked open item each time.
                    while let Some(id) = doc.items.iter().find(|i| !i.done && github::issue_ref(&i.text) == Some(n)).map(|i| i.id.clone()) {
                        doc.set_done(&id, true)?;
                        done += 1;
                    }
                    Ok(done)
                })?;
            }
            out(format!("closed issue #{n}; {completed} TODO item(s) completed"), json!({"closed": n, "completed_todos": completed}))
        }
        "import_issues" => {
            let p = project_of(server, &a)?;
            let section = a.opt_str("section")?.unwrap_or("Issues");
            let issues = github::issues(&ops::primary_root(&p)?, "open")?;
            let n = github::import_issues(&p, section, &issues)?;
            out(format!("imported {n} of {} open issues into {section:?}", issues.len()), json!({"imported": n, "open_issues": issues.len()}))
        }
        "create_github_repo" => {
            let p = project_of(server, &a)?;
            publishing(store)?;
            let root = ops::primary_root(&p)?;
            let name = a.opt_str("name")?.map_or_else(|| pa_core::folder_name(&root.display().to_string()), str::to_string);
            let owner = a.opt_str("owner")?.map(str::to_string).or_else(github::current_user).unwrap_or_default();
            let (private, push) = (a.opt_bool("private")?.unwrap_or(true), a.opt_bool("push")?.unwrap_or(true));
            let url = github::create_repo(&root, &owner, &name, private, push)?;
            out(format!("created {url}"), json!({"url": url, "private": private, "pushed": push}))
        }
        "list_tasks" => {
            let p = project_of(server, &a)?;
            let rows: Vec<Value> = store.tasks(p.id)?.iter().map(task_json).collect();
            let suggestions: Vec<Value> =
                tasks::suggestions(&ops::primary_root(&p)?).into_iter().map(|(name, command)| json!({"name": name, "command": command})).collect();
            out(format!("{} tasks, {} suggestions", rows.len(), suggestions.len()), json!({"tasks": rows, "suggestions": suggestions}))
        }
        "add_task" => {
            let p = project_of(server, &a)?;
            let (name, command) = (a.req_str("name")?.trim(), a.req_str("command")?.trim());
            if command.is_empty() {
                return Err(Failure::Params("command is empty".into()));
            }
            let id = store.add_task(p.id, name, command, a.opt_str("cwd")?.unwrap_or(""), a.opt_str("env")?.unwrap_or(""))?;
            let t = find_task(store, &p, &id.to_string())?;
            out(format!("added task {name:?} (#{id})"), json!({"task": task_json(&t)}))
        }
        "remove_task" => {
            let p = project_of(server, &a)?;
            let t = find_task(store, &p, &a.req_int("id")?.to_string())?;
            store.delete_task(t.id)?;
            out(format!("removed task {:?}", t.name), json!({"removed": t.id}))
        }
        "run_task" => {
            let p = project_of(server, &a)?;
            let key = match args.get("task") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return Err(Failure::Params("\"task\" must be a task id or name".into())),
            };
            let t = find_task(store, &p, &key)?;
            let timeout = a.opt_int("timeout_secs")?.unwrap_or(120).clamp(1, 600) as u64;
            let run = tasks::Run::start(&t, &ops::primary_root(&p)?, || {})?;
            let ended = run.wait(Duration::from_secs(timeout));
            if !ended {
                run.stop();
                run.wait(Duration::from_secs(5));
            }
            let log = run.log();
            let total = log.lines.len() + log.dropped;
            let tail: Vec<&str> = log.lines.iter().skip(log.lines.len().saturating_sub(300)).map(|(_, l)| l.as_str()).collect();
            let code = log.exit.flatten();
            let what = match (ended, code) {
                (false, _) => format!("stopped after {timeout} s"),
                (true, Some(c)) => format!("exit code {c}"),
                (true, None) => "ended without an exit code".into(),
            };
            out(
                format!("{}: {what}, {total} output lines", t.name),
                json!({"task": t.name, "command": t.command, "exit_code": code, "timed_out": !ended, "lines": total, "output": tail.join("\n")}),
            )
        }
        "list_dependencies" => {
            let p = project_of(server, &a)?;
            let projects = store.projects()?;
            let name = |id: i64| projects.iter().find(|q| q.id == id).map(|q| q.name.clone());
            let row = |d: &Dependency, other: i64| {
                let c = deps::check_projects(&projects, d);
                let v = |r: Option<&Result<String, String>>| match r {
                    Some(Ok(v)) => json!(v),
                    _ => Value::Null,
                };
                let errors: Vec<&String> = c.iter().flat_map(|c| [&c.used, &c.latest]).filter_map(|r| r.as_ref().err()).collect();
                json!({
                    "project": name(other),
                    "used": v(c.as_ref().map(|c| &c.used)),
                    "latest": v(c.as_ref().map(|c| &c.latest)),
                    "standing": c.as_ref().map_or("unknown", |c| standing(c.standing)),
                    "errors": errors,
                    "used_file": d.used_file,
                    "used_pattern": d.used_pattern,
                    "used_fixed": d.used_fixed,
                    "latest_tag": d.latest_tag,
                })
            };
            let uses: Vec<Value> = store.dependencies(p.id)?.iter().map(|d| row(d, d.depends_on)).collect();
            let used_by: Vec<Value> = store.dependents(p.id)?.iter().map(|d| row(d, d.project_id)).collect();
            let behind = uses.iter().filter(|u| u["standing"] == "behind").count();
            out(
                format!("{} uses {} project(s) ({behind} behind) and is used by {}", p.name, uses.len(), used_by.len()),
                json!({"uses": uses, "used_by": used_by}),
            )
        }
        "set_dependency" => {
            let p = project_of(server, &a)?;
            let other = ops::find_project(store, &a.key("depends_on")?)?;
            let s = |k: &str| -> Result<Option<String>, Failure> { Ok(a.opt_str(k)?.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)) };
            let d = Dependency {
                id: 0,
                project_id: p.id,
                depends_on: other.id,
                used_file: s("used_file")?,
                used_pattern: a.opt_str("used_pattern")?.filter(|v| !v.is_empty()).map(str::to_string),
                used_fixed: s("used_fixed")?,
                latest_tag: s("latest_tag")?,
            };
            deps::validate(&d).map_err(Failure::Tool)?;
            store.set_dependency(&d)?;
            let c = deps::check_projects(&store.projects()?, &d);
            let shown = |r: Option<&Result<String, String>>| r.and_then(|r| r.as_ref().ok()).map_or("?".to_string(), String::clone);
            out(
                format!("{} uses {} {} — latest {}", p.name, other.name, shown(c.as_ref().map(|c| &c.used)), shown(c.as_ref().map(|c| &c.latest))),
                json!({"standing": c.as_ref().map_or("unknown", |c| standing(c.standing))}),
            )
        }
        "remove_dependency" => {
            let p = project_of(server, &a)?;
            let other = ops::find_project(store, &a.key("depends_on")?)?;
            let d = store
                .dependencies(p.id)?
                .into_iter()
                .find(|d| d.depends_on == other.id)
                .ok_or_else(|| Failure::Tool(format!("{} does not depend on {}", p.name, other.name)))?;
            store.delete_dependency(d.id)?;
            out(format!("{} no longer depends on {}", p.name, other.name), json!({"removed": d.id}))
        }
        "list_claude_items" => {
            let scope = claude::scope_of(store, &a.key("project")?)?;
            let items: Vec<Value> = claude::inventory(&scope)
                .iter()
                .map(|i| json!({"item": i.rel, "kind": i.kind.label(), "name": i.name, "folder": i.dir, "read_only": i.read_only}))
                .collect();
            out(
                format!("{} Claude config items in {}", items.len(), scope.root.display()),
                json!({"root": scope.root.display().to_string(), "global": scope.global, "items": items}),
            )
        }
        "read_claude_item" => {
            let scope = claude::scope_of(store, &a.key("project")?)?;
            let rel = claude::norm_rel(a.req_str("item")?)?;
            let part = claude_part(&a)?;
            let mut v = json!({"item": rel, "file": scope.path(&rel).display().to_string()});
            let file = if scope.path(&rel).is_dir() {
                v["files"] = json!(claude::files(&scope, &rel));
                format!("{rel}/SKILL.md")
            } else {
                rel.clone()
            };
            let path = scope.path(&file);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) if file != rel => return out(format!("{rel}: a folder without SKILL.md"), v),
                Err(e) => return Err(Failure::Tool(format!("{}: {e}", path.display()))),
            };
            v["text"] = json!(claude::read_part(&text, &part)?);
            if file.ends_with(".md") {
                v["sections"] = json!(claude::md_sections(&text).iter().map(|s| json!({"title": s.title, "level": s.level})).collect::<Vec<_>>());
            } else if file.ends_with(".json") {
                v["keys"] = json!(claude::json_parts(&text).iter().map(|(k, label)| json!({"key": k, "label": label})).collect::<Vec<_>>());
            }
            out(format!("{file} ({} lines)", text.lines().count()), v)
        }
        "copy_claude_item" => {
            let item = a.req_str("item")?;
            let part = claude_part(&a)?;
            let from = claude::scope_of(store, &a.key("from")?)?;
            let to = claude::scope_of(store, &a.key("to")?)?;
            let overwrite = a.opt_bool("overwrite")?.unwrap_or(false);
            let plan = claude::plan_copy(&from, item, &part, &to, a.opt_str("to_item")?, overwrite)?;
            claude_write(server, &plan, a.opt_bool("apply")?.unwrap_or(false), overwrite)
        }
        "edit_claude_item" => {
            let (item, new_text) = (a.req_str("item")?, a.req_str("text")?);
            let scope = claude::scope_of(store, &a.key("project")?)?;
            let plan = claude::plan_edit(&scope, item, None, new_text)?;
            claude_write(server, &plan, a.opt_bool("apply")?.unwrap_or(false), false)
        }
        "list_templates" => {
            let dir = template::ensure(store, &server.data_dir)?;
            let rows: Vec<Value> = template::list(&dir)
                .into_iter()
                .map(|l| match l.template {
                    Ok(t) => template_json(&t),
                    Err(e) => json!({"folder": pa_core::folder_name(&l.dir.to_string_lossy()), "error": e}),
                })
                .collect();
            out(format!("{} templates in {}", rows.len(), dir.display()), json!({"dir": dir.display().to_string(), "templates": rows}))
        }
        "create_project" => {
            let key = a.req_str("template")?;
            let parent = a.req_str("parent_dir")?;
            let prompts: HashMap<String, String> = match args.get("prompts") {
                Some(Value::Object(m)) => m
                    .iter()
                    .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())).ok_or_else(|| Failure::Params(format!("prompt {k:?} must be a string"))))
                    .collect::<Result<_, _>>()?,
                _ => return Err(Failure::Params("\"prompts\" must be an object of strings".into())),
            };
            let t = template::find(&template::ensure(store, &server.data_dir)?, key)?;
            let mut lines = Vec::new();
            let created = template::create(store, &t, &prompts, Path::new(parent), a.opt_str("group")?, &mut |l| lines.push(l));
            let tail: Vec<String> = lines.iter().skip(lines.len().saturating_sub(200)).cloned().collect();
            let (id, dir) = created.map_err(|e| Failure::Tool(format!("{e}\n{}", tail.join("\n"))))?;
            let p = ops::find_project(store, &id.to_string())?;
            out(
                format!("created {} (#{}) from {} in {}", p.name, p.id, t.name, dir.display()),
                json!({"project": summary(&p, &Catalog::load(store)?), "dir": dir.display().to_string(), "output": tail}),
            )
        }
        _ => Err(Failure::Params(format!("unknown tool {name:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_core::Store;
    use std::fs;
    use std::path::PathBuf;

    const CHECKBOX: &str = "# TODO\n\n## Open\n\n- [ ] first\n  - [ ] child\n- [x] shipped\n\n## Later\n\n- [ ] someday\n";
    const ROADMAP: &str = "# Roadmap\n\n## v0.1 — Start\nStatus: in progress\n\n- [ ] set up\n\n## v0.2 — Next\nStatus: planned\n";

    /// A data folder and a project folder "Demo" with a checkbox TODO.md and a ROADMAP.md, tracked.
    fn fixture(name: &str) -> (Server, PathBuf) {
        let base = std::env::temp_dir().join(format!("pa-mcp-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let data_dir = base.join("data");
        let project = base.join("Demo");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("TODO.md"), CHECKBOX).unwrap();
        fs::write(project.join("ROADMAP.md"), ROADMAP).unwrap();
        let server = Server { store: Store::open(&data_dir.join("pa.db")).unwrap(), data_dir };
        ops::import_folder(&server.store, &project, None, None, None).unwrap();
        (server, project)
    }

    fn run(s: &Server, name: &str, args: Value) -> Out {
        match call(s, name, args.as_object().unwrap()) {
            Ok(out) => out,
            Err(e) => panic!("{name} failed: {e:?}"),
        }
    }

    fn fails(s: &Server, name: &str, args: Value) -> String {
        match call(s, name, args.as_object().unwrap()) {
            Err(Failure::Tool(message)) => message,
            other => panic!("{name}: expected a tool failure, got {:?}", other.map(|o| o.summary)),
        }
    }

    fn id_of(data: &Value, text: &str) -> String {
        data["items"].as_array().unwrap().iter().find(|i| i["text"] == text).unwrap()["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn every_listed_tool_is_handled() {
        let (s, _) = fixture("names");
        for t in list() {
            let name = t["name"].as_str().unwrap();
            if let Err(Failure::Params(m)) = call(&s, name, &Map::new()) {
                assert!(!m.starts_with("unknown tool"), "{name} is listed but not handled");
            }
        }
    }

    #[test]
    fn checkbox_todos_add_complete_move_remove() {
        let (s, dir) = fixture("todo");
        let list_data = run(&s, "list_todos", json!({"project": "Demo", "include_done": true})).data;
        assert_eq!(list_data["open"], 3);
        let first = id_of(&list_data, "first");
        let shipped = id_of(&list_data, "shipped");

        let added = run(&s, "add_todo", json!({"project": "Demo", "text": "new one", "section": "Open"})).data;
        assert_eq!(added["item"]["section"], "Open");
        assert_eq!(added["open"], 4);
        let new_id = added["item"]["id"].as_str().unwrap().to_string();

        run(&s, "complete_todo", json!({"project": "Demo", "id": first, "done": true}));
        let moved = run(&s, "move_todo", json!({"project": "Demo", "id": new_id, "section": "Later"})).data;
        assert_eq!(moved["item"]["section"], "Later", "the id changes, the item is found by text");
        run(&s, "remove_todo", json!({"project": "Demo", "id": shipped}));

        let text = fs::read_to_string(dir.join("TODO.md")).unwrap();
        assert!(text.contains("- [x] first\n  - [ ] child\n"), "{text}");
        assert!(!text.contains("shipped"), "{text}");
        assert!(text.contains("## Later\n\n- [ ] someday\n- [ ] new one\n"), "{text}");
    }

    #[test]
    fn bullet_todos_move_done_items_to_their_section() {
        let (s, dir) = fixture("bullets");
        fs::write(dir.join("TODO.md"), "# TODO\n\n## Open\n\n- one\n- two\n\n## Done\n\n").unwrap();
        let data = run(&s, "list_todos", json!({"project": "Demo"})).data;
        assert!(data["style"].as_str().unwrap().starts_with("Bullets"));
        run(&s, "complete_todo", json!({"project": "Demo", "id": id_of(&data, "one"), "done": true}));
        let text = fs::read_to_string(dir.join("TODO.md")).unwrap();
        let (open, done) = text.split_once("## Done").unwrap();
        assert!(!open.contains("- one") && done.contains("- one"), "{text}");
    }

    #[test]
    fn change_style_rewrites_with_backup_and_stores_the_style() {
        let (s, dir) = fixture("style");
        let change = run(&s, "change_todo_style", json!({"project": "Demo", "style": "bullet:delete", "mode": "working_copy"})).data;
        let text = fs::read_to_string(dir.join("TODO.md")).unwrap();
        assert!(text.contains("- first") && !text.contains("[ ]"), "{text}");
        let backup = PathBuf::from(change["backup"].as_str().unwrap());
        assert_eq!(fs::read_to_string(backup).unwrap(), CHECKBOX);
        let project = run(&s, "get_project", json!({"project": "Demo"})).data;
        assert_eq!(project["todo_style"]["source"], "stored");
        assert!(project["todo_style"]["label"].as_str().unwrap().starts_with("Bullets"));
    }

    #[test]
    fn publishing_is_refused_without_the_setting() {
        let (s, dir) = fixture("publish");
        let msg = fails(
            &s,
            "change_todo_style",
            json!({"project": "Demo", "style": "bullet:delete", "mode": "branch", "branch": "todo-style", "push_and_pr": true}),
        );
        assert!(msg.contains("mcp.allow_publish"), "{msg}");
        assert_eq!(fs::read_to_string(dir.join("TODO.md")).unwrap(), CHECKBOX);
    }

    #[test]
    fn a_file_changed_between_read_and_save_is_refused() {
        let (s, dir) = fixture("conflict");
        let project = ops::find_project(&s.store, "Demo").unwrap();
        let result = ops::edit_todo(&project, |doc| {
            fs::write(dir.join("TODO.md"), "# TODO\n\n## Open\n\n- [ ] changed elsewhere\n")?;
            doc.add("Open", "mine")?;
            Ok(())
        });
        let err = result.expect_err("refused").to_string();
        assert!(err.contains("changed outside the app"), "{err}");
        assert!(fs::read_to_string(dir.join("TODO.md")).unwrap().contains("changed elsewhere"));
    }

    #[test]
    fn roadmap_milestones_and_items() {
        let (s, dir) = fixture("roadmap");
        let road = run(&s, "get_roadmap", json!({"project": "Demo"})).data;
        assert_eq!(road["editable"], true);
        assert_eq!(road["milestones"][0]["status"], "in progress");
        assert_eq!(road["milestones"][0]["progress"]["total"], 1);
        let set_up = road["milestones"][0]["items"][0]["id"].as_str().unwrap().to_string();

        run(&s, "add_milestone", json!({"project": "Demo", "title": "v0.3 — Later"}));
        run(&s, "set_milestone_status", json!({"project": "Demo", "title": "v0.1 — Start", "status": "done"}));
        let item = run(&s, "add_milestone_item", json!({"project": "Demo", "milestone": "v0.2 — Next", "text": "plan it"})).data;
        assert!(item["item"]["id"].is_string());
        run(&s, "complete_milestone_item", json!({"project": "Demo", "id": set_up, "done": true}));
        let msg = fails(&s, "add_milestone_item", json!({"project": "Demo", "milestone": "nope", "text": "x"}));
        assert!(msg.contains("no milestone"), "{msg}");

        let text = fs::read_to_string(dir.join("ROADMAP.md")).unwrap();
        assert!(text.contains("## v0.1 — Start\nStatus: released"), "a version's done is written as released: {text}");
        assert!(text.contains("## v0.3 — Later\nStatus: planned"), "{text}");
        assert!(text.contains("## v0.2 — Next\nStatus: planned\n- [ ] plan it\n"), "{text}");
        assert!(text.contains("- [x] set up"), "{text}");
    }

    #[test]
    fn notes_crud_global_and_per_project() {
        let (s, _) = fixture("notes");
        let id = run(&s, "write_note", json!({"title": "Ideas", "body": "one"})).data["id"].as_i64().unwrap();
        assert_eq!(run(&s, "read_note", json!({"id": id})).data["body"], "one");
        run(&s, "write_note", json!({"id": id, "title": "Ideas 2", "body": "two"}));
        let read = run(&s, "read_note", json!({"id": id})).data;
        assert_eq!((read["title"].as_str(), read["body"].as_str()), (Some("Ideas 2"), Some("two")));
        assert_eq!(run(&s, "list_notes", json!({})).data["notes"].as_array().unwrap().len(), 1);
        assert_eq!(run(&s, "list_notes", json!({"project": "Demo"})).data["notes"].as_array().unwrap().len(), 0);
        run(&s, "delete_note", json!({"id": id}));
        assert_eq!(run(&s, "list_notes", json!({})).data["notes"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn capture_files_into_the_inbox_or_the_notes() {
        let (s, dir) = fixture("capture");
        run(&s, "capture", json!({"project": "Demo", "kind": "idea", "title": "Dark mode", "details": "for the board"}));
        run(&s, "capture", json!({"project": "Demo", "kind": "TODO", "title": "Write docs"}));
        let text = fs::read_to_string(dir.join("TODO.md")).unwrap();
        assert!(text.starts_with("# TODO\n\n## Inbox\n\n- [ ] Idea: Dark mode\n      for the board\n- [ ] Write docs\n\n## Open\n"), "{text}");
        let note = run(&s, "capture", json!({"project": "Demo", "kind": "Note", "title": "Thought", "details": "body"})).data;
        assert_eq!(run(&s, "read_note", json!({"id": note["id"]})).data["body"], "body");
        assert!(matches!(call(&s, "capture", json!({"project": "Demo", "kind": "Wish", "title": "x"}).as_object().unwrap()), Err(Failure::Params(_))));

        fs::remove_file(dir.join("TODO.md")).unwrap();
        fs::write(dir.join("TODO.txt"), "A one\r\nB two\r\n").unwrap();
        run(&s, "capture", json!({"project": "Demo", "kind": "Bug", "title": "Crash", "details": "on\nstart"}));
        assert_eq!(fs::read_to_string(dir.join("TODO.txt")).unwrap(), "A one\r\nB two\r\nBug: Crash — on start\r\n");

        fs::remove_file(dir.join("TODO.txt")).unwrap();
        run(&s, "capture", json!({"project": "Demo", "kind": "Bug", "title": "No file"}));
        assert!(fs::read_to_string(dir.join("TODO.md")).unwrap().contains("## Inbox\n\n- [ ] Bug: No file\n"));
    }

    #[test]
    fn current_project_takes_the_longest_checkout() {
        let (s, dir) = fixture("nested");
        let inner = dir.join("tools").join("inner");
        fs::create_dir_all(inner.join("src")).unwrap();
        ops::import_folder(&s.store, &inner, None, None, None).unwrap();
        let deep = run(&s, "current_project", json!({"path": inner.join("src").display().to_string()})).data;
        assert_eq!(deep["project"]["name"], "inner");
        let outer = run(&s, "current_project", json!({"path": dir.join("src").display().to_string()})).data;
        assert_eq!(outer["project"]["name"], "Demo");
        let msg = fails(&s, "current_project", json!({"path": s.data_dir.display().to_string()}));
        assert!(msg.contains("no tracked project"), "{msg}");
    }

    #[test]
    fn list_projects_filters_by_text() {
        let (s, _) = fixture("filter");
        assert_eq!(run(&s, "list_projects", json!({"query": "demo"})).data["projects"].as_array().unwrap().len(), 1);
        assert_eq!(run(&s, "list_projects", json!({"query": "zzz"})).data["projects"].as_array().unwrap().len(), 0);
    }

    /// The tracked project "Demo" with `text` as its ROADMAP.md.
    fn with_roadmap(name: &str, text: &str) -> (Server, PathBuf) {
        let (s, dir) = fixture(name);
        fs::write(dir.join("ROADMAP.md"), text).unwrap();
        (s, dir)
    }

    /// Makes `dir` a git repository whose only commit carries the tag `name`.
    fn tag(dir: &Path, name: &str) {
        for args in
            [vec!["init", "-q"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"], vec!["tag", name]]
        {
            assert!(pa_core::cmd("git").args(&args).current_dir(dir).status().unwrap().success(), "git {args:?}");
        }
    }

    #[test]
    fn get_roadmap_lists_versions_with_board_warnings_and_suggestions() {
        let text = "# Roadmap\n\n## Milestone: UI\nVersions: v0.5.0, v0.6.0\n\n## v0.4.0\nStatus: released\n\n## v0.5.0 — Foundation\nMilestone: UI\n\n- [x] palette\n- [ ] docking\n\n## v0.6.0\nStatus: planned\nMilestone: UI\n\n## Milestone: Docs\nStatus: done\nVersions: v0.7.0\n\n## v0.7.0\nStatus: planned\n";
        let (s, dir) = with_roadmap("get-roadmap-v2", text);
        tag(&dir, "v0.5.0");
        let road = run(&s, "get_roadmap", json!({"project": "Demo"})).data;
        let entries = road["milestones"].as_array().unwrap();
        assert_eq!(entries.len(), 6, "versions and milestones, no sections");
        assert_eq!(
            (entries[0]["kind"].as_str(), entries[0]["progress"]["done"].as_u64(), entries[0]["progress"]["total"].as_u64()),
            (Some("milestone"), Some(1), Some(2))
        );
        assert_eq!(
            (entries[2]["kind"].as_str(), entries[2]["version"].as_str(), entries[2]["title"].as_str()),
            (Some("version"), Some("v0.5.0"), Some("Foundation"))
        );
        assert_eq!(entries[1]["status"], "released");
        assert_eq!(entries[0]["effective"], "planned", "derived from its versions");
        assert_eq!(entries[4]["warning"], "Status says done but v0.7.0 is planned");
        assert_eq!(entries[4]["explicit_status"], true);
        assert_eq!(road["board"]["now"], json!([]));
        assert_eq!(road["board"]["next"], json!(["UI", "v0.5.0"]));
        assert_eq!(road["board"]["later"], json!(["v0.6.0", "v0.7.0"]));
        assert_eq!(road["board"]["done"], json!(["v0.4.0", "Docs"]));
        assert_eq!(road["release_suggestions"], json!(["v0.5.0"]), "tagged and not released");
    }

    #[test]
    fn roadmap_edits_add_link_move_and_set_versions() {
        let (s, dir) = with_roadmap("roadmap-edits", "# Roadmap\n\n## v0.1 — Start\nStatus: planned\n\n## v0.3 — Late\nStatus: planned\n");
        let added = run(&s, "add_version", json!({"project": "Demo", "version": "v0.2.0", "title": "Middle", "milestone": "Docs"})).data;
        assert_eq!((added["entry"]["version"].as_str(), added["entry"]["links"].clone()), (Some("v0.2.0"), json!(["Docs"])));
        let msg = fails(&s, "add_version", json!({"project": "Demo", "version": "v0.2.0"}));
        assert!(msg.contains("already exists"), "{msg}");
        run(&s, "link_version", json!({"project": "Demo", "version": "v0.3", "milestone": "Docs"}));
        let order = run(&s, "move_roadmap_entry", json!({"project": "Demo", "entry": "v0.3", "before": "v0.1"})).data;
        assert_eq!(order["order"], json!(["v0.3", "v0.1", "v0.2.0"]));
        run(&s, "set_milestone_status", json!({"project": "Demo", "title": "v0.1 — Start", "status": "released"}));

        let text = fs::read_to_string(dir.join("ROADMAP.md")).unwrap();
        assert!(text.starts_with("# Roadmap\n\n## v0.3 — Late\nStatus: planned\nMilestone: Docs\n"), "{text}");
        assert!(text.contains("## v0.2.0 — Middle\nStatus: planned\nMilestone: Docs\n"), "{text}");
        assert!(text.contains("## v0.1 — Start\nStatus: released"), "{text}");
        let end = run(&s, "move_roadmap_entry", json!({"project": "Demo", "entry": "v0.3"})).data;
        assert_eq!(end["order"][2], "v0.3", "no before: to the end");
        let bad = fails(&s, "add_version", json!({"project": "Demo", "version": "later"}));
        assert!(bad.contains("not a version"), "{bad}");
    }

    #[test]
    fn tag_prefix_drives_suggestions_and_shows_in_the_project() {
        let (s, dir) = with_roadmap("tag-prefix", "# Roadmap\n\n## v0.5.0\nStatus: planned\n");
        tag(&dir, "spec-v0.5.0");
        assert_eq!(run(&s, "get_roadmap", json!({"project": "Demo"})).data["release_suggestions"], json!([]), "default prefix v");
        run(&s, "update_project", json!({"project": "Demo", "tag_prefix": "spec-v"}));
        assert_eq!(run(&s, "get_roadmap", json!({"project": "Demo"})).data["release_suggestions"], json!(["v0.5.0"]));
        assert_eq!(run(&s, "get_project", json!({"project": "Demo"})).data["tag_prefix"], "spec-v");
        run(&s, "update_project", json!({"project": "Demo", "tag_prefix": null}));
        assert_eq!(run(&s, "get_project", json!({"project": "Demo"})).data["tag_prefix"], "v");
    }

    #[test]
    fn list_todos_all_adds_roadmap_items_with_their_source() {
        let (s, _) = with_roadmap("all-sources", "# Roadmap\n\n## v0.1 — Start\nStatus: planned\n\n- [ ] set up roadmap\n");
        let all = run(&s, "list_todos_all", json!({})).data["items"].clone();
        assert_eq!(all.as_array().unwrap().len(), 4, "three open TODO items and one roadmap item");
        let roadmap = run(&s, "list_todos_all", json!({"source": "roadmap"})).data["items"].clone();
        assert_eq!(
            (roadmap[0]["source"].as_str(), roadmap[0]["entry"].as_str(), roadmap[0]["text"].as_str()),
            (Some("roadmap"), Some("v0.1"), Some("set up roadmap"))
        );
        assert_eq!(roadmap.as_array().unwrap().len(), 1);
        let todos = run(&s, "list_todos_all", json!({"source": "todo"})).data["items"].clone();
        assert!(todos.as_array().unwrap().iter().all(|i| i["source"] == "todo"), "{todos}");
        assert!(matches!(call(&s, "list_todos_all", &json!({"source": "x"}).as_object().unwrap().clone()), Err(Failure::Params(_))));
        let q = |query: &str| run(&s, "list_todos_all", json!({"query": query})).data["items"].as_array().unwrap().len();
        assert_eq!((q("source:roadmap"), q("in:Later"), q("-in:Later project:demo")), (1, 1, 3));
        assert_eq!(q("is:done"), 1, "is: in the query makes the status default to all");
    }

    #[test]
    fn tasks_add_list_run_and_remove() {
        let (s, dir) = fixture("tasks");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let listed = run(&s, "list_tasks", json!({"project": "Demo"})).data;
        assert_eq!(listed["tasks"], json!([]));
        assert_eq!(listed["suggestions"][1]["command"], "cargo test");
        let added = run(&s, "add_task", json!({"project": "Demo", "name": "Greet", "command": "echo hello-from-task", "env": "X=1"})).data;
        let id = added["task"]["id"].as_i64().unwrap();
        let ran = run(&s, "run_task", json!({"project": "Demo", "task": "greet", "timeout_secs": 30})).data;
        assert_eq!((ran["exit_code"].as_i64(), ran["timed_out"].as_bool()), (Some(0), Some(false)));
        assert!(ran["output"].as_str().unwrap().contains("hello-from-task"), "{ran}");
        assert!(fails(&s, "run_task", json!({"project": "Demo", "task": "nope"})).contains("no task"));
        run(&s, "remove_task", json!({"project": "Demo", "id": id}));
        assert_eq!(run(&s, "list_tasks", json!({"project": "Demo"})).data["tasks"], json!([]));
    }

    #[test]
    fn dependencies_set_list_and_remove() {
        let (s, dir) = fixture("deps");
        let spec = dir.parent().unwrap().join("Spec");
        fs::create_dir_all(&spec).unwrap();
        fs::write(spec.join("package.json"), r#"{"version": "11.0"}"#).unwrap();
        ops::import_folder(&s.store, &spec, None, None, None).unwrap();
        fs::write(dir.join("SPEC_VERSION"), "spec-v10.1\n").unwrap();
        let msg = fails(&s, "set_dependency", json!({"project": "Demo", "depends_on": "Spec"}));
        assert!(msg.contains("used version"), "{msg}");
        let set = run(&s, "set_dependency", json!({"project": "Demo", "depends_on": "Spec", "used_file": "SPEC_VERSION", "used_pattern": "spec-v([0-9.]+)"}));
        assert_eq!(set.summary, "Demo uses Spec 10.1 — latest 11.0");
        assert_eq!(set.data["standing"], "behind");
        let listed = run(&s, "list_dependencies", json!({"project": "Demo"})).data;
        assert_eq!((listed["uses"][0]["project"].as_str(), listed["uses"][0]["used"].as_str()), (Some("Spec"), Some("10.1")));
        let other = run(&s, "list_dependencies", json!({"project": "Spec"})).data;
        assert_eq!(other["used_by"][0]["project"], "Demo");
        run(&s, "remove_dependency", json!({"project": "Demo", "depends_on": "Spec"}));
        assert!(fails(&s, "remove_dependency", json!({"project": "Demo", "depends_on": "Spec"})).contains("does not depend"));
    }

    #[test]
    fn publishing_tools_are_refused_without_the_setting() {
        let (s, _) = fixture("publish-m4");
        for (name, args) in [
            ("git_push", json!({"project": "Demo"})),
            ("create_issue", json!({"project": "Demo", "title": "x"})),
            ("close_issue", json!({"project": "Demo", "number": 1})),
            ("create_github_repo", json!({"project": "Demo"})),
        ] {
            let msg = fails(&s, name, args);
            assert!(msg.contains("publishing is off"), "{name}: {msg}");
        }
    }

    #[test]
    fn git_tools_fetch_switch_and_worktree() {
        let (s, dir) = fixture("git-m4");
        tag(&dir, "v0.1.0");
        assert!(run(&s, "git_fetch", json!({"project": "Demo"})).summary.starts_with("git fetch"));
        let branch = ops::branches(&ops::find_project(&s.store, "Demo").unwrap()).unwrap().0.unwrap();
        let wt = dir.parent().unwrap().join("Demo-feature");
        let made = run(&s, "create_worktree", json!({"project": "Demo", "branch": "feature", "new": true})).data;
        assert_eq!(made["path"].as_str().map(PathBuf::from), Some(PathBuf::from(pa_core::norm(&wt))));
        assert_eq!(ops::find_project(&s.store, "Demo").unwrap().checkouts.len(), 2, "the worktree is a checkout");
        assert!(fails(&s, "switch_branch", json!({"project": "Demo", "branch": "feature"})).contains("failed"), "checked out in the worktree");
        pa_core::cmd("git").args(["branch", "topic"]).current_dir(&dir).status().unwrap();
        run(&s, "switch_branch", json!({"project": "Demo", "branch": "topic"}));
        run(&s, "switch_branch", json!({"project": "Demo", "branch": branch}));
    }

    #[test]
    fn capture_without_a_github_remote_stays_in_the_inbox() {
        let (s, dir) = fixture("capture-issue");
        let id = ops::find_project(&s.store, "Demo").unwrap().id;
        s.store.set_setting(&github::capture_key(id), "true").unwrap();
        let data = run(&s, "capture", json!({"project": "Demo", "kind": "Bug", "title": "Offline"})).data;
        assert_eq!((data["issue"].is_null(), data["issue_error"].is_null()), (true, true));
        assert!(fs::read_to_string(dir.join("TODO.md")).unwrap().contains("- [ ] Bug: Offline\n"));
    }

    #[test]
    fn claude_items_preview_by_default_and_write_only_when_allowed() {
        let (s, dir) = fixture("claude");
        let home = s.data_dir.parent().unwrap().join("home");
        s.store.set_setting(claude::HOME, &home.display().to_string()).unwrap();
        fs::create_dir_all(dir.join(".claude/agents")).unwrap();
        fs::write(dir.join(".claude/agents/reviewer.md"), "Review.\n").unwrap();
        fs::write(dir.join(".claude/settings.json"), r#"{"permissions":{"allow":["Bash(cargo test:*)"]}}"#).unwrap();
        let items = run(&s, "list_claude_items", json!({"project": "Demo"})).data["items"].clone();
        assert_eq!(items.as_array().unwrap().len(), 2, "{items}");
        let read = run(&s, "read_claude_item", json!({"project": "Demo", "item": ".claude/settings.json"})).data;
        assert_eq!(read["keys"][2]["key"], "/permissions/allow/0");

        let preview = run(&s, "copy_claude_item", json!({"from": "Demo", "to": "global", "item": ".claude/agents/reviewer.md"})).data;
        assert_eq!((preview["applied"].as_bool(), preview["changes"][0]["new_file"].as_bool()), (Some(false), Some(true)));
        assert!(!home.join(".claude/agents/reviewer.md").exists(), "a preview writes nothing");
        let refused = fails(&s, "copy_claude_item", json!({"from": "Demo", "to": "global", "item": ".claude/agents/reviewer.md", "apply": true}));
        assert!(refused.contains(claude::ALLOW_WRITE), "{refused}");

        s.store.set_setting(claude::ALLOW_WRITE, "true").unwrap();
        run(&s, "copy_claude_item", json!({"from": "Demo", "to": "global", "item": ".claude/agents/reviewer.md", "apply": true}));
        assert_eq!(fs::read_to_string(home.join(".claude/agents/reviewer.md")).unwrap(), "Review.\n");
        run(&s, "copy_claude_item", json!({"from": "Demo", "to": "global", "item": ".claude/settings.json", "key": "/permissions/allow/0", "apply": true}));
        let global = run(&s, "read_claude_item", json!({"project": "global", "item": ".claude/settings.json", "key": "/permissions/allow"})).data;
        assert_eq!(global["text"], "[\n  \"Bash(cargo test:*)\"\n]");

        let bad = fails(&s, "edit_claude_item", json!({"project": "Demo", "item": ".claude/settings.json", "text": "{", "apply": true}));
        assert!(bad.contains("not valid JSON"), "{bad}");
        let edited = run(&s, "edit_claude_item", json!({"project": "Demo", "item": "CLAUDE.md", "text": "# Demo\n", "apply": true})).data;
        assert_eq!(edited["applied"], true);
        assert_eq!(fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), "# Demo\n");
        assert!(fails(&s, "edit_claude_item", json!({"project": "Demo", "item": "TODO.md", "text": "x", "apply": true})).contains("not a Claude config"));
    }

    #[test]
    fn templates_are_listed_and_create_tracked_projects() {
        let (s, dir) = fixture("templates");
        let base = dir.parent().unwrap().to_path_buf();
        s.store.set_setting(template::DIR, &base.join("templates").display().to_string()).unwrap();
        let mine = base.join("templates/mine");
        fs::create_dir_all(mine.join("files")).unwrap();
        fs::write(mine.join("files/NOTES.md"), "{{name}}: {{description}}\n").unwrap();
        let toml = "name = \"Mine\"\ntags = [\"t\"]\n[[prompt]]\nkey = \"description\"\n[[step]]\noverlay = \"files\"\n[[step]]\nrun = \"echo ok\"\n";
        fs::write(mine.join(template::FILE), toml).unwrap();
        let listed = run(&s, "list_templates", json!({})).data;
        let names: Vec<&str> = listed["templates"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"Mine") && names.contains(&"Rust egui app"), "{names:?}");

        let parent = base.join("code");
        fs::create_dir_all(&parent).unwrap();
        let made = run(&s, "create_project", json!({"template": "mine", "prompts": {"name": "fresh", "description": "new"}, "parent_dir": parent.display().to_string()})).data;
        assert_eq!(made["project"]["name"], "fresh");
        assert_eq!(made["project"]["tags"], json!(["t"]));
        assert!(made["output"].as_array().unwrap().iter().any(|l| l == "ok"), "{made}");
        assert_eq!(fs::read_to_string(parent.join("fresh/NOTES.md")).unwrap(), "fresh: new\n");
        let again = fails(&s, "create_project", json!({"template": "mine", "prompts": {"name": "fresh"}, "parent_dir": parent.display().to_string()}));
        assert!(again.contains("exists already"), "{again}");
    }
}
