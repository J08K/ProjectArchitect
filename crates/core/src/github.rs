//! GitHub through the `gh` CLI (SPEC § 7.6, D-7): repository info, creating a repository, and issues linked to TODO
//! items by a `(#12)` suffix on the item's first line. `gh` holds the login; the app never stores a token. Every
//! call names the repository (`-R owner/name`, from the `origin` remote). Parsing is kept apart from running `gh`, so
//! the tests need neither `gh` nor the network.

use crate::db::{Project, Store};
use crate::git::{GitError, exec};
use crate::ops::{self, CaptureKind, Captured};
use crate::todo::TodoDoc;
use serde_json::Value;
use std::path::Path;

/// Per-project setting key prefix: `capture.issues.<project id>` = `true` makes captures GitHub issues too.
pub const CAPTURE_ISSUES: &str = "capture.issues";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoInfo {
    /// `owner/name`.
    pub slug: String,
    pub url: String,
    /// `PUBLIC`, `PRIVATE` or `INTERNAL`.
    pub visibility: String,
    pub stars: u64,
    pub open_issues: u64,
    pub open_prs: u64,
    pub default_branch: Option<String>,
    pub latest_release: Option<Release>,
    /// The newest workflow run on the default branch.
    pub ci: Option<Ci>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub published: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ci {
    pub workflow: String,
    /// `completed`, `in_progress`, `queued`, …
    pub status: String,
    /// `success`, `failure`, `cancelled`, … (empty until completed).
    pub conclusion: String,
    pub url: String,
}

impl Ci {
    /// The conclusion once completed, else the status.
    pub fn state(&self) -> &str {
        if self.conclusion.is_empty() { &self.status } else { &self.conclusion }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    /// `OPEN` or `CLOSED`.
    pub state: String,
    pub url: String,
    pub labels: Vec<String>,
}

impl Issue {
    pub fn open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }
}

/// `owner/name` of the checkout's `origin` remote when it is on GitHub.
pub fn slug(repo: &Path) -> Option<String> {
    let remote = crate::info::git(repo, &["remote", "get-url", "origin"])?;
    let url = crate::info::github_url(&remote)?;
    url.strip_prefix("https://github.com/").map(str::to_string)
}

fn slug_or_err(repo: &Path) -> Result<String, GitError> {
    slug(repo).ok_or_else(|| GitError(format!("{} has no GitHub remote (origin)", repo.display())))
}

fn gh(args: &[&str]) -> Result<String, GitError> {
    exec("gh", None, args, None, None)
}

const REPO_FIELDS: &str = "nameWithOwner,url,visibility,stargazerCount,issues,pullRequests,defaultBranchRef,latestRelease";

/// Repository info and CI state (two `gh` calls; needs the network).
pub fn info(repo: &Path) -> Result<RepoInfo, GitError> {
    let slug = slug_or_err(repo)?;
    let json = gh(&["repo", "view", &slug, "--json", REPO_FIELDS])?;
    let mut info = parse_repo(&json).ok_or_else(|| GitError(format!("unexpected output from gh repo view: {json}")))?;
    if let Some(branch) = info.default_branch.clone()
        && let Ok(runs) = gh(&["run", "list", "-R", &slug, "--branch", &branch, "--limit", "10", "--json", "status,conclusion,workflowName,url,event"])
    {
        info.ci = parse_ci(&runs);
    }
    Ok(info)
}

fn parse_repo(json: &str) -> Option<RepoInfo> {
    let v: Value = serde_json::from_str(json).ok()?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let count = |k: &str| v.get(k).and_then(|x| x.get("totalCount")).and_then(Value::as_u64).unwrap_or(0);
    let release = v.get("latestRelease").filter(|r| r.is_object()).map(|r| Release {
        tag: s(r, "tagName"),
        name: s(r, "name"),
        url: s(r, "url"),
        published: s(r, "publishedAt"),
    });
    Some(RepoInfo {
        slug: s(&v, "nameWithOwner"),
        url: s(&v, "url"),
        visibility: s(&v, "visibility"),
        stars: v.get("stargazerCount").and_then(Value::as_u64).unwrap_or(0),
        open_issues: count("issues"),
        open_prs: count("pullRequests"),
        default_branch: v.get("defaultBranchRef").and_then(|b| b.get("name")).and_then(Value::as_str).filter(|b| !b.is_empty()).map(str::to_string),
        latest_release: release,
        ci: None,
    })
}

/// The newest run, preferring push-triggered workflows (CI) over scheduled or manual ones.
fn parse_ci(json: &str) -> Option<Ci> {
    let runs: Vec<Value> = serde_json::from_str(json).ok()?;
    let run = runs.iter().find(|r| r.get("event").and_then(Value::as_str) == Some("push")).or(runs.first())?;
    let s = |k: &str| run.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    Some(Ci { workflow: s("workflowName"), status: s("status"), conclusion: s("conclusion"), url: s("url") })
}

/// Issues in `state` (`open`, `closed` or `all`), newest first, at most 500.
pub fn issues(repo: &Path, state: &str) -> Result<Vec<Issue>, GitError> {
    let slug = slug_or_err(repo)?;
    let json = gh(&["issue", "list", "-R", &slug, "--state", state, "--limit", "500", "--json", "number,title,state,url,labels"])?;
    parse_issues(&json).ok_or_else(|| GitError("unexpected output from gh issue list".into()))
}

fn parse_issues(json: &str) -> Option<Vec<Issue>> {
    let list: Vec<Value> = serde_json::from_str(json).ok()?;
    Some(
        list.iter()
            .map(|i| {
                let s = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                let labels = i.get("labels").and_then(Value::as_array).map_or_else(Vec::new, |ls| {
                    ls.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect()
                });
                Issue { number: i.get("number").and_then(Value::as_u64).unwrap_or(0), title: s("title"), state: s("state"), url: s("url"), labels }
            })
            .collect(),
    )
}

fn issue_create_args(slug: &str, title: &str, body: &str, labels: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["issue", "create", "-R", slug, "--title", title, "--body", body].map(String::from).to_vec();
    for l in labels {
        args.extend(["--label".to_string(), l.to_string()]);
    }
    args
}

/// Opens an issue and returns its number and URL. PUBLISHES. A label the repository lacks makes `gh` fail, so the
/// issue is then created without labels.
pub fn create_issue(repo: &Path, title: &str, body: &str, labels: &[&str]) -> Result<(u64, String), GitError> {
    let slug = slug_or_err(repo)?;
    let run = |labels: &[&str]| {
        let args = issue_create_args(&slug, title, body, labels);
        gh(&args.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let out = match run(labels) {
        Err(e) if !labels.is_empty() && e.0.to_lowercase().contains("label") => run(&[])?,
        r => r?,
    };
    let url = out.lines().rev().find(|l| l.contains("/issues/")).unwrap_or_default().trim().to_string();
    let n = issue_number(&url).ok_or_else(|| GitError(format!("gh did not print the issue URL: {out}")))?;
    Ok((n, url))
}

/// `https://github.com/o/r/issues/12` → 12.
fn issue_number(url: &str) -> Option<u64> {
    url.trim().trim_end_matches('/').rsplit_once("/issues/")?.1.parse().ok()
}

/// Closes an issue as completed. PUBLISHES.
pub fn close_issue(repo: &Path, number: u64) -> Result<(), GitError> {
    let slug = slug_or_err(repo)?;
    gh(&["issue", "close", &number.to_string(), "-R", &slug]).map(|_| ())
}

/// The signed-in GitHub user, the default owner for a new repository.
pub fn current_user() -> Option<String> {
    gh(&["api", "user", "-q", ".login"]).ok().filter(|u| !u.is_empty())
}

fn repo_create_args(owner: &str, name: &str, private: bool, source: &str, push: bool) -> Vec<String> {
    let full = if owner.trim().is_empty() { name.trim().to_string() } else { format!("{}/{}", owner.trim(), name.trim()) };
    let visibility = if private { "--private" } else { "--public" };
    let mut args: Vec<String> = ["repo", "create", &full, visibility, "--source", source, "--remote", "origin"].map(String::from).to_vec();
    if push {
        args.push("--push".into());
    }
    args
}

/// The `gh repo create` command line, as the confirmation shows it.
pub fn repo_create_command(repo: &Path, owner: &str, name: &str, private: bool, push: bool) -> String {
    let args = repo_create_args(owner, name, private, &repo.display().to_string(), push);
    let quoted: Vec<String> = args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect();
    format!("gh {}", quoted.join(" "))
}

/// Creates `owner/name` on GitHub from the checkout, adds it as `origin` and pushes when `push`. Returns the
/// repository URL. PUBLISHES: always confirmed by the person first.
pub fn create_repo(repo: &Path, owner: &str, name: &str, private: bool, push: bool) -> Result<String, GitError> {
    if name.trim().is_empty() || name.trim().starts_with('-') || owner.trim().starts_with('-') {
        return Err(GitError("give the repository a name".into()));
    }
    if crate::info::git(repo, &["remote", "get-url", "origin"]).is_some() {
        return Err(GitError(format!("{} already has an origin remote", repo.display())));
    }
    let args = repo_create_args(owner, name, private, &repo.display().to_string(), push);
    let out = exec("gh", Some(repo), &args.iter().map(String::as_str).collect::<Vec<_>>(), None, None)?;
    Ok(out.lines().find(|l| l.contains("github.com/")).map_or_else(|| format!("https://github.com/{}", args[2]), |l| l.trim().to_string()))
}

// --- issues ↔ TODO items --------------------------------------------------------------------------------------

/// The issue an item links to: a `(#12)` at the end of its first line.
pub fn issue_ref(text: &str) -> Option<u64> {
    let first = text.lines().next()?.trim_end();
    let inner = first.strip_suffix(')')?.rsplit_once("(#")?.1;
    inner.parse().ok()
}

/// The item text with `(#n)` at the end of its first line (replacing an existing link).
pub fn with_issue(text: &str, n: u64) -> String {
    let (first, rest) = text.split_once('\n').map_or((text, None), |(a, b)| (a, Some(b)));
    let base = match issue_ref(first) {
        Some(old) => first.trim_end().strip_suffix(&format!("(#{old})")).unwrap_or(first).trim_end(),
        None => first.trim_end(),
    };
    match rest {
        Some(rest) => format!("{base} (#{n})\n{rest}"),
        None => format!("{base} (#{n})"),
    }
}

/// An item as an issue: title = its first line without the link, body = the rest.
pub fn issue_parts(text: &str) -> (String, String) {
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let title = match issue_ref(first) {
        Some(n) => first.trim_end().strip_suffix(&format!("(#{n})")).unwrap_or(first).trim_end().to_string(),
        None => first.trim().to_string(),
    };
    (title, rest.trim().to_string())
}

/// Open issues that no item of `doc` links to yet.
pub fn unlinked<'a>(doc: &TodoDoc, issues: &'a [Issue]) -> Vec<&'a Issue> {
    let linked: Vec<u64> = doc.items.iter().filter_map(|i| issue_ref(&i.text)).collect();
    issues.iter().filter(|i| i.open() && !linked.contains(&i.number)).collect()
}

/// An item line for an imported issue.
pub fn import_text(issue: &Issue) -> String {
    format!("{} (#{})", issue.title.lines().next().unwrap_or_default().trim(), issue.number)
}

/// Create an issue from a TODO item (primary checkout) and write `(#n)` on its line. PUBLISHES.
pub fn issue_from_item(project: &Project, item_id: &str) -> ops::Result<(u64, String)> {
    let file = ops::load_todo(project)?;
    let item = file.doc.item(item_id).ok_or_else(|| ops::Error(format!("no item with id {item_id}")))?;
    if let Some(n) = issue_ref(&item.text) {
        return Err(ops::Error(format!("the item is already linked to issue #{n}")));
    }
    let root = ops::primary_root(project)?;
    let (title, body) = issue_parts(&item.text);
    let (n, url) = create_issue(&root, &title, &body, &[])?;
    link(project, item_id, n)?;
    Ok((n, url))
}

/// Write `(#n)` on an item's line. Returns the item's new id (it follows the text).
fn link(project: &Project, item_id: &str, n: u64) -> ops::Result<String> {
    ops::edit_todo(project, |doc| {
        let text = doc.item(item_id).map(|i| i.text.clone()).ok_or_else(|| ops::Error(format!("no item with id {item_id}")))?;
        let linked = with_issue(&text, n);
        doc.set_text(item_id, &linked)?;
        Ok(doc.items.iter().find(|i| i.text == linked).map_or_else(String::new, |i| i.id.clone()))
    })
    .map(|(id, _)| id)
    .map_err(|e| ops::Error(format!("issue #{n} was created, but its number could not be written on the TODO line ({e}); add (#{n}) by hand")))
}

/// Add the open issues that no item links to yet to `section` (primary checkout). Returns how many were added.
pub fn import_issues(project: &Project, section: &str, issues: &[Issue]) -> ops::Result<usize> {
    let (n, _) = ops::edit_todo(project, |doc| {
        let texts: Vec<String> = unlinked(doc, issues).into_iter().map(import_text).collect();
        for t in &texts {
            doc.add(section, t)?;
        }
        Ok(texts.len())
    })?;
    Ok(n)
}

// --- captures ---------------------------------------------------------------------------------------------------

pub fn capture_key(project_id: i64) -> String {
    format!("{CAPTURE_ISSUES}.{project_id}")
}

/// The project's "captures become issues" setting is on and its primary checkout has a GitHub remote.
pub fn captures_become_issues(store: &Store, project: &Project) -> bool {
    let on = store.setting(&capture_key(project.id)).ok().flatten().as_deref() == Some("true");
    on && ops::primary_root(project).ok().and_then(|r| slug(&r)).is_some()
}

/// Labels a capture's issue gets (GitHub's default labels).
pub fn labels(kind: CaptureKind) -> &'static [&'static str] {
    match kind {
        CaptureKind::Bug => &["bug"],
        CaptureKind::Idea => &["enhancement"],
        CaptureKind::Todo | CaptureKind::Note => &[],
    }
}

