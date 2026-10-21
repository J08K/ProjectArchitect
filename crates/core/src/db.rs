//! The SQLite store (`pa.db`): projects, checkouts, groups, purposes, tags, scan roots, ignored paths, settings.
//! WAL mode so the GUI and the MCP server can share the file. Repo content (TODO.md, ...) never lives here (D-1).

use crate::info::Info;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::time::Duration;

pub type Result<T> = rusqlite::Result<T>;

/// Schema steps; `PRAGMA user_version` is the number applied. Never edit a step, append a new one.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE groups (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT
);
CREATE TABLE purposes (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    sort INTEGER NOT NULL DEFAULT 0,
    track_todos INTEGER NOT NULL DEFAULT 1,
    watch_upstream INTEGER NOT NULL DEFAULT 0,
    show_in_overview INTEGER NOT NULL DEFAULT 1,
    scan_code_todos INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL,
    purpose_id INTEGER REFERENCES purposes(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'Active' CHECK (status IN ('Active', 'Paused', 'Done', 'Abandoned')),
    added_at INTEGER NOT NULL
);
CREATE TABLE checkouts (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    is_primary INTEGER NOT NULL DEFAULT 0,
    info TEXT,
    info_at INTEGER
);
CREATE TABLE tags (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (project_id, tag)
);
CREATE TABLE scan_roots (path TEXT PRIMARY KEY, depth INTEGER NOT NULL DEFAULT 3);
CREATE TABLE ignored_paths (path TEXT PRIMARY KEY);
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

INSERT INTO purposes (name, sort, track_todos, watch_upstream, show_in_overview, scan_code_todos) VALUES
    ('Developing', 0, 1, 0, 1, 1),
    ('Reference', 1, 0, 1, 1, 0),
    ('Installed tool', 2, 0, 1, 1, 0),
    ('Contributor', 3, 1, 1, 1, 1),
    ('Archive', 4, 0, 0, 0, 0);
"#,
    r#"
ALTER TABLE projects ADD COLUMN todo_path TEXT;
ALTER TABLE projects ADD COLUMN roadmap_path TEXT;
CREATE TABLE notes (
    id INTEGER PRIMARY KEY,
    project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);
"#,
    r#"
ALTER TABLE projects ADD COLUMN todo_style TEXT;
"#,
    r#"
ALTER TABLE projects ADD COLUMN tag_prefix TEXT;
"#,
    r#"
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    cwd TEXT NOT NULL DEFAULT '',
    env TEXT NOT NULL DEFAULT ''
);
CREATE TABLE dependencies (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    depends_on INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    used_file TEXT,
    used_pattern TEXT,
    used_fixed TEXT,
    latest_tag TEXT,
    UNIQUE (project_id, depends_on)
);
"#,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Status {
    #[default]
    Active,
    Paused,
    Done,
    Abandoned,
}

impl Status {
    pub const ALL: [Status; 4] = [Status::Active, Status::Paused, Status::Done, Status::Abandoned];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "Active",
            Status::Paused => "Paused",
            Status::Done => "Done",
            Status::Abandoned => "Abandoned",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|v| v.as_str().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    /// The folder that holds the group's projects (e.g. `Code\Hobby`), when it has one.
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Purpose {
    pub id: i64,
    pub name: String,
    pub sort: i64,
    pub track_todos: bool,
    pub watch_upstream: bool,
    pub show_in_overview: bool,
    pub scan_code_todos: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Checkout {
    pub id: i64,
    pub project_id: i64,
    pub path: String,
    pub primary: bool,
    /// Last collected info (cached so the overview shows at once on start).
    pub info: Option<Info>,
    pub info_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub purpose_id: Option<i64>,
    pub status: Status,
    /// TODO file, relative to the checkout; `None` = auto-detect (`todo::find_file`).
    pub todo_path: Option<String>,
    /// Roadmap file, relative to the checkout; `None` = auto-detect (`roadmap::find_roadmap`).
    pub roadmap_path: Option<String>,
    /// The TODO style as [`crate::todo::Style::encode`] wrote it; `None` = detect (`Style::detect`).
    pub todo_style: Option<String>,
    /// Prefix of the release tags (`v0.5.0` → `v`); `None` = `v` (`ops::tag_prefix`), `Some("")` = bare `0.5.0`.
    pub tag_prefix: Option<String>,
    pub tags: Vec<String>,
    /// Primary first, then by path.
    pub checkouts: Vec<Checkout>,
}

/// A markdown note: per project, or global (`project_id` None).
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: i64,
    pub project_id: Option<i64>,
    pub title: String,
    pub body: String,
    pub updated_at: i64,
}

impl Project {
    pub fn primary(&self) -> Option<&Checkout> {
        self.checkouts.first()
    }

