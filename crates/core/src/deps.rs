//! Dependencies between tracked projects (SPEC § 7.10, D-19): "this project uses that one at version V", kept in the
//! store with how to read the used version (a file and a regex, or a fixed value) and the latest one (the other
//! project's version, or its newest tag matching a glob). Versions compare numerically per dot-separated part.
//! ponytail: defined by hand only, no detection from manifests; add it if hand entry gets tedious.

use crate::db::{Project, Result, Store};
use rusqlite::params;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dependency {
    pub id: i64,
    /// The project that uses the other one.
    pub project_id: i64,
    pub depends_on: i64,
    /// File in this project's checkout holding the used version, read with `used_pattern`.
    pub used_file: Option<String>,
    /// A regex; its first capture group (else the whole match) is the version: `spec-v([0-9.]+)`.
    pub used_pattern: Option<String>,
    /// A fixed used version, when no file says it.
    pub used_fixed: Option<String>,
    /// Glob of the other project's tags (`spec-v*`); `None` = the other project's version (manifest, else latest tag).
    pub latest_tag: Option<String>,
}

const COLUMNS: &str = "id, project_id, depends_on, used_file, used_pattern, used_fixed, latest_tag";

fn row(r: &rusqlite::Row) -> Result<Dependency> {
    Ok(Dependency {
        id: r.get(0)?,
        project_id: r.get(1)?,
        depends_on: r.get(2)?,
        used_file: r.get(3)?,
        used_pattern: r.get(4)?,
        used_fixed: r.get(5)?,
        latest_tag: r.get(6)?,
    })
}

impl Store {
    /// What `project_id` depends on.
    pub fn dependencies(&self, project_id: i64) -> Result<Vec<Dependency>> {
        self.c.prepare(&format!("SELECT {COLUMNS} FROM dependencies WHERE project_id = ?1 ORDER BY id"))?.query_map([project_id], row)?.collect()
    }

    /// What depends on `project_id`.
    pub fn dependents(&self, project_id: i64) -> Result<Vec<Dependency>> {
        self.c.prepare(&format!("SELECT {COLUMNS} FROM dependencies WHERE depends_on = ?1 ORDER BY id"))?.query_map([project_id], row)?.collect()
    }

    pub fn all_dependencies(&self) -> Result<Vec<Dependency>> {
        self.c.prepare(&format!("SELECT {COLUMNS} FROM dependencies ORDER BY id"))?.query_map([], row)?.collect()
    }

    /// Add or replace the dependency of `d.project_id` on `d.depends_on` (one per pair). Returns its id.
    pub fn set_dependency(&self, d: &Dependency) -> Result<i64> {
        self.c.execute(
            "INSERT INTO dependencies (project_id, depends_on, used_file, used_pattern, used_fixed, latest_tag) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (project_id, depends_on) DO UPDATE SET used_file = ?3, used_pattern = ?4, used_fixed = ?5, latest_tag = ?6",
            params![d.project_id, d.depends_on, d.used_file, d.used_pattern, d.used_fixed, d.latest_tag],
        )?;
        self.c.query_row("SELECT id FROM dependencies WHERE project_id = ?1 AND depends_on = ?2", [d.project_id, d.depends_on], |r| r.get(0))
    }

    pub fn delete_dependency(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM dependencies WHERE id = ?1", [id]).map(|_| ())
    }
}

/// How the used version stands against the latest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Current,
    /// Used is older: the warning case.
    Behind,
    /// Used is newer than the latest (an unreleased version).
    Ahead,
    /// Not comparable as numbers and not equal.
    Differs,
    /// A version could not be read.
    Unknown,
}

/// A dependency's versions as read now.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub used: std::result::Result<String, String>,
    pub latest: std::result::Result<String, String>,
    pub standing: Standing,
}