/// The issue for a capture already in the Inbox (item `item_id`), its number written on the line. PUBLISHES.
/// Returns the issue number and the item's new id.
pub fn capture_issue(project: &Project, kind: CaptureKind, title: &str, details: &str, item_id: &str) -> ops::Result<(u64, String)> {
    let root = ops::primary_root(project)?;
    let (n, _) = create_issue(&root, title.trim(), details.trim(), labels(kind))?;
    Ok((n, link(project, item_id, n)?))
}

/// [`ops::capture`], then the issue when the project's setting asks for one and `may_publish`. The second value is
/// `None` when no issue was due, else the issue number or why there is none (offline, publishing off): the capture
/// stays in the Inbox either way.
pub fn capture(
    store: &Store,
    project: &Project,
    kind: CaptureKind,
    title: &str,
    details: &str,
    may_publish: bool,
) -> ops::Result<(Captured, Option<Result<u64, String>>)> {
    let mut captured = ops::capture(store, project, kind, title, details)?;
    let issue = match &mut captured {
        Captured::Todo { id, .. } if captures_become_issues(store, project) => Some(if may_publish {
            capture_issue(project, kind, title, details, id).map(|(n, new_id)| {
                *id = new_id;
                n
            })
            .map_err(|e| e.0)
        } else {
            Err(format!("publishing is off ({}), so no issue was created", ops::ALLOW_PUBLISH))
        }),
        _ => None,
    };
    Ok((captured, issue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_and_ci_json() {
        let repo = r#"{"defaultBranchRef":{"name":"trunk"},"issues":{"totalCount":12},"latestRelease":{"name":"CLI 2.1","tagName":"v2.1.0","url":"https://github.com/o/r/releases/tag/v2.1.0","publishedAt":"2026-09-30T02:40:02Z"},"nameWithOwner":"o/r","pullRequests":{"totalCount":3},"stargazerCount":46,"url":"https://github.com/o/r","visibility":"PRIVATE"}"#;
        let info = parse_repo(repo).unwrap();
        assert_eq!((info.slug.as_str(), info.visibility.as_str(), info.stars, info.open_issues, info.open_prs), ("o/r", "PRIVATE", 46, 12, 3));
        assert_eq!(info.default_branch.as_deref(), Some("trunk"));
        assert_eq!(info.latest_release.unwrap().tag, "v2.1.0");
        let bare = parse_repo(r#"{"nameWithOwner":"o/r","latestRelease":null,"defaultBranchRef":{"name":""}}"#).unwrap();
        assert_eq!((bare.latest_release, bare.default_branch, bare.stars), (None, None, 0));

        let runs = r#"[{"conclusion":"success","event":"schedule","status":"completed","url":"u1","workflowName":"Triage"},{"conclusion":"","event":"push","status":"in_progress","url":"u2","workflowName":"CI"}]"#;
        let ci = parse_ci(runs).unwrap();
        assert_eq!((ci.workflow.as_str(), ci.state()), ("CI", "in_progress"), "push runs first");
        assert_eq!(parse_ci(r#"[{"conclusion":"failure","event":"schedule","status":"completed","url":"u","workflowName":"T"}]"#).unwrap().state(), "failure");
        assert_eq!(parse_ci("[]"), None);
    }

    #[test]
    fn issue_json_and_urls() {
        let list = r#"[{"labels":[{"id":"x","name":"bug"}],"number":14,"state":"OPEN","title":"Crash","url":"https://github.com/o/r/issues/14"},{"labels":[],"number":9,"state":"CLOSED","title":"Old","url":"u"}]"#;
        let issues = parse_issues(list).unwrap();
        assert_eq!((issues[0].number, issues[0].labels.as_slice(), issues[0].open(), issues[1].open()), (14, &["bug".to_string()][..], true, false));
        assert_eq!(issue_number("https://github.com/o/r/issues/12\n"), Some(12));
        assert_eq!(issue_number("https://github.com/o/r/pull/12"), None);
    }

    #[test]
    fn command_lines() {
        assert_eq!(
            issue_create_args("o/r", "T", "B", &["bug"]),
            ["issue", "create", "-R", "o/r", "--title", "T", "--body", "B", "--label", "bug"]
        );
        assert_eq!(repo_create_args("me", "proj", true, "C:\\x", true), ["repo", "create", "me/proj", "--private", "--source", "C:\\x", "--remote", "origin", "--push"]);
        assert_eq!(repo_create_args("", "proj", false, "d", false)[2..4], ["proj".to_string(), "--public".to_string()]);
        assert_eq!(
            repo_create_command(Path::new("C:\\My Repo"), "me", "p", true, false),
            "gh repo create me/p --private --source \"C:\\My Repo\" --remote origin"
        );
    }

    #[test]
    fn issue_links_on_item_lines() {
        assert_eq!(issue_ref("Fix it (#12)"), Some(12));
        assert_eq!(issue_ref("Fix it (#12)\nmore"), Some(12));
        assert_eq!(issue_ref("Fix (#12) later"), None);
        assert_eq!(issue_ref("Fix (#x)"), None);
        assert_eq!(with_issue("Fix it", 3), "Fix it (#3)");
        assert_eq!(with_issue("Fix it (#2)\ndetails", 3), "Fix it (#3)\ndetails");
        assert_eq!(issue_parts("Idea: X (#4)\nline one\nline two"), ("Idea: X".to_string(), "line one\nline two".to_string()));
        let doc = crate::todo::parse("## A\n\n- [ ] one (#1)\n- [ ] two\n");
        let issues = [
            Issue { number: 1, title: "one".into(), state: "OPEN".into(), ..Default::default() },
            Issue { number: 2, title: "two\nbody".into(), state: "OPEN".into(), ..Default::default() },
            Issue { number: 3, title: "closed".into(), state: "CLOSED".into(), ..Default::default() },
        ];
        let new: Vec<String> = unlinked(&doc, &issues).into_iter().map(import_text).collect();
        assert_eq!(new, ["two (#2)"]);
    }

    #[test]
    fn captures_stay_in_the_inbox_without_a_github_remote() {
        let dir = std::env::temp_dir().join(format!("pa-github-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", "https://gitlab.com/o/r.git"]] {
            assert!(crate::cmd("git").args(&args).current_dir(&dir).status().unwrap().success());
        }
        let store = Store::open_in_memory().unwrap();
        let id = store.add_project("p", None, None, &[dir.display().to_string()]).unwrap();
        store.set_setting(&capture_key(id), "true").unwrap();
        let project = ops::find_project(&store, "p").unwrap();
        assert_eq!(slug(&dir), None);
        assert!(!captures_become_issues(&store, &project), "not a GitHub remote");
        let (captured, issue) = capture(&store, &project, CaptureKind::Bug, "Crash", "", true).unwrap();
        assert!(matches!(captured, Captured::Todo { .. }) && issue.is_none());
        assert!(std::fs::read_to_string(dir.join("TODO.md")).unwrap().contains("## Inbox\n\n- [ ] Bug: Crash\n"));

        // A GitHub remote with publishing off: no gh call, the reason is reported.
        crate::cmd("git").args(["remote", "set-url", "origin", "git@github.com:o/r.git"]).current_dir(&dir).status().unwrap();
        assert_eq!(slug(&dir).as_deref(), Some("o/r"));
        let (_, issue) = capture(&store, &project, CaptureKind::Idea, "Later", "", false).unwrap();
        assert!(issue.unwrap().unwrap_err().contains("publishing is off"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