    pub fn info(&self) -> Option<&Info> {
        self.primary().and_then(|c| c.info.as_ref())
    }
}

pub struct Store {
    /// Crate-wide so feature modules (`tasks`, `deps`) keep their own queries.
    pub(crate) c: Connection,
}

impl Store {
    /// Open (or create) the database file, parent folder included.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let c = Connection::open(path)?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        Store::init(c)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(c: Connection) -> Result<Store> {
        c.busy_timeout(Duration::from_secs(5))?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        let done: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, step) in MIGRATIONS.iter().enumerate().skip(done as usize) {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(step)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Ok(Store { c })
    }

    /// Changes when another connection (the MCP server) wrote to the file; poll it to know when to reload.
    pub fn data_version(&self) -> Result<i64> {
        self.c.pragma_query_value(None, "data_version", |r| r.get(0))
    }

    // --- projects -------------------------------------------------------------------------------------------

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut projects: Vec<Project> = self
            .c
            .prepare(
                "SELECT id, name, group_id, purpose_id, status, todo_path, roadmap_path, todo_style, tag_prefix FROM projects ORDER BY name COLLATE NOCASE",
            )?
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    group_id: r.get(2)?,
                    purpose_id: r.get(3)?,
                    status: Status::parse(&r.get::<_, String>(4)?).unwrap_or_default(),
                    todo_path: r.get(5)?,
                    roadmap_path: r.get(6)?,
                    todo_style: r.get(7)?,
                    tag_prefix: r.get(8)?,
                    tags: Vec::new(),
                    checkouts: Vec::new(),
                })
            })?
            .collect::<Result<_>>()?;
        let index: std::collections::HashMap<i64, usize> = projects.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        let mut st = self.c.prepare("SELECT project_id, tag FROM tags ORDER BY tag COLLATE NOCASE")?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (pid, tag) = row?;
            if let Some(&i) = index.get(&pid) {
                projects[i].tags.push(tag);
            }
        }
        let mut st = self.c.prepare(
            "SELECT id, project_id, path, is_primary, info, info_at FROM checkouts ORDER BY is_primary DESC, path COLLATE NOCASE",
        )?;
        for c in st.query_map([], checkout_row)? {
            let c = c?;
            if let Some(&i) = index.get(&c.project_id) {
                projects[i].checkouts.push(c);
            }
        }
        Ok(projects)
    }

    /// New project with its checkouts (the first path becomes the primary one). Returns its id.
    pub fn add_project(&self, name: &str, group_id: Option<i64>, purpose_id: Option<i64>, paths: &[String]) -> Result<i64> {
        let tx = self.c.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO projects (name, group_id, purpose_id, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, group_id, purpose_id, crate::now_secs()],
        )?;
        let id = tx.last_insert_rowid();
        for (i, p) in paths.iter().enumerate() {
            tx.execute("INSERT INTO checkouts (project_id, path, is_primary) VALUES (?1, ?2, ?3)", params![id, p, i == 0])?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Save name, group, purpose, status, file paths, TODO style, tag prefix and tags (checkouts have their own calls).
    pub fn update_project(&self, p: &Project) -> Result<()> {
        let tx = self.c.unchecked_transaction()?;
        tx.execute(
            "UPDATE projects SET name = ?2, group_id = ?3, purpose_id = ?4, status = ?5, todo_path = ?6, roadmap_path = ?7, todo_style = ?8, tag_prefix = ?9
             WHERE id = ?1",
            params![p.id, p.name, p.group_id, p.purpose_id, p.status.as_str(), p.todo_path, p.roadmap_path, p.todo_style, p.tag_prefix],
        )?;
        tx.execute("DELETE FROM tags WHERE project_id = ?1", [p.id])?;
        for t in &p.tags {
            tx.execute("INSERT OR IGNORE INTO tags (project_id, tag) VALUES (?1, ?2)", params![p.id, t])?;
        }
        tx.commit()
    }

    /// Forget a project (its folders are not touched).
    pub fn remove_project(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM projects WHERE id = ?1", [id]).map(|_| ())
    }

    // --- checkouts ------------------------------------------------------------------------------------------

    pub fn add_checkout(&self, project_id: i64, path: &str) -> Result<i64> {
        self.c.execute("INSERT INTO checkouts (project_id, path) VALUES (?1, ?2)", params![project_id, path])?;
        Ok(self.c.last_insert_rowid())
    }

    pub fn checkout_by_path(&self, path: &str) -> Result<Option<Checkout>> {
        self.c
            .query_row("SELECT id, project_id, path, is_primary, info, info_at FROM checkouts WHERE path = ?1", [path], checkout_row)
            .optional()
    }

    pub fn remove_checkout(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM checkouts WHERE id = ?1", [id]).map(|_| ())
    }

    pub fn set_primary(&self, checkout_id: i64) -> Result<()> {
        self.c.execute(
            "UPDATE checkouts SET is_primary = (id = ?1) WHERE project_id = (SELECT project_id FROM checkouts WHERE id = ?1)",
            [checkout_id],
        )?;
        Ok(())
    }

    /// The folder moved: point the checkout at its new place (cached info dropped).
    pub fn relocate_checkout(&self, id: i64, path: &str) -> Result<()> {
        self.c.execute("UPDATE checkouts SET path = ?2, info = NULL, info_at = NULL WHERE id = ?1", params![id, path]).map(|_| ())
    }

    pub fn set_info(&self, checkout_id: i64, info: &Info) -> Result<()> {
        let json = serde_json::to_string(info).unwrap_or_default();
        self.c
            .execute("UPDATE checkouts SET info = ?2, info_at = ?3 WHERE id = ?1", params![checkout_id, json, crate::now_secs()])
            .map(|_| ())
    }

    // --- notes ----------------------------------------------------------------------------------------------

    /// Notes of a project (`None` = the global ones), newest first.
    pub fn notes(&self, project_id: Option<i64>) -> Result<Vec<Note>> {
        self.c
            .prepare("SELECT id, project_id, title, body, updated_at FROM notes WHERE project_id IS ?1 ORDER BY updated_at DESC, id DESC")?
            .query_map([project_id], |r| Ok(Note { id: r.get(0)?, project_id: r.get(1)?, title: r.get(2)?, body: r.get(3)?, updated_at: r.get(4)? }))?
            .collect()
    }

    pub fn add_note(&self, project_id: Option<i64>, title: &str) -> Result<i64> {
        self.c.execute("INSERT INTO notes (project_id, title, updated_at) VALUES (?1, ?2, ?3)", params![project_id, title, crate::now_secs()])?;
        Ok(self.c.last_insert_rowid())
    }

    /// Save title and body; `updated_at` becomes now.
    pub fn update_note(&self, n: &Note) -> Result<()> {
        self.c
            .execute("UPDATE notes SET title = ?2, body = ?3, updated_at = ?4 WHERE id = ?1", params![n.id, n.title, n.body, crate::now_secs()])
            .map(|_| ())
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM notes WHERE id = ?1", [id]).map(|_| ())
    }

    // --- groups and purposes --------------------------------------------------------------------------------

    pub fn groups(&self) -> Result<Vec<Group>> {
        self.c
            .prepare("SELECT id, name, path FROM groups ORDER BY name COLLATE NOCASE")?
            .query_map([], |r| Ok(Group { id: r.get(0)?, name: r.get(1)?, path: r.get(2)? }))?
            .collect()
    }

    /// New group; a taken name gets " (2)", " (3)", ... Returns its id.
    pub fn add_group(&self, name: &str, path: Option<&str>) -> Result<i64> {
        let name = self.free_name("groups", name)?;
        self.c.execute("INSERT INTO groups (name, path) VALUES (?1, ?2)", params![name, path])?;
        Ok(self.c.last_insert_rowid())
    }

    pub fn update_group(&self, g: &Group) -> Result<()> {
        self.c.execute("UPDATE groups SET name = ?2, path = ?3 WHERE id = ?1", params![g.id, g.name, g.path]).map(|_| ())
    }

    /// Delete a group; its projects stay, without a group.
    pub fn delete_group(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM groups WHERE id = ?1", [id]).map(|_| ())
    }

    pub fn purposes(&self) -> Result<Vec<Purpose>> {
        self.c
            .prepare(
                "SELECT id, name, sort, track_todos, watch_upstream, show_in_overview, scan_code_todos FROM purposes ORDER BY sort, name",
            )?
            .query_map([], |r| {
                Ok(Purpose {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    sort: r.get(2)?,
                    track_todos: r.get(3)?,
                    watch_upstream: r.get(4)?,
                    show_in_overview: r.get(5)?,
                    scan_code_todos: r.get(6)?,
                })
            })?
            .collect()
    }

    pub fn add_purpose(&self, name: &str) -> Result<i64> {
        let name = self.free_name("purposes", name)?;
        self.c.execute("INSERT INTO purposes (name, sort) VALUES (?1, (SELECT COALESCE(MAX(sort), 0) + 1 FROM purposes))", [name])?;
        Ok(self.c.last_insert_rowid())
    }

    pub fn update_purpose(&self, p: &Purpose) -> Result<()> {
        self.c
            .execute(
                "UPDATE purposes SET name = ?2, sort = ?3, track_todos = ?4, watch_upstream = ?5, show_in_overview = ?6, scan_code_todos = ?7
                 WHERE id = ?1",
                params![p.id, p.name, p.sort, p.track_todos, p.watch_upstream, p.show_in_overview, p.scan_code_todos],
            )
            .map(|_| ())
    }

    /// Delete a purpose; its projects stay, without a purpose.
    pub fn delete_purpose(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM purposes WHERE id = ?1", [id]).map(|_| ())
    }

    fn free_name(&self, table: &str, name: &str) -> Result<String> {
        let base = if name.trim().is_empty() { "Untitled" } else { name.trim() };
        let mut n = 1;
        let mut candidate = base.to_string();
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE name = ?1");
        while self.c.query_row(&sql, [&candidate], |r| r.get::<_, i64>(0))? > 0 {
            n += 1;
            candidate = format!("{base} ({n})");
        }
        Ok(candidate)
    }

    // --- scan roots, ignore list, settings ------------------------------------------------------------------

    pub fn scan_roots(&self) -> Result<Vec<(String, u32)>> {
        self.c.prepare("SELECT path, depth FROM scan_roots ORDER BY path")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }

    pub fn set_scan_root(&self, path: &str, depth: u32) -> Result<()> {
        self.c.execute("INSERT OR REPLACE INTO scan_roots (path, depth) VALUES (?1, ?2)", params![path, depth]).map(|_| ())
    }

    pub fn remove_scan_root(&self, path: &str) -> Result<()> {
        self.c.execute("DELETE FROM scan_roots WHERE path = ?1", [path]).map(|_| ())
    }

    pub fn ignored(&self) -> Result<Vec<String>> {
        self.c.prepare("SELECT path FROM ignored_paths ORDER BY path")?.query_map([], |r| r.get(0))?.collect()
    }

    pub fn ignore(&self, path: &str) -> Result<()> {
        self.c.execute("INSERT OR IGNORE INTO ignored_paths (path) VALUES (?1)", [path]).map(|_| ())
    }

    pub fn unignore(&self, path: &str) -> Result<()> {
        self.c.execute("DELETE FROM ignored_paths WHERE path = ?1", [path]).map(|_| ())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.c.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)", params![key, value]).map(|_| ())
    }
}

