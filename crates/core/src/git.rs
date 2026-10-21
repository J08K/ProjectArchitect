//! Git and GitHub through the `git` and `gh` CLIs (CLAUDE.md): commit one file to a branch without touching the
//! working tree or the real index, push it, and open a pull request; the Git panel's reads and actions (SPEC § 7.5).
//! Errors carry git's own message.

use crate::cmd;
use std::fmt;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

#[derive(Debug)]
pub struct GitError(pub String);

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

/// The local branch `repo` has checked out; `None` when detached or not a repository.
pub fn current_branch(repo: &Path) -> Option<String> {
    git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok().filter(|b| !b.is_empty())
}

/// Local branch names.
pub fn branches(repo: &Path) -> Result<Vec<String>, GitError> {
    Ok(git(repo, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])?.lines().map(str::to_string).collect())
}

/// Tag names (`git tag --list`).
pub fn tags(repo: &Path) -> Result<Vec<String>, GitError> {
    Ok(git(repo, &["tag", "--list"])?.lines().map(str::to_string).collect())
}

/// `origin/HEAD`'s branch, else `main` or `master` when one exists locally.
pub fn default_branch(repo: &Path) -> Option<String> {
    if let Ok(head) = git(repo, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        && let Some(branch) = head.strip_prefix("origin/")
    {
        return Some(branch.to_string());
    }
    let local = branches(repo).ok()?;
    ["main", "master"].into_iter().find(|b| local.iter().any(|l| l == b)).map(str::to_string)
}

/// An `origin` remote is configured.
pub fn has_remote(repo: &Path) -> bool {
    git(repo, &["remote"]).is_ok_and(|remotes| remotes.lines().any(|r| r == "origin"))
}

/// `gh --version` runs.
pub fn gh_available() -> bool {
    cmd("gh").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Commit `content` as `rel_path` on `branch` and return the new commit id.
/// - `branch` is checked out here: write the file atomically in the working tree, then `git commit --only` for that
///   path alone, whatever else is staged or dirty.
/// - otherwise: never touch the working tree or the real index. A temporary index holds `branch`'s tip (or `base`
///   when the branch does not exist), gets the file (`hash-object`, so autocrlf and filters apply), and the new
///   commit moves the branch with a compare-and-swap.
pub fn commit_file_to_branch(repo: &Path, rel_path: &str, content: &str, branch: &str, base: &str, message: &str) -> Result<String, GitError> {
    check_branch(repo, branch)?;
    check_path(rel_path)?;
    if current_branch(repo).as_deref() == Some(branch) {
        return commit_in_worktree(repo, rel_path, content, message);
    }
    commit_in_temp_index(repo, rel_path, content, branch, base, message)
}

/// `git push -u origin <branch>`. Not tested here: it needs a network remote.
pub fn push(repo: &Path, branch: &str) -> Result<(), GitError> {
    check_branch(repo, branch)?;
    git(repo, &["push", "-u", "origin", branch]).map(|_| ())
}

/// Opens a pull request `branch` into `base` and returns its URL. An existing PR for that head is returned as it is.
/// Not tested here beyond [`pr_create_args`]: it needs GitHub.
pub fn open_pr(repo: &Path, branch: &str, base: &str, title: &str, body: &str) -> Result<String, GitError> {
    check_branch(repo, branch)?;
    if let Ok(url) = exec("gh", Some(repo), &["pr", "view", branch, "--json", "url", "-q", ".url"], None, None)
        && !url.is_empty()
    {
        return Ok(url);
    }
    let args = pr_create_args(branch, base, title, body);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    exec("gh", Some(repo), &refs, None, None).map(|out| out.lines().last().unwrap_or_default().trim().to_string())
}

/// The arguments of `gh pr create`.
fn pr_create_args(branch: &str, base: &str, title: &str, body: &str) -> Vec<String> {
    ["pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body].map(String::from).to_vec()
}

fn commit_in_worktree(repo: &Path, rel_path: &str, content: &str, message: &str) -> Result<String, GitError> {
    let path = repo.join(rel_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_err)?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, content).map_err(io_err)?;
    std::fs::rename(&tmp, &path).map_err(io_err)?;
    // `add` only stages this path, so a new file can be committed with `--only`.
    git(repo, &["add", "--", rel_path])?;
    git(repo, &["commit", "--only", "-m", message, "--", rel_path])?;
    git(repo, &["rev-parse", "HEAD"])
}

fn commit_in_temp_index(repo: &Path, rel_path: &str, content: &str, branch: &str, base: &str, message: &str) -> Result<String, GitError> {
    let tip = resolve(repo, &format!("refs/heads/{branch}"));
    let parent = match &tip {
        Some(t) => t.clone(),
        None => resolve(repo, base).ok_or_else(|| GitError(format!("base {base:?} does not exist")))?,
    };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("pa-git-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    let index = dir.join("index");
    let result = build_commit(repo, &index, &parent, rel_path, content, message).and_then(|commit| {
        // ponytail: a missing branch is created with an all-zero old value, which git reads as "must not exist".
        let old = tip.clone().unwrap_or_else(|| "0".repeat(commit.len()));
        update_ref(repo, branch, &commit, &old).map(|()| commit)
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// A commit on top of `parent` with `rel_path` set to `content`, built in the temporary `index`.
fn build_commit(repo: &Path, index: &Path, parent: &str, rel_path: &str, content: &str, message: &str) -> Result<String, GitError> {
    exec("git", Some(repo), &["read-tree", parent], Some(index), None)?;
    let blob = git_stdin(repo, &["hash-object", "-w", "--path", rel_path, "--stdin"], content.as_bytes())?;
    exec("git", Some(repo), &["update-index", "--add", "--cacheinfo", &format!("100644,{blob},{rel_path}")], Some(index), None)?;
    let tree = exec("git", Some(repo), &["write-tree"], Some(index), None)?;
    git(repo, &["commit-tree", &tree, "-p", parent, "-m", message])
}

/// Moves `refs/heads/<branch>` from `old` to `new`; fails, changing nothing, when the branch is not at `old`.
fn update_ref(repo: &Path, branch: &str, new: &str, old: &str) -> Result<(), GitError> {
    git(repo, &["update-ref", &format!("refs/heads/{branch}"), new, old]).map(|_| ())
}

/// `branch` is a valid branch name. A leading `-` would read as an option, so it is refused first.
fn check_branch(repo: &Path, branch: &str) -> Result<(), GitError> {
    if branch.starts_with('-') || git(repo, &["check-ref-format", "--branch", branch]).is_err() {
        return Err(GitError(format!("{branch:?} is not a valid branch name")));
    }
    Ok(())
}

/// A relative path inside the repository: no root, no `..`.
fn check_path(rel_path: &str) -> Result<(), GitError> {
    let normal = !rel_path.is_empty() && Path::new(rel_path).components().all(|c| matches!(c, Component::Normal(_)));
    if normal { Ok(()) } else { Err(GitError(format!("{rel_path:?} is not a path inside the repository"))) }
}

/// The full commit id of `rev`, if it names a commit.
fn resolve(repo: &Path, rev: &str) -> Option<String> {
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).ok()
}

fn git(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    exec("git", Some(repo), args, None, None)
}

fn git_stdin(repo: &Path, args: &[&str], input: &[u8]) -> Result<String, GitError> {
    exec("git", Some(repo), args, None, Some(input))
}

/// Runs a program in `cwd` (`None`: the current directory) and returns its trimmed stdout. `index` sets
/// `GIT_INDEX_FILE`; `input` is written to stdin. Failures carry stderr (or stdout when stderr is empty).
pub(crate) fn exec(program: &str, cwd: Option<&Path>, args: &[&str], index: Option<&Path>, input: Option<&[u8]>) -> Result<String, GitError> {
    let mut c = cmd(program);
    c.args(args).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    if let Some(index) = index {
        c.env("GIT_INDEX_FILE", index);
    }
    let mut child = c.spawn().map_err(|e| GitError(format!("{program}: {e}")))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(bytes).map_err(io_err)?;
    }
    let out = child.wait_with_output().map_err(io_err)?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let text = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
    Err(GitError(if text.is_empty() { format!("{program} {} failed", args.first().unwrap_or(&"")) } else { text }))
}

fn io_err(e: io::Error) -> GitError {
    GitError(e.to_string())
}

// --- the Git panel (SPEC § 7.5) -------------------------------------------------------------------------------

/// One commit of `git log`.
#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    /// ISO date-time.
    pub date: String,
    pub author: String,
    pub subject: String,
}

/// The last `limit` commits of the checked-out branch, newest first.
pub fn log(repo: &Path, limit: usize) -> Result<Vec<Commit>, GitError> {
    let n = limit.to_string();
    let out = git(repo, &["log", "-n", &n, "--format=%h%x09%cI%x09%an%x09%s"])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            Some(Commit { hash: f.next()?.into(), date: f.next()?.into(), author: f.next()?.into(), subject: f.next()?.into() })
        })
        .collect())
}