/// The used version: the fixed value, else the pattern's match in the file (relative to `this_root`).
pub fn used_version(d: &Dependency, this_root: &Path) -> std::result::Result<String, String> {
    if let Some(v) = d.used_fixed.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        return Ok(v.to_string());
    }
    let (Some(file), Some(pattern)) = (d.used_file.as_deref(), d.used_pattern.as_deref()) else {
        return Err("no used version: give a fixed value, or a file and a pattern".into());
    };
    let re = regex::Regex::new(pattern).map_err(|e| format!("pattern: {e}"))?;
    let path = this_root.join(file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let caps = re.captures(&text).ok_or_else(|| format!("{pattern} does not match {file}"))?;
    Ok(caps.get(1).or_else(|| caps.get(0)).map_or("", |m| m.as_str()).trim().to_string())
}

/// The latest version: the newest tag matching the glob (git's version sort), else the project's version (manifest,
/// else latest tag; SPEC § 5).
pub fn latest_version(d: &Dependency, other_root: &Path) -> std::result::Result<String, String> {
    let git = |args: &[&str]| crate::info::git(other_root, args).filter(|s| !s.is_empty());
    match d.latest_tag.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
        Some(glob) => git(&["tag", "--list", glob, "--sort=-v:refname"])
            .and_then(|t| t.lines().next().map(str::to_string))
            .ok_or_else(|| format!("no tag matches {glob}")),
        None => crate::info::manifest_version(other_root)
            .or_else(|| git(&["describe", "--tags", "--abbrev=0"]))
            .ok_or_else(|| "the project has no version (manifest or tag)".to_string()),
    }
}

pub fn check(d: &Dependency, this_root: &Path, other_root: &Path) -> Check {
    let (used, latest) = (used_version(d, this_root), latest_version(d, other_root));
    let standing = match (&used, &latest) {
        (Ok(u), Ok(l)) => compare(u, l),
        _ => Standing::Unknown,
    };
    Check { used, latest, standing }
}

/// [`check`] in the primary checkouts of both projects; `None` when one of them is gone or has no checkout.
pub fn check_projects(projects: &[Project], d: &Dependency) -> Option<Check> {
    let root = |id: i64| projects.iter().find(|p| p.id == id).and_then(|p| p.primary()).map(|c| PathBuf::from(&c.path));
    Some(check(d, &root(d.project_id)?, &root(d.depends_on)?))
}

/// Refuses what cannot be checked: a dependency on itself, no used version, a pattern that is not a regex.
pub fn validate(d: &Dependency) -> std::result::Result<(), String> {
    if d.project_id == d.depends_on {
        return Err("a project cannot depend on itself".into());
    }
    let fixed = d.used_fixed.as_deref().is_some_and(|v| !v.trim().is_empty());
    match (fixed, d.used_file.as_deref().filter(|f| !f.trim().is_empty()), d.used_pattern.as_deref().filter(|p| !p.is_empty())) {
        (true, _, _) => Ok(()),
        (false, Some(_), Some(pattern)) => regex::Regex::new(pattern).map(|_| ()).map_err(|e| format!("pattern: {e}")),
        _ => Err("give the used version: a fixed value, or a file and a pattern".into()),
    }
}

/// Compare the numeric parts after any prefix and before any suffix (`spec-v10.1` vs `spec-v11-b2512`): missing parts
/// are 0, so `1.0` equals `1`. Equal numbers with different suffixes (`1.2.0-beta`, `1.2.0`) differ.
pub fn compare(used: &str, latest: &str) -> Standing {
    match (numbers(used), numbers(latest)) {
        (Some((mut a, ra)), Some((mut b, rb))) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            match a.cmp(&b) {
                Ordering::Less => Standing::Behind,
                Ordering::Greater => Standing::Ahead,
                Ordering::Equal if ra == rb => Standing::Current,
                Ordering::Equal => Standing::Differs,
            }
        }
        _ if used.trim() == latest.trim() => Standing::Current,
        _ => Standing::Differs,
    }
}