fn checkout_row(r: &rusqlite::Row) -> Result<Checkout> {
    let info: Option<String> = r.get(4)?;
    Ok(Checkout {
        id: r.get(0)?,
        project_id: r.get(1)?,
        path: r.get(2)?,
        primary: r.get(3)?,
        // ponytail: an unreadable cached info (older format) is just dropped; the next refresh rewrites it.
        info: info.and_then(|s| serde_json::from_str(&s).ok()),
        info_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let purposes = s.purposes().unwrap();
        assert_eq!(purposes[0].name, "Developing");
        let g = s.add_group("Hobby", Some(r"C:\Code\Hobby")).unwrap();
        assert_eq!(s.add_group("Hobby", None).unwrap(), g + 1);
        assert_eq!(s.groups().unwrap()[1].name, "Hobby (2)");

        let id = s.add_project("trailmix-web", Some(g), Some(purposes[0].id), &["b".into(), "a".into()]).unwrap();
        let mut p = s.projects().unwrap().remove(0);
        assert_eq!(p.checkouts.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), ["b", "a"], "primary first");
        p.status = Status::Paused;
        p.tags = vec!["web".into(), "svelte".into()];
        s.update_project(&p).unwrap();
        let p = s.projects().unwrap().remove(0);
        assert_eq!((p.status, p.tags.as_slice()), (Status::Paused, &["svelte".to_string(), "web".to_string()][..]));

        s.set_primary(p.checkouts[1].id).unwrap();
        assert_eq!(s.projects().unwrap()[0].primary().unwrap().path, "a");
        let info = Info { version: Some("1.2.3".into()), ..Default::default() };
        s.set_info(p.checkouts[0].id, &info).unwrap();
        assert_eq!(s.checkout_by_path("b").unwrap().unwrap().info, Some(info));

        s.delete_group(g).unwrap();
        assert_eq!(s.projects().unwrap()[0].group_id, None);
        s.remove_project(id).unwrap();
        assert!(s.checkout_by_path("a").unwrap().is_none(), "checkouts cascade");
    }

    #[test]
    fn migrations_run_once() {
        let dir = std::env::temp_dir().join(format!("pa-db-{}", std::process::id()));
        let file = dir.join("pa.db");
        Store::open(&file).unwrap().set_setting("k", "v").unwrap();
        let s = Store::open(&file).unwrap();
        assert_eq!(s.setting("k").unwrap().as_deref(), Some("v"));
        assert_eq!(s.purposes().unwrap().len(), 5);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_a_v2_file_to_the_todo_style() {
        let dir = std::env::temp_dir().join(format!("pa-db-v2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pa.db");
        {
            // A file as step 2 left it: the notes table exists, user_version is 2.
            let c = Connection::open(&file).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.execute_batch(MIGRATIONS[1]).unwrap();
            c.execute("INSERT INTO projects (name, added_at) VALUES ('old', 1)", []).unwrap();
            c.pragma_update(None, "user_version", 2).unwrap();
        }
        let s = Store::open(&file).unwrap();
        let mut p = s.projects().unwrap().remove(0);
        assert_eq!((p.name.as_str(), p.todo_path.as_deref(), p.todo_style.as_deref()), ("old", None, None));
        p.todo_style = Some("bullet:move:Done recently".into());
        s.update_project(&p).unwrap();
        let p = s.projects().unwrap().remove(0);
        assert_eq!(p.todo_style.as_deref(), Some("bullet:move:Done recently"));
        assert!(crate::todo::Style::decode("bullet:move:Done recently").is_some());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_a_v1_file() {
        let dir = std::env::temp_dir().join(format!("pa-db-v1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pa.db");
        {
            // A file as step 1 left it.
            let c = Connection::open(&file).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.execute("INSERT INTO projects (name, added_at) VALUES ('old', 1)", []).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
        }
        let s = Store::open(&file).unwrap();
        let mut p = s.projects().unwrap().remove(0);
        assert_eq!((p.name.as_str(), p.todo_path.as_deref(), p.roadmap_path.as_deref()), ("old", None, None));
        p.todo_path = Some("docs/TODO.md".into());
        s.update_project(&p).unwrap();
        assert_eq!(s.projects().unwrap()[0].todo_path.as_deref(), Some("docs/TODO.md"));

        assert!(s.notes(None).unwrap().is_empty());
        let n = s.add_note(Some(p.id), "first").unwrap();
        let g = s.add_note(None, "global").unwrap();
        let mut note = s.notes(Some(p.id)).unwrap().remove(0);
        assert_eq!((note.id, note.body.as_str()), (n, ""));
        note.body = "text".into();
        s.update_note(&note).unwrap();
        assert_eq!(s.notes(Some(p.id)).unwrap()[0].body, "text");
        assert_eq!(s.notes(None).unwrap()[0].id, g);
        s.remove_project(p.id).unwrap();
        assert!(s.notes(Some(p.id)).unwrap().is_empty(), "notes cascade with their project");
        s.delete_note(g).unwrap();
        assert!(s.notes(None).unwrap().is_empty());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