/// A changed or untracked file: git's two-letter status (`" M"`, `"??"`, `"R "`) and the path.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub code: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: String,
    /// `None` when detached.
    pub branch: Option<String>,
    pub head: String,
    /// `git worktree lock`: git refuses to remove it.
    pub locked: bool,
    /// The folder is gone; removing it only prunes git's record.
    pub missing: bool,
    /// Changed and untracked files of a linked worktree: removing it needs force and loses them.
    pub changes: Option<usize>,
    /// Commits of a linked worktree's HEAD that the default branch lacks; `Some(0)` = merged.
    pub unmerged: Option<usize>,
    /// The date of a linked worktree's HEAD commit (ISO).
    pub last_commit: Option<String>,
    /// When a Claude Code session in the folder last wrote (seconds since the epoch). Filled by the caller
    /// (`claude::last_session`), not by [`snapshot`].
    pub claude: Option<i64>,
}

/// What the Git panel shows for one checkout.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead_behind: Option<(u32, u32)>,
    pub changes: Vec<Change>,
    pub branches: Vec<String>,
    pub worktrees: Vec<Worktree>,
    /// The last 20.
    pub commits: Vec<Commit>,
    /// Newest version first (`git tag --sort=-v:refname`).
    pub tags: Vec<String>,
}

