//! Progress over time (after YouTrack's reports), read from git: open and done items of a TODO or roadmap file at
//! its past commits (one point per day the file changed: that day's last commit), each roadmap entry's progress at
//! those points, and commits and Claude Code sessions per week. Everything here runs `git` many times: call it off
//! the UI thread.

use crate::info::git;
use crate::roadmap;
use crate::todo::{self, Style};
use std::path::Path;

/// Items of a file on one day (days since 1970-01-01).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub day: i64,
    pub open: usize,
    pub done: usize,
}

/// A roadmap entry's progress on one day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntryPoint {
    pub day: i64,
    pub done: usize,
    pub total: usize,
}

/// The texts of `rel` (relative to the repository) on the last `max` days it changed, oldest first.
fn versions(repo: &Path, rel: &str, max: usize) -> Vec<(i64, String)> {
    // The path as git tracks it: a case-insensitive file system finds `ROADMAP.md` for `roadmap.md`, git does not.
    let rel = rel.replace('\\', "/");
    let tracked = git(repo, &["ls-files", "--", &format!(":(icase){rel}")]).unwrap_or_default();
    let rel = tracked.lines().next().unwrap_or(&rel).to_string();
    let log = git(repo, &["log", "--format=%H %cI", "--", &rel]).unwrap_or_default();
    let mut days: Vec<(i64, &str)> = Vec::new();
    // Newest first: the first commit seen on a day is that day's last.
    for (hash, date) in log.lines().filter_map(|l| l.split_once(' ')) {
        let Some(day) = crate::iso_days(date) else { continue };
        if days.last().is_none_or(|d| d.0 != day) {
            days.push((day, hash));
        }
    }
    days.truncate(max);
    days.reverse();
    days.into_iter().filter_map(|(day, hash)| git(repo, &["show", &format!("{hash}:{rel}")]).map(|t| (day, t))).collect()
}

/// Open and done items of a TODO file over its history, read in `style`.
pub fn todo_history(repo: &Path, rel: &str, style: &Style, max: usize) -> Vec<Point> {
    versions(repo, rel, max)
        .into_iter()
        .map(|(day, text)| {
            let doc = todo::parse_with(&text, style);
            let done = doc.items.iter().filter(|i| i.done).count();
            Point { day, open: doc.items.len() - done, done }
        })
        .collect()
}

/// A roadmap over its history: all items, and each entry's progress (by version, else title) on the days it existed.
pub fn roadmap_history(repo: &Path, rel: &str, max: usize) -> (Vec<Point>, Vec<(String, Vec<EntryPoint>)>) {
    let mut points = Vec::new();
    let mut entries: Vec<(String, Vec<EntryPoint>)> = Vec::new();
    for (day, text) in versions(repo, rel, max) {
        let r = roadmap::parse_roadmap(&text);
        let done = r.doc.items.iter().filter(|i| i.done).count();
        points.push(Point { day, open: r.doc.items.len() - done, done });
        for m in &r.milestones {
            let key = m.version.clone().unwrap_or_else(|| m.title.clone());
            let p = EntryPoint { day, done: m.done, total: m.total };
            match entries.iter_mut().find(|e| e.0 == key) {
                Some(e) => e.1.push(p),
                None => entries.push((key, vec![p])),
            }
        }
    }
    (points, entries)
}

/// How many of `days` (days since 1970-01-01) fall in each of the last `weeks` weeks, oldest first; the last week
/// ends today.
pub fn per_week(days: impl IntoIterator<Item = i64>, weeks: usize) -> Vec<usize> {
    let today = crate::today_days();
    let mut out = vec![0; weeks];
    for d in days {
        let ago = (today - d).div_euclid(7);
        if (0..weeks as i64).contains(&ago) {
            out[weeks - 1 - ago as usize] += 1;
        }
    }
    out
}

/// Commits per week over the last `weeks` weeks, on every branch.
pub fn commits_per_week(repo: &Path, weeks: usize) -> Vec<usize> {
    let since = format!("--since={} days ago", weeks * 7);
    let log = git(repo, &["log", "--all", &since, "--format=%cI"]).unwrap_or_default();
    per_week(log.lines().filter_map(crate::iso_days), weeks)
}

/// Claude Code sessions per week over the last `weeks` weeks (by a transcript's last write), in these folders and
/// the Claude worktrees under them.
pub fn sessions_per_week(home: &Path, folders: &[String], weeks: usize) -> Vec<usize> {
    per_week(folders.iter().flat_map(|f| crate::claude::session_times(home, f)).map(|s| s.div_euclid(86_400)), weeks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A commit of `TODO.md` with `text`, dated `days_ago` days back.
    fn commit(dir: &Path, text: &str, days_ago: i64) {
        std::fs::write(dir.join("TODO.md"), text).unwrap();
        let secs = crate::now_secs() - days_ago * 86_400;
        let date = format!("@{secs} +0000");
        let run = |args: &[&str]| {
            let ok = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date).status().unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        run(&["add", "TODO.md"]);
        run(&["commit", "-q", "-m", "x"]);
    }

    #[test]
    fn counts_items_per_day_and_commits_per_week() {
        let dir = std::env::temp_dir().join(format!("pa-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.name", "t"], &["config", "user.email", "t@t"], &["config", "commit.gpgsign", "false"]] {
            assert!(Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
        }
        commit(&dir, "# TODO\n\n- [ ] a\n- [ ] b\n", 20);
        commit(&dir, "# TODO\n\n- [x] a\n- [ ] b\n- [ ] c\n", 9);
        commit(&dir, "# TODO\n\n- [x] a\n- [x] b\n- [ ] c\n", 9);
        commit(&dir, "# TODO\n\n- [x] a\n- [x] b\n- [x] c\n", 1);
        let h = todo_history(&dir, "TODO.md", &Style::CHECKLIST, 10);
        let counts: Vec<(usize, usize)> = h.iter().map(|p| (p.open, p.done)).collect();
        assert_eq!(counts, [(2, 0), (1, 2), (0, 3)], "one point per day, the day's last commit");
        assert!(h[0].day < h[1].day && h[1].day < h[2].day);
        assert_eq!(todo_history(&dir, "TODO.md", &Style::CHECKLIST, 2).len(), 2, "the newest days");
        assert_eq!(todo_history(&dir, "todo.md", &Style::CHECKLIST, 10).len(), 3, "the path as git tracks it");
        assert_eq!(commits_per_week(&dir, 4), [0, 1, 2, 1]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn weeks_end_today() {
        let t = crate::today_days();
        assert_eq!(per_week([t, t - 6, t - 7, t - 30, t + 3], 3), [0, 1, 2]);
    }
}