/// The dot-separated numbers from the first digit on, and what follows them: `spec-v10.1-b25` → ([10, 1], "-b25").
fn numbers(v: &str) -> Option<(Vec<u64>, &str)> {
    let v = &v[v.find(|c: char| c.is_ascii_digit())?..];
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(v.len());
    let (num, rest) = v.split_at(end);
    Some((num.trim_end_matches('.').split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_comparison() {
        assert_eq!(compare("10.1", "11"), Standing::Behind);
        assert_eq!(compare("spec-v10.1", "spec-v10.1"), Standing::Current);
        assert_eq!(compare("v1.0", "1"), Standing::Current);
        assert_eq!(compare("0.10.0", "0.9.3"), Standing::Ahead);
        assert_eq!(compare("1.2.0-beta", "1.2.0"), Standing::Differs);
        assert_eq!(compare("main", "dev"), Standing::Differs);
        assert_eq!(compare("main", "main"), Standing::Current);
        // A SPEC_VERSION constant against another project's `spec-v` tags: build stamps after the numbers.
        assert_eq!(compare("10.1", "spec-v11-b25122525"), Standing::Behind);
        assert_eq!(compare("spec-v11-b25122525", "spec-v11-b25122525"), Standing::Current);
        assert_eq!(compare("v2.", "2"), Standing::Current);
    }

    #[test]
    fn store_and_check_against_files_and_tags() {
        let base = std::env::temp_dir().join(format!("pa-deps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (app, spec) = (base.join("app"), base.join("spec"));
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&spec).unwrap();
        std::fs::write(app.join("SPEC_VERSION"), "built against spec-v10.1\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"],
            vec!["tag", "spec-v9"],
            vec!["tag", "spec-v11"],
        ] {
            assert!(crate::cmd("git").args(&args).current_dir(&spec).status().unwrap().success());
        }

        let s = Store::open_in_memory().unwrap();
        let a = s.add_project("app", None, None, &[app.display().to_string()]).unwrap();
        let b = s.add_project("spec", None, None, &[spec.display().to_string()]).unwrap();
        let mut d = Dependency {
            project_id: a,
            depends_on: b,
            used_file: Some("SPEC_VERSION".into()),
            used_pattern: Some(r"spec-v([0-9.]+)".into()),
            latest_tag: Some("spec-v*".into()),
            ..Default::default()
        };
        d.id = s.set_dependency(&d).unwrap();
        let c = check(&d, &app, &spec);
        assert_eq!((c.used.as_deref(), c.latest.as_deref(), c.standing), (Ok("10.1"), Ok("spec-v11"), Standing::Behind));

        d.latest_tag = None;
        d.used_fixed = Some("2.0".into());
        std::fs::write(spec.join("Cargo.toml"), "[package]\nversion = \"2.0.0\"\n").unwrap();
        assert_eq!(s.set_dependency(&d).unwrap(), d.id, "one dependency per pair");
        let stored = s.dependencies(a).unwrap();
        assert_eq!(stored, [d.clone()]);
        assert_eq!(s.dependents(b).unwrap(), [d.clone()]);
        let c = check(&d, &app, &spec);
        assert_eq!((c.used.as_deref(), c.latest.as_deref(), c.standing), (Ok("2.0"), Ok("2.0.0"), Standing::Current), "the manifest version");

        d.used_fixed = None;
        d.used_pattern = Some("nope([0-9]+)".into());
        assert_eq!(check(&d, &app, &spec).standing, Standing::Unknown);
        d.used_pattern = Some("(".into());
        assert!(used_version(&d, &app).unwrap_err().starts_with("pattern"));
        assert!(validate(&d).unwrap_err().starts_with("pattern"));
        d.used_pattern = None;
        assert!(validate(&d).is_err(), "no used version");
        d.used_fixed = Some("1".into());
        assert!(validate(&d).is_ok());
        assert!(validate(&Dependency { project_id: a, depends_on: a, used_fixed: Some("1".into()), ..Default::default() }).is_err());
        let projects = s.projects().unwrap();
        assert_eq!(check_projects(&projects, &d).map(|c| c.used), Some(Ok("1".to_string())));

        s.remove_project(b).unwrap();
        assert!(s.all_dependencies().unwrap().is_empty(), "removing either project removes the dependency");
        let _ = std::fs::remove_dir_all(base);
    }
}