/// The panel's data: a handful of `git` calls, so run it off the UI thread.
pub fn snapshot(repo: &Path) -> Result<Snapshot, GitError> {
    let status = git(repo, &["status", "--porcelain=v1"])?;
    let mut s = Snapshot { changes: parse_changes(&status), branch: current_branch(repo), ..Default::default() };
    s.upstream = upstream(repo);
    if s.upstream.is_some()
        && let Ok(counts) = git(repo, &["rev-list", "--left-right", "--count", "HEAD...@{u}"])
    {
        let mut it = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
        s.ahead_behind = Some((it.next().unwrap_or(0), it.next().unwrap_or(0)));
    }
    s.branches = branches(repo)?;
    s.worktrees = parse_worktrees(&git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default());
    // How the linked worktrees are used, for cleaning up: a few git calls each.
    let base = default_branch(repo);
    for w in s.worktrees.iter_mut().skip(1) {
        w.missing = !Path::new(&w.path).exists();
        if w.missing || w.head.is_empty() {
            continue;
        }
        w.changes = git(Path::new(&w.path), &["status", "--porcelain=v1"]).ok().map(|t| t.lines().count());
        w.last_commit = git(repo, &["log", "-1", "--format=%cI", &w.head]).ok();
        if let Some(b) = &base {
            w.unmerged = git(repo, &["rev-list", "--count", &format!("{b}..{}", w.head)]).ok().and_then(|n| n.parse().ok());
        }
    }
    // A repository without commits has no log yet.
    s.commits = log(repo, 20).unwrap_or_default();
    s.tags = git(repo, &["tag", "--list", "--sort=-v:refname"])?.lines().map(str::to_string).collect();
    Ok(s)
}

/// The checked-out branch's upstream (`origin/main`), if it has one.
fn upstream(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).ok().filter(|u| !u.is_empty())
}

/// `git status --porcelain=v1` lines.
fn parse_changes(status: &str) -> Vec<Change> {
    status.lines().filter(|l| l.len() > 3).map(|l| Change { code: l[..2].to_string(), path: l[3..].trim_matches('"').to_string() }).collect()
}

