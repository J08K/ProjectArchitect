//! ProjectArchitect core: the SQLite store, the folder scanner and per-checkout project info.
//! No UI. The GUI (`apps/pa`) and the MCP server (M3) both go through this crate.

pub mod claude;
pub mod codetodo;
pub mod db;
pub mod deps;
pub mod git;
pub mod github;
pub mod history;
pub mod info;
pub mod install;
pub mod loc;
pub mod ops;
pub mod query;
pub mod roadmap;
pub mod scan;
pub mod tasks;
pub mod template;
pub mod todo;
pub mod update;

use std::path::{Path, PathBuf};
use std::process::Command;

pub use codetodo::CodeTodo;
pub use db::{Checkout, Group, Note, Project, Purpose, Status, Store};
pub use info::Info;
pub use roadmap::{EntryKind, Milestone, MilestoneStatus, Roadmap};
pub use todo::{Completion, Marker, SaveError, Section, Style, TodoDoc, TodoError, TodoItem};

/// A command that never flashes a console window when started from the GUI (Windows `CREATE_NO_WINDOW`).
pub fn cmd(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// Absolute, canonical path as stored in the database (no `\\?\` prefix); the input as-is when it does not exist.
pub fn norm(p: &Path) -> String {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// The folder's last path component, used as the default project name.
pub fn folder_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// App data folder: `%APPDATA%\ProjectArchitect` on Windows, else `$XDG_DATA_HOME/projectarchitect`
/// (`~/.local/share/projectarchitect`).
pub fn default_data_dir() -> PathBuf {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA").unwrap_or_default().join("ProjectArchitect")
    } else {
        var("XDG_DATA_HOME").unwrap_or_else(|| var("HOME").unwrap_or_default().join(".local").join("share")).join("projectarchitect")
    }
}

/// Run a launcher template (`{path}` replaced by the folder) through the shell, detached.
/// Windows: `cmd /S /C "<command>"`, so the template's own quotes are kept exactly as written.
pub fn launch(template: &str, path: &str) -> std::io::Result<()> {
    let line = template.replace("{path}", path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd("cmd").raw_arg(format!("/S /C \"{line}\"")).current_dir(path).spawn()?;
    }
    #[cfg(not(windows))]
    cmd("sh").arg("-c").arg(line).current_dir(path).spawn()?;
    Ok(())
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date `(year, month, day)` of days since 1970-01-01 (the inverse of [`days_from_civil`]).
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Days since 1970-01-01 of an ISO date or date-time (`2026-10-16`, `2026-10-16T12:00:00+02:00`).
pub fn iso_days(s: &str) -> Option<i64> {
    let mut it = s.get(..10)?.split('-').map(|p| p.parse::<i64>().ok());
    Some(days_from_civil(it.next()??, it.next()??, it.next()??))
}

/// Today as days since 1970-01-01 (UTC).
pub fn today_days() -> i64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    (secs / 86_400) as i64
}

/// Seconds since 1970-01-01.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(iso_days("2026-10-16T09:00:00+02:00"), Some(days_from_civil(2026, 10, 16)));
        assert_eq!(iso_days("bad"), None);
        for d in [-1, 0, 11_017, 20_742, days_from_civil(2024, 2, 29)] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        assert_eq!(civil_from_days(days_from_civil(2026, 10, 19)), (2026, 10, 19));
    }

    #[test]
    fn names() {
        let path = if cfg!(windows) { r"C:\Code\Repos\garden" } else { "/home/me/Code/garden" };
        assert_eq!(folder_name(path), "garden");
    }
}
