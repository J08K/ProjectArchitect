//! Discovery (SPEC § 4): walk the scan roots and suggest projects and groups. Nothing is tracked until the user
//! confirms in the Import view. Read-only.

use crate::info::{self, language, skip_dir};
use std::collections::HashSet;
use std::path::Path;

/// Manifests that make a folder a project even without git or top-level source files.
const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
    "pom.xml",
    "build.gradle",
    "CMakeLists.txt",
    "Makefile",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub path: String,
    pub name: String,
    pub git: bool,
    pub remote: Option<String>,
    pub root_commit: Option<String>,
}

/// A folder that is not a project itself but holds two or more (e.g. `Code\Hobby`).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupSuggestion {
    pub path: String,
    pub name: String,
    /// Has a README.md or CLAUDE.md, so it looks like a deliberate grouping folder; pre-selected in the Import view.
    /// ponytail: heuristic; a plain "Repos" folder without a README is offered but not pre-selected.
    pub curated: bool,
    /// Candidate paths directly inside it (tracked ones included).
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanResult {
    /// Untracked candidates only.
    pub candidates: Vec<Candidate>,
    pub groups: Vec<GroupSuggestion>,
}

/// Is this folder a project (D-18): `.git`, a manifest or a top-level source file.
pub fn is_project_dir(dir: &Path) -> bool {
    if dir.join(".git").exists() || MANIFESTS.iter().any(|m| dir.join(m).is_file()) {
        return true;
    }
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| e.file_type().is_ok_and(|t| t.is_file()) && language(&e.file_name().to_string_lossy()).is_some())
    })
}

/// Walk `roots` (path, depth). `ignored` paths and everything below them are skipped; `tracked` paths are walked
/// (for nested repositories and group membership) but not returned as candidates.
pub fn scan(roots: &[(String, u32)], ignored: &[String], tracked: &HashSet<String>) -> ScanResult {
    let ignored: HashSet<String> = ignored.iter().map(|p| p.to_lowercase()).collect();
    let mut out = ScanResult::default();
    let mut seen = HashSet::new();
    for (root, depth) in roots {
        walk(Path::new(root), *depth, false, &ignored, tracked, &mut seen, &mut out);
    }
    // Shallow first: of several clones, the import makes the first one primary, and a copy nested inside another
    // repo (`Archive\trailmix`) is rarely the main one.
    out.candidates.sort_by_cached_key(|c| (Path::new(&c.path).components().count(), c.path.to_lowercase()));
    out
}

/// Returns whether `dir` is a project. Inside a project only nested git repositories count (a clone kept inside
/// another repo, e.g. `Archive\trailmix-web`), not every sub-package with its own manifest.
fn walk(
    dir: &Path,
    depth: u32,
    inside_project: bool,
    ignored: &HashSet<String>,
    tracked: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut ScanResult,
) -> bool {
    let path = crate::norm(dir);
    if ignored.contains(&path.to_lowercase()) || !seen.insert(path.to_lowercase()) {
        return false;
    }
    let has_git = dir.join(".git").exists();
    let is_project = if inside_project { has_git } else { is_project_dir(dir) };
    if is_project && !tracked.contains(&path) {
        let (remote, root_commit) = if has_git { info::identity(dir) } else { (None, None) };
        out.candidates.push(Candidate { name: crate::folder_name(&path), path: path.clone(), git: has_git, remote, root_commit });
    }
    if depth == 0 {
        return is_project;
    }
    let mut members = Vec::new();
    let mut children: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect())
        .unwrap_or_default();
    children.sort();
    for child in children {
        let name = child.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if skip_dir(&name) {
            continue;
        }
        if walk(&child, depth - 1, inside_project || is_project, ignored, tracked, seen, out) {
            members.push(crate::norm(&child));
        }
    }
    if !is_project && !inside_project && members.len() >= 2 {
        let curated = dir.join("README.md").is_file() || dir.join("CLAUDE.md").is_file();
        out.groups.push(GroupSuggestion { name: crate::folder_name(&path), path, curated, members });
    }
    is_project
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_projects_and_groups() {
        let root = std::env::temp_dir().join(format!("pa-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mk = |p: &str| fs::create_dir_all(root.join(p)).unwrap();
        let file = |p: &str| fs::write(root.join(p), "x").unwrap();
        // A group like Hobby: README, two repos, a loose script folder.
        mk("Repos/Tools/A/.git");
        mk("Repos/Tools/B/.git");
        mk("Repos/Tools/B/crates/x");
        file("Repos/Tools/B/crates/x/Cargo.toml");
        mk("Repos/Tools/ideas");
        file("Repos/Tools/ideas/index.html");
        file("Repos/Tools/README.md");
        // A nested clone inside a repo, and a sub-package that is not one.
        mk("Repos/Mig/.git");
        mk("Repos/Mig/inner/.git");
        // Not projects: docs only, dependency folders, ignored.
        mk("Docs");
        file("Docs/notes.txt");
        mk("Repos/Web/node_modules/pkg");
        file("Repos/Web/package.json");
        file("Repos/Web/node_modules/pkg/package.json");
        mk("Repos/Skip/.git");

        let base = crate::norm(&root);
        let p = |s: &str| format!("{base}{}{}", std::path::MAIN_SEPARATOR, s.replace('/', std::path::MAIN_SEPARATOR_STR));
        let tracked: HashSet<String> = [p("Repos/Tools/A")].into();
        let r = scan(&[(base.clone(), 3)], &[p("Repos/Skip")], &tracked);
        let mut found: Vec<&str> = r.candidates.iter().map(|c| c.path.strip_prefix(&base).unwrap()).collect();
        found.sort();
        let sep = |s: &str| s.replace('/', std::path::MAIN_SEPARATOR_STR);
        assert_eq!(
            found,
            [sep("/Repos/Mig"), sep("/Repos/Mig/inner"), sep("/Repos/Tools/B"), sep("/Repos/Tools/ideas"), sep("/Repos/Web")]
        );
        let tools = r.groups.iter().find(|g| g.name == "Tools").expect("Tools group");
        assert!(tools.curated);
        assert_eq!(tools.members.len(), 3, "tracked A counts as a member");
        let repos = r.groups.iter().find(|g| g.name == "Repos").expect("Repos group");
        assert!(!repos.curated);
        assert!(!r.groups.iter().any(|g| g.name == "Mig"), "a project is never a group");
        let _ = fs::remove_dir_all(root);
    }
}