/// `git worktree list --porcelain`: blocks of `worktree <path>`, `HEAD <id>`, `branch refs/heads/<b>` or `detached`,
/// and `locked [reason]`. The main worktree comes first.
fn parse_worktrees(text: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(Worktree { path: path.replace('/', std::path::MAIN_SEPARATOR_STR), ..Default::default() });
        } else if let Some(w) = out.last_mut() {
            if let Some(b) = line.strip_prefix("branch ") {
                w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                w.head = h.to_string();
            } else if line == "locked" || line.starts_with("locked ") {
                w.locked = true;
            }
        }
    }
    out
}

/// Removes linked worktrees of `repo` by path: `git worktree remove` (`--force` discards their uncommitted changes;
/// git never removes a locked one), or `git worktree prune` when the folder is already gone. With `branches`, each
/// removed worktree's branch is deleted when merged (`git branch -d` keeps unmerged ones). Runs from the main
/// worktree; the main worktree itself is refused. One output per command; a failure does not stop the others.
pub fn remove_worktrees(repo: &Path, paths: &[String], force: bool, branches: bool) -> Vec<Output> {
    let list = parse_worktrees(&git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default());
    let Some(main) = list.first().map(|w| PathBuf::from(&w.path)) else {
        return vec![refused("git worktree list".into(), GitError(format!("{} is not a git repository", repo.display())))];
    };
    let same = |a: &str, b: &str| crate::norm(Path::new(a)).eq_ignore_ascii_case(&crate::norm(Path::new(b)));
    let mut out = Vec::new();
    for p in paths {
        let Some(w) = list.iter().skip(1).find(|w| same(&w.path, p)) else {
            out.push(refused(format!("git worktree remove {p}"), GitError(format!("{p} is not a linked worktree of this repository"))));
            continue;
        };
        let o = match (Path::new(&w.path).exists(), force) {
            (false, _) => run(&main, &["worktree", "prune"]),
            (true, false) => run(&main, &["worktree", "remove", &w.path]),
            (true, true) => run(&main, &["worktree", "remove", "--force", &w.path]),
        };
        let removed = o.ok;
        out.push(o);
        if let (true, true, Some(b)) = (removed, branches, &w.branch) {
            out.push(run(&main, &["branch", "-d", b]));
        }
    }
    out
}

/// A git command as the panel shows it: the command line, whether it worked, and stdout and stderr together
/// (git writes progress and most messages to stderr).
#[derive(Clone, Debug)]
pub struct Output {
    pub command: String,
    pub ok: bool,
    pub text: String,
}

/// Runs `git <args>` in `repo` for the panel. Never prompts in a terminal (`GIT_TERMINAL_PROMPT=0`); a credential
/// helper may still open its own window.
pub fn run(repo: &Path, args: &[&str]) -> Output {
    let command = format!("git {}", args.join(" "));
    let out = cmd("git").args(args).current_dir(repo).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).output();
    match out {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(err.trim_end());
            }
            Output { command, ok: out.status.success(), text }
        }
        Err(e) => Output { command, ok: false, text: format!("git: {e}") },
    }
}

fn refused(command: String, e: GitError) -> Output {
    Output { command, ok: false, text: e.0 }
}

/// `git fetch --all --prune`.
pub fn fetch(repo: &Path) -> Output {
    run(repo, &["fetch", "--all", "--prune"])
}

/// `git pull --ff-only`: never creates a merge commit; a diverged branch is reported, not merged.
pub fn pull(repo: &Path) -> Output {
    run(repo, &["pull", "--ff-only"])
}

/// `git push`, or `git push -u origin <branch>` when the branch has no upstream yet.
pub fn push_current(repo: &Path) -> Output {
    let Some(branch) = current_branch(repo) else {
        return refused("git push".into(), GitError("no branch is checked out (detached HEAD)".into()));
    };
    if upstream(repo).is_some() { run(repo, &["push"]) } else { run(repo, &["push", "-u", "origin", &branch]) }
}

/// `git switch <branch>`; git refuses when local changes would be overwritten.
pub fn switch(repo: &Path, branch: &str) -> Output {
    match check_branch(repo, branch) {
        Ok(()) => run(repo, &["switch", branch]),
        Err(e) => refused(format!("git switch {branch}"), e),
    }
}

