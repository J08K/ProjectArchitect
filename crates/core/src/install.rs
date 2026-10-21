//! Replacing an installed binary that may be running. Windows will not overwrite or delete a running exe but does
//! rename it, so the new file goes in under the fixed name and the old one is set aside as `<name>.<secs>.old`;
//! [`remove_old`] deletes those once nothing runs them. Registrations (Claude Code, Codex) keep the fixed path.

use std::io;
use std::path::{Path, PathBuf};

/// Where the installed binaries live: `%LOCALAPPDATA%\Programs\ProjectArchitect` (Windows only; elsewhere the
/// package manager owns the location).
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("ProjectArchitect"))
}

/// Put `src` at `dest`, setting a running `dest` aside. `dest` is untouched if the copy fails.
pub fn replace(src: &Path, dest: &Path) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let new = sibling(dest, "new");
    std::fs::copy(src, &new)?;
    if dest.exists() {
        let old = sibling(dest, &format!("{}.old", crate::now_secs()));
        if let Err(e) = std::fs::rename(dest, &old) {
            let _ = std::fs::remove_file(&new);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&new, dest) {
            let _ = std::fs::rename(&old, dest);
            return Err(e);
        }
    } else {
        std::fs::rename(&new, dest)?;
    }
    remove_old(dest);
    Ok(())
}

/// Delete the set-aside copies of `exe` (`<name>.*.old` next to it) that are no longer running; locked ones stay
/// for the next call. Returns how many were removed.
pub fn remove_old(exe: &Path) -> usize {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|n| n.to_str())) else { return 0 };
    let prefix = format!("{name}.");
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".old")))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Clean up after an earlier update of the running program. Called at start by every binary.
pub fn remove_old_self() {
    if let Ok(exe) = std::env::current_exe() {
        remove_old(&exe);
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{suffix}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pa-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn replace_installs_sets_aside_and_cleans_up() {
        let d = dir("replace");
        let (src, dest) = (d.join("build.exe"), d.join("bin").join("tool.exe"));
        std::fs::write(&src, "v1").unwrap();
        replace(&src, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "v1");
        std::fs::write(&src, "v2").unwrap();
        replace(&src, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "v2");
        // Nothing runs the old copy here, so it is gone already; a locked one would stay until remove_old.
        let left: Vec<_> = std::fs::read_dir(d.join("bin")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["tool.exe"]);
    }

    #[test]
    fn remove_old_only_touches_this_binarys_copies() {
        let d = dir("old");
        for n in ["tool.exe", "tool.exe.1.old", "tool.exe.2.old", "other.exe.1.old", "tool.exe.new"] {
            std::fs::write(d.join(n), "").unwrap();
        }
        assert_eq!(remove_old(&d.join("tool.exe")), 2);
        let mut left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["other.exe.1.old", "tool.exe", "tool.exe.new"]);
    }

    #[test]
    fn a_failed_copy_leaves_the_installed_file() {
        let d = dir("fail");
        let dest = d.join("tool.exe");
        std::fs::write(&dest, "v1").unwrap();
        assert!(replace(&d.join("missing.exe"), &dest).is_err());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "v1");
    }
}