/// The default folder of a new worktree: `<parent>/<folder>-<branch>`, next to the checkout.
pub fn worktree_path(repo: &Path, branch: &str) -> PathBuf {
    let folder = repo.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let safe: String = branch.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let name = if safe.is_empty() { format!("{folder}-worktree") } else { format!("{folder}-{safe}") };
    repo.parent().unwrap_or(repo).join(name)
}

/// `git worktree add <path> <branch>`, or `-b <branch>` (a new branch from HEAD) when `new`. The folder must not
/// exist yet.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, new: bool) -> Output {
    let p = path.to_string_lossy().to_string();
    let command = format!("git worktree add {p} {branch}");
    if let Err(e) = check_branch(repo, branch) {
        return refused(command, e);
    }
    if !path.is_absolute() || path.exists() {
        return refused(command, GitError(format!("{p} must be a new folder, given as a full path")));
    }
    if new { run(repo, &["worktree", "add", "-b", branch, &p]) } else { run(repo, &["worktree", "add", &p, branch]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository on `main` with one commit. The identity is set in the repository only.
    fn tmp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-git-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgsign", "false")] {
            git(&dir, &["config", k, v]).unwrap();
        }
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(&dir, &["add", "README.md"]).unwrap();
        git(&dir, &["commit", "-q", "-m", "init"]).unwrap();
        dir
    }

    #[test]
    fn commits_to_a_new_branch_without_touching_the_worktree() {
        let repo = tmp_repo("new");
        std::fs::write(repo.join("dirty.txt"), "untracked").unwrap();
        std::fs::write(repo.join("README.md"), "changed\n").unwrap();
        let status = git(&repo, &["status", "--porcelain"]).unwrap();
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        let id = commit_file_to_branch(&repo, "docs/TODO.md", "- [ ] a\n", "todo/x", "main", "Convert TODO").unwrap();

        assert_eq!(git(&repo, &["status", "--porcelain"]).unwrap(), status);
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git(&repo, &["rev-parse", "todo/x"]).unwrap(), id);
        assert_eq!(git(&repo, &["rev-parse", "todo/x^"]).unwrap(), head, "parent is the base");
        assert_eq!(git(&repo, &["show", "todo/x:docs/TODO.md"]).unwrap(), "- [ ] a");
        assert_eq!(current_branch(&repo).as_deref(), Some("main"));
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn commits_to_an_existing_branch_on_its_tip() {
        let repo = tmp_repo("existing");
        let main = git(&repo, &["rev-parse", "main"]).unwrap();
        let first = commit_file_to_branch(&repo, "TODO.md", "- [ ] a\n", "topic", "main", "one").unwrap();
        let second = commit_file_to_branch(&repo, "TODO.md", "- [x] a\n", "topic", "main", "two").unwrap();
        assert_eq!(git(&repo, &["rev-parse", "topic^"]).unwrap(), first);
        assert_eq!(git(&repo, &["rev-parse", "topic"]).unwrap(), second);
        assert_eq!(git(&repo, &["show", "topic:TODO.md"]).unwrap(), "- [x] a");
        assert_eq!(git(&repo, &["rev-parse", "main"]).unwrap(), main);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn commits_to_the_checked_out_branch_leaving_other_changes_alone() {
        let repo = tmp_repo("checked");
        std::fs::write(repo.join("README.md"), "dirty\n").unwrap();
        std::fs::write(repo.join("staged.txt"), "s\n").unwrap();
        git(&repo, &["add", "staged.txt"]).unwrap();
        let before = git(&repo, &["status", "--porcelain"]).unwrap();

        let id = commit_file_to_branch(&repo, "TODO.md", "- [ ] a\n", "main", "main", "Add TODO").unwrap();

        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap(), id);
        assert_eq!(git(&repo, &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]).unwrap(), "TODO.md");
        assert_eq!(std::fs::read_to_string(repo.join("TODO.md")).unwrap(), "- [ ] a\n");
        assert_eq!(git(&repo, &["status", "--porcelain"]).unwrap(), before, "README stays dirty, staged.txt stays staged");
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn rejects_bad_branch_names_and_paths() {
        let repo = tmp_repo("names");
        for bad in ["bad..name", "-x", "a b", "", "x.lock"] {
            assert!(commit_file_to_branch(&repo, "TODO.md", "x", bad, "main", "m").is_err(), "{bad:?}");
        }
        assert!(commit_file_to_branch(&repo, "../TODO.md", "x", "ok", "main", "m").is_err());
        assert!(commit_file_to_branch(&repo, "/abs.md", "x", "ok", "main", "m").is_err());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/ok"]).is_err(), "nothing was created");
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn update_ref_is_a_compare_and_swap() {
        let repo = tmp_repo("cas");
        let a = commit_file_to_branch(&repo, "TODO.md", "a\n", "topic", "main", "a").unwrap();
        let b = commit_file_to_branch(&repo, "TODO.md", "b\n", "topic", "main", "b").unwrap();
        // The branch is at `b`: an update that expects `a` must fail and leave the branch where it is.
        assert!(update_ref(&repo, "topic", &a, &a).is_err());
        assert_eq!(git(&repo, &["rev-parse", "topic"]).unwrap(), b);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn branch_queries() {
        let repo = tmp_repo("queries");
        assert_eq!(branches(&repo).unwrap(), ["main"]);
        assert_eq!(default_branch(&repo).as_deref(), Some("main"));
        assert!(!has_remote(&repo));
        assert_eq!(current_branch(&repo).as_deref(), Some("main"));
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A bare repository as `origin`, the clone `a` that pushed its first commit there, and a second clone `b`.
    /// Returns (base folder, a, b).
    fn with_origin(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("pa-git-origin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let origin = base.join("origin.git");
        git(&base, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]).unwrap();
        let a = tmp_repo(&format!("{name}-a"));
        git(&a, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
        let first = push_current(&a);
        assert!(first.ok, "first push sets the upstream: {}", first.text);
        let b = base.join("b");
        git(&base, &["clone", "-q", origin.to_str().unwrap(), b.to_str().unwrap()]).unwrap();
        for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgsign", "false")] {
            git(&b, &["config", k, v]).unwrap();
        }
        (base, a, b)
    }

    #[test]
    fn panel_actions_against_a_local_bare_remote() {
        let (base, a, b) = with_origin("panel");
        std::fs::write(a.join("new.txt"), "x\n").unwrap();
        git(&a, &["add", "new.txt"]).unwrap();
        git(&a, &["commit", "-q", "-m", "second"]).unwrap();
        let pushed = push_current(&a);
        assert!(pushed.ok, "{}", pushed.text);

        assert!(fetch(&b).ok);
        let s = snapshot(&b).unwrap();
        assert_eq!((s.branch.as_deref(), s.upstream.as_deref(), s.ahead_behind), (Some("main"), Some("origin/main"), Some((0, 1))));
        let pulled = pull(&b);
        assert!(pulled.ok, "{}", pulled.text);
        let s = snapshot(&b).unwrap();
        assert_eq!(s.ahead_behind, Some((0, 0)));
        assert_eq!(s.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "init"]);

        git(&b, &["branch", "topic"]).unwrap();
        assert!(switch(&b, "topic").ok);
        assert_eq!(current_branch(&b).as_deref(), Some("topic"));
        assert!(!switch(&b, "-f").ok, "options are not branch names");
        assert!(!switch(&b, "nope").ok);
        let topic = push_current(&b);
        assert!(topic.ok && topic.command == "git push -u origin topic", "{} {}", topic.command, topic.text);

        let wt = base.join("b-feature");
        let added = add_worktree(&b, &wt, "feature", true);
        assert!(added.ok, "{}", added.text);
        assert_eq!(current_branch(&wt).as_deref(), Some("feature"));
        assert!(!add_worktree(&b, &wt, "other", true).ok, "the folder exists");
        assert!(!add_worktree(&b, Path::new("relative"), "other", true).ok);
        let s = snapshot(&b).unwrap();
        assert_eq!(s.worktrees.len(), 2);
        assert_eq!(s.worktrees[1].branch.as_deref(), Some("feature"));
        assert_eq!(crate::norm(Path::new(&s.worktrees[1].path)), crate::norm(&wt));

        std::fs::write(b.join("README.md"), "changed\n").unwrap();
        std::fs::write(b.join("untracked.txt"), "u").unwrap();
        git(&b, &["tag", "v0.2.0"]).unwrap();
        git(&b, &["tag", "v0.10.0"]).unwrap();
        let s = snapshot(&b).unwrap();
        assert_eq!(s.changes, [Change { code: " M".into(), path: "README.md".into() }, Change { code: "??".into(), path: "untracked.txt".into() }]);
        assert_eq!(s.tags, ["v0.10.0", "v0.2.0"], "version order, newest first");
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(a);
    }

    #[test]
    fn worktree_folder_next_to_the_checkout() {
        let base = std::env::temp_dir();
        assert_eq!(worktree_path(&base.join("pa"), "feat/x y"), base.join("pa-feat-x-y"));
        assert_eq!(worktree_path(&base.join("pa"), ""), base.join("pa-worktree"));
    }

    #[test]
    fn worktree_list_parsing() {
        let w = parse_worktrees("worktree C:/x/main\nHEAD abc\nbranch refs/heads/main\n\nworktree C:/x/wt\nHEAD def\ndetached\n");
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].branch.as_deref(), w[1].branch.as_deref()), (Some("main"), None));
        assert!(w[0].path.ends_with("main"));
        assert_eq!((w[1].head.as_str(), w[1].locked), ("def", false));
        assert!(parse_worktrees("worktree /a\nHEAD 1\n\nworktree /b\nHEAD 2\nlocked claude\n")[1].locked);
    }

    #[test]
    fn worktrees_show_their_use_and_are_removed_safely() {
        let main = tmp_repo("wt-remove");
        let base = main.parent().unwrap().to_path_buf();
        let (done, busy) = (base.join(format!("pa-wt-done-{}", std::process::id())), base.join(format!("pa-wt-busy-{}", std::process::id())));
        for d in [&done, &busy] {
            let _ = std::fs::remove_dir_all(d);
        }
        assert!(add_worktree(&main, &done, "done", true).ok);
        assert!(add_worktree(&main, &busy, "busy", true).ok);
        std::fs::write(busy.join("work.txt"), "committed").unwrap();
        git(&busy, &["add", "work.txt"]).unwrap();
        git(&busy, &["commit", "-q", "-m", "work"]).unwrap();
        std::fs::write(busy.join("more.txt"), "uncommitted").unwrap();

        let s = snapshot(&main).unwrap();
        let w = |name: &str| s.worktrees.iter().find(|w| w.branch.as_deref() == Some(name)).unwrap().clone();
        assert_eq!((w("done").changes, w("done").unmerged), (Some(0), Some(0)), "clean and merged");
        assert_eq!((w("busy").changes, w("busy").unmerged), (Some(1), Some(1)));
        assert!(w("busy").last_commit.is_some() && s.worktrees[0].changes.is_none());

        let paths = [done.display().to_string(), busy.display().to_string(), main.display().to_string()];
        let out = remove_worktrees(&main, &paths, false, true);
        let shown: Vec<(&str, bool)> = out.iter().map(|o| (o.command.as_str(), o.ok)).collect();
        assert_eq!(shown.len(), 4, "{shown:?}");
        assert!(out[0].ok && out[1].command == "git branch -d done" && out[1].ok, "{shown:?}");
        assert!(!out[2].ok, "uncommitted changes need force: {}", out[2].text);
        assert!(!out[3].ok && out[3].text.contains("not a linked worktree"), "the main worktree stays");
        assert!(!done.exists() && busy.exists() && !branches(&main).unwrap().contains(&"done".to_string()));

        let out = remove_worktrees(&main, &[busy.display().to_string()], true, true);
        assert!(out[0].ok && !out[1].ok, "removed with force; the unmerged branch is kept: {}", out[1].text);
        assert!(!busy.exists() && branches(&main).unwrap().contains(&"busy".to_string()));
        let _ = std::fs::remove_dir_all(main);
    }

    #[test]
    fn pr_arguments() {
        assert_eq!(
            pr_create_args("todo/x", "main", "Convert TODO", "Body"),
            ["pr", "create", "--head", "todo/x", "--base", "main", "--title", "Convert TODO", "--body", "Body"]
        );
    }
}
