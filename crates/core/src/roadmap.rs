//! ROADMAP.md (SPEC § 7.4, D-13, ROADMAP v2 in `docs/briefs/next.md` § 2). Each `##` heading is one of three things:
//! - a **version** when it starts with one (`v0.5.0`, `1.2`, `v0.4.5-beta`): a release with a status;
//! - a **milestone** when it starts with `Milestone:`, or has a `Status:` / `Target:` line or checklist items;
//! - a plain **section** otherwise: kept verbatim, not an entry.
//!
//! Built on [`TodoDoc`]: item edits go through `doc` with the entry's heading as the section name.

use crate::todo::{self, Style, TodoDoc, TodoError, TodoItem};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// A release (version) or a theme that can span versions (milestone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Milestone,
    Version,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MilestoneStatus {
    Planned,
    InProgress,
    /// A shipped version: `done`, `released` or `shipped` on a version.
    Released,
    /// A finished milestone: `done`, or `released` on a milestone.
    Done,
    /// Any other `Status:` value, kept as written.
    Other(String),
}

impl MilestoneStatus {
    /// The status of an entry of `kind` from its first word(s): `done (shipped with v0.5.1)` is done. Anything
    /// that starts otherwise (`v0.8.0 done; …`) is `Other`, kept as written.
    fn parse(s: &str, kind: EntryKind) -> Self {
        let lower = s.trim().to_ascii_lowercase();
        let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(2).collect();
        let finished = if kind == EntryKind::Version { MilestoneStatus::Released } else { MilestoneStatus::Done };
        match words.as_slice() {
            ["in", "progress", ..] | ["wip" | "active" | "started" | "ongoing", ..] => MilestoneStatus::InProgress,
            ["planned" | "next" | "todo" | "later", ..] => MilestoneStatus::Planned,
            ["done" | "released" | "shipped" | "complete" | "completed" | "finished", ..] => finished,
            _ => MilestoneStatus::Other(s.trim().to_string()),
        }
    }

    /// The text written after `Status:`.
    pub fn label(&self) -> &str {
        match self {
            MilestoneStatus::Planned => "planned",
            MilestoneStatus::InProgress => "in progress",
            MilestoneStatus::Released => "released",
            MilestoneStatus::Done => "done",
            MilestoneStatus::Other(s) => s,
        }
    }

    /// Planned 0, in progress 1, released or done 2; `Other` has none.
    fn rank(&self) -> Option<u8> {
        match self {
            MilestoneStatus::Planned => Some(0),
            MilestoneStatus::InProgress => Some(1),
            MilestoneStatus::Released | MilestoneStatus::Done => Some(2),
            MilestoneStatus::Other(_) => None,
        }
    }
}

/// Whether a `Status:` text says more than its status word(s): a note to show on hover.
fn status_says_more(text: &str, status: &MilestoneStatus) -> bool {
    let words = if *status == MilestoneStatus::InProgress { 2 } else { 1 };
    matches!(status, MilestoneStatus::Other(_)) || text.split_whitespace().count() > words
}

/// The status as an entry of `kind` writes it: a version's `done` is `released`, a milestone's `released` is `done`.
fn as_kind(kind: EntryKind, s: MilestoneStatus) -> MilestoneStatus {
    match (kind, s) {
        (EntryKind::Version, MilestoneStatus::Done) => MilestoneStatus::Released,
        (EntryKind::Milestone, MilestoneStatus::Released) => MilestoneStatus::Done,
        (_, s) => s,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Milestone {
    pub kind: EntryKind,
    /// A version: the text after it (may be empty). A milestone: the heading, or the text after `Milestone:`.
    pub title: String,
    /// The whole `##` heading text: the section name for [`TodoDoc`] calls.
    pub heading: String,
    /// 0-based line of the heading.
    pub line: usize,
    /// A version: as written (`v0.5.0`). A milestone: `None`.
    pub version: Option<String>,
    /// A milestone: the versions named by its `Versions:` line. A version: the milestones named by `Milestone:`.
    pub links: Vec<String>,
    /// The status as the derivation sees it: the `Status:` value, or `Planned` when there is none.
    pub status: MilestoneStatus,
    /// Whether the entry has its own `Status:` line.
    pub explicit_status: bool,
    /// The `Status:` text as written when it says more than the status word (`done (v0.6.0 and v0.6.1)`).
    pub status_note: Option<String>,
    /// What the app uses: the explicit status, else the status derived from the linked versions. Same as `status`.
    pub effective: MilestoneStatus,
    /// Set when an explicit `Status:` disagrees with the linked versions: `Status says done but v0.5.1 is planned`.
    pub warning: Option<String>,
    pub target: Option<String>,
    /// Free text: lines that are neither the header nor items.
    pub body: String,
    /// The entry's own items.
    pub items: Vec<TodoItem>,
    /// Done items of the entry, and for a milestone also of its linked versions.
    pub done: usize,
    /// Items of the entry, and for a milestone also of its linked versions.
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Roadmap {
    pub doc: TodoDoc,
    /// Versions and milestones in file order. Sections are not entries.
    pub milestones: Vec<Milestone>,
    /// The document is editable and has no markdown tables.
    pub editable: bool,
}

/// The board lanes (ROADMAP v2 § 3): indices into [`Roadmap::milestones`], in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub now: Vec<usize>,
    pub next: Vec<usize>,
    pub later: Vec<usize>,
    pub done: Vec<usize>,
}

/// Roadmap items are always checkboxes, whatever the text looks like.
pub fn parse_roadmap(text: &str) -> Roadmap {
    from_doc(todo::parse_with(text, &Style::CHECKLIST))
}

fn from_doc(doc: TodoDoc) -> Roadmap {
    let n = doc.lines.len();
    let mut milestones = Vec::new();
    for (k, sec) in doc.sections.iter().enumerate() {
        let start = sec.line + 1;
        let end = doc.sections.get(k + 1).map_or(n, |s| s.line);
        let head = header(&doc.lines, start, end);
        let items: Vec<TodoItem> = doc.items.iter().filter(|i| i.line >= start && i.line < end).cloned().collect();
        let Some((kind, title, version)) = classify(&sec.title, &head, !items.is_empty()) else { continue };
        let body: Vec<&str> =
            (head.body_start..end).filter(|&j| !items.iter().any(|i| j >= i.line && j < i.end)).map(|j| doc.lines[j].as_str()).collect();
        let status = head.status.as_deref().map_or(MilestoneStatus::Planned, |s| MilestoneStatus::parse(s, kind));
        milestones.push(Milestone {
            kind,
            title,
            heading: sec.title.clone(),
            line: sec.line,
            version,
            links: match kind {
                EntryKind::Milestone => head.versions,
                EntryKind::Version => head.milestones,
            },
            explicit_status: head.status.is_some(),
            status_note: head.status.as_deref().map(str::trim).filter(|s| status_says_more(s, &status)).map(str::to_string),
            status: status.clone(),
            effective: status,
            warning: None,
            target: head.target,
            body: body.join("\n").trim().to_string(),
            done: items.iter().filter(|i| i.done).count(),
            total: items.len(),
            items,
        });
    }
    resolve(&mut milestones);
    let table = doc.lines.iter().any(|l| is_row(l));
    let editable = doc.editable && !table;
    Roadmap { doc, milestones, editable }
}

/// What a heading is: its kind, its title and its version. `None` for a plain section.
fn classify(heading: &str, head: &Header, has_items: bool) -> Option<(EntryKind, String, Option<String>)> {
    if let Some(title) = field(heading.trim(), "milestone:") {
        return Some((EntryKind::Milestone, title.trim().to_string(), None));
    }
    if let Some((version, rest)) = split_version(heading) {
        return Some((EntryKind::Version, title_after(rest), Some(version.to_string())));
    }
    (head.status.is_some() || head.target.is_some() || has_items).then(|| (EntryKind::Milestone, heading.trim().to_string(), None))
}

/// The title after a version: an optional separator (` — `, ` - `, `:`) is dropped.
fn title_after(rest: &str) -> String {
    let t = rest.trim_start();
    t.strip_prefix(['—', '–', '-', ':']).unwrap_or(t).trim().to_string()
}

/// A heading that starts with a version: optional letters (`v`), `N.N` or `N.N.N`, an optional `-pre-release`, then
/// the end or a separator. Returns the version as written and the text after it.
fn split_version(heading: &str) -> Option<(&str, &str)> {
    let t = heading.trim_start();
    let b = t.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_alphabetic()).count();
    let digits = |at: usize| -> Option<usize> {
        let d = b[at..].iter().take_while(|c| c.is_ascii_digit()).count();
        (d > 0).then_some(at + d)
    };
    let (mut end, mut parts, mut at) = (letters, 0, letters);
    while let Some(next) = digits(at) {
        parts += 1;
        end = next;
        if parts == 3 || b.get(next) != Some(&b'.') || digits(next + 1).is_none() {
            break;
        }
        at = next + 1;
    }
    if parts < 2 {
        return None;
    }
    if b.get(end) == Some(&b'-') {
        let pre = b[end + 1..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'.' || **c == b'-').count();
        if pre > 0 {
            end += 1 + pre;
        }
    }
    let rest = &t[end..];
    let ok = rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with(['—', '–', '-', ':', '(']);
    ok.then(|| (&t[..end], rest))
}

/// A version without its own leading letters: `v0.5.0` is `0.5.0`.
fn bare(version: &str) -> &str {
    version.trim_start_matches(|c: char| c.is_ascii_alphabetic())
}

/// Orders version strings by their numbers (`0.9` before `0.10`); a pre-release sorts before its release; an
/// unparsable string sorts last. ponytail: pre-releases compare as plain text (`beta.2` after `beta.10`).
pub fn semver_cmp(a: &str, b: &str) -> Ordering {
    match (semver_key(a), semver_key(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

/// The numbers padded to three, then whether it is a release, then the pre-release text.
fn semver_key(s: &str) -> Option<([u64; 3], bool, Option<String>)> {
    let (version, _) = split_version(s)?;
    let (nums, pre) = match bare(version).split_once('-') {
        Some((n, p)) => (n, Some(p.to_string())),
        None => (bare(version), None),
    };
    let mut parts = [0u64; 3];
    for (slot, p) in parts.iter_mut().zip(nums.split('.')) {
        *slot = p.parse().ok()?;
    }
    Some((parts, pre.is_none(), pre))
}

/// Derives each milestone's status from its linked versions, checks an explicit `Status:` against it, and adds the
/// linked versions' items to the milestone's progress.
fn resolve(ms: &mut [Milestone]) {
    let settled: Vec<(MilestoneStatus, Option<String>, usize, usize)> = (0..ms.len()).map(|i| settle(ms, i)).collect();
    for (m, (status, warning, done, total)) in ms.iter_mut().zip(settled) {
        m.effective = status.clone();
        m.status = status;
        m.warning = warning;
        m.done = done;
        m.total = total;
    }
}

fn settle(ms: &[Milestone], i: usize) -> (MilestoneStatus, Option<String>, usize, usize) {
    let m = &ms[i];
    if m.kind == EntryKind::Version {
        return (m.status.clone(), None, m.done, m.total);
    }
    let linked: Vec<&Milestone> = (0..ms.len())
        .filter(|&j| {
            ms[j].kind == EntryKind::Version
                && (ms[j].links.iter().any(|l| l.eq_ignore_ascii_case(&m.title)) || m.links.iter().any(|l| names(&ms[j], l)))
        })
        .map(|j| &ms[j])
        .collect();
    let done = m.done + linked.iter().map(|v| v.done).sum::<usize>();
    let total = m.total + linked.iter().map(|v| v.total).sum::<usize>();
    if linked.is_empty() {
        return (m.status.clone(), None, done, total);
    }
    // Derived (decision 4): all released -> done; any in progress or released -> in progress; else planned.
    let ranks: Vec<u8> = linked.iter().map(|v| v.status.rank().unwrap_or(0)).collect();
    let derived = if ranks.iter().all(|&r| r == 2) {
        MilestoneStatus::Done
    } else if ranks.iter().any(|&r| r >= 1) {
        MilestoneStatus::InProgress
    } else {
        MilestoneStatus::Planned
    };
    match (m.explicit_status, m.status.rank(), derived.rank()) {
        (false, _, _) => (derived, None, done, total),
        (true, Some(e), Some(d)) if e != d => {
            // The first version that is behind an explicit status that is ahead of the versions, or ahead of one behind.
            let warning = linked
                .iter()
                .find(|v| {
                    let r = v.status.rank().unwrap_or(0);
                    if e > d { r < e } else { r > e }
                })
                .map(|v| {
                    let name = v.version.as_deref().unwrap_or(&v.title);
                    format!("Status says {} but {name} is {}", m.status.label(), v.status.label())
                });
            (m.status.clone(), warning, done, total)
        }
        _ => (m.status.clone(), None, done, total),
    }
}

/// Whether `name` is the entry's version (exact) or its title (case-insensitive).
fn names(e: &Milestone, name: &str) -> bool {
    e.version.as_deref() == Some(name) || e.title.eq_ignore_ascii_case(name)
}

/// The lines under the heading at `line`, as `(first line after it, exclusive end)`: the end is the next `##`.
fn bounds(doc: &TodoDoc, line: usize) -> (usize, usize) {
    let end = doc.sections.iter().map(|s| s.line).find(|&l| l > line).unwrap_or(doc.lines.len());
    (line + 1, end)
}

/// Puts `block` at `at` with a blank line after it; at the end of the file (`None`, or `at` past it) with a blank
/// line before it when the file does not end in one.
fn insert_block(lines: &mut Vec<String>, at: Option<usize>, mut block: Vec<String>) {
    match at {
        Some(at) if at < lines.len() => {
            block.push(String::new());
            lines.splice(at..at, block);
        }
        _ => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
}

/// The header block under a heading: the field lines before the first other text. Blank lines are allowed between.
#[derive(Default)]
struct Header {
    status: Option<String>,
    status_line: Option<usize>,
    target: Option<String>,
    /// `Versions:` of a milestone.
    versions: Vec<String>,
    /// `Milestone:` / `Milestones:` of a version.
    milestones: Vec<String>,
    milestones_line: Option<usize>,
    /// The last field line, so a new field can go under it.
    last_field: Option<usize>,
    body_start: usize,
}

fn header(lines: &[String], start: usize, end: usize) -> Header {
    let mut h = Header { body_start: start, ..Default::default() };
    for (j, line) in lines.iter().enumerate().take(end).skip(start) {
        let t = line.trim();
        let is_field = if let Some(v) = field(t, "status:") {
            h.status = Some(v.trim().to_string());
            h.status_line = Some(j);
            true
        } else if let Some(v) = field(t, "target:") {
            h.target = Some(v.trim().to_string()).filter(|v| !v.is_empty());
            true
        } else if let Some(v) = field(t, "versions:") {
            h.versions = split_list(v);
            true
        } else if let Some(v) = field(t, "milestones:").or_else(|| field(t, "milestone:")) {
            h.milestones = split_list(v);
            h.milestones_line = Some(j);
            true
        } else {
            false
        };
        if is_field {
            h.last_field = Some(j);
        } else if !t.is_empty() {
            break;
        }
        h.body_start = j + 1;
    }
    h
}

/// The value of a `key:` line (case-insensitive key).
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.get(..key.len()).filter(|p| p.eq_ignore_ascii_case(key)).map(|_| &line[key.len()..])
}

/// A comma-separated list, trimmed, without empty names.
fn split_list(v: &str) -> Vec<String> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

impl Roadmap {
    pub fn render(&self) -> String {
        self.doc.render()
    }

    /// The entry called `name`: a version (exact), else a title or heading (case-insensitive).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.milestones
            .iter()
            .position(|m| m.version.as_deref() == Some(name))
            .or_else(|| self.milestones.iter().position(|m| m.title.eq_ignore_ascii_case(name)))
            .or_else(|| self.milestones.iter().position(|m| m.heading.eq_ignore_ascii_case(name)))
    }

    fn check_editable(&self) -> Result<(), TodoError> {
        if self.editable { Ok(()) } else { Err(TodoError::ReadOnly) }
    }

    /// Set the entry's `Status:` line (replaced, or inserted under the heading). A version's `done` is written as
    /// `released`. `name` is a version or a title.
    pub fn set_status(&mut self, name: &str, status: MilestoneStatus) -> Result<(), TodoError> {
        self.check_editable()?;
        let i = self.index_of(name).ok_or_else(|| TodoError::NotFound(name.to_string()))?;
        let (line, kind) = (self.milestones[i].line, self.milestones[i].kind);
        let (start, end) = bounds(&self.doc, line);
        let text = format!("Status: {}", as_kind(kind, status).label());
        match header(&self.doc.lines, start, end).status_line {
            Some(j) => self.doc.lines[j] = text,
            None => self.doc.lines.insert(start, text),
        }
        self.reparse();
        Ok(())
    }

    /// Append a milestone: heading and `Status:` line. The heading is `## Milestone: X` when the file already uses
    /// that prefix, else `## X`.
    pub fn add_milestone(&mut self, title: &str, status: MilestoneStatus) -> Result<(), TodoError> {
        self.check_editable()?;
        if self.index_of(title).is_some() {
            return Err(TodoError::Exists(title.to_string()));
        }
        let prefixed = self.doc.sections.iter().any(|s| field(s.title.trim(), "milestone:").is_some());
        let heading = if prefixed { format!("## Milestone: {title}") } else { format!("## {title}") };
        let block = vec![heading, format!("Status: {}", as_kind(EntryKind::Milestone, status).label())];
        insert_block(&mut self.doc.lines, None, block);
        self.reparse();
        Ok(())
    }

    /// Insert a version (`## v0.5.0 — title`, `Status:`, optional `Milestone:`). It goes before the first version that
    /// sorts after it by semver, else after the last version, else at the end of the file.
    pub fn add_version(&mut self, version: &str, title: &str, status: MilestoneStatus, milestone: Option<&str>) -> Result<(), TodoError> {
        self.check_editable()?;
        let version = version.trim();
        if !split_version(version).is_some_and(|(v, rest)| v == version && rest.is_empty()) {
            return Err(TodoError::Unsupported(format!("{version:?} is not a version such as v0.5.0")));
        }
        if self.index_of(version).is_some() {
            return Err(TodoError::Exists(version.to_string()));
        }
        let title = title.trim();
        let heading = if title.is_empty() { format!("## {version}") } else { format!("## {version} — {title}") };
        let mut block = vec![heading, format!("Status: {}", as_kind(EntryKind::Version, status).label())];
        if let Some(m) = milestone.map(str::trim).filter(|m| !m.is_empty()) {
            block.push(format!("Milestone: {m}"));
        }
        let versions = || self.milestones.iter().filter(|m| m.kind == EntryKind::Version);
        let at = versions()
            .find(|m| semver_cmp(m.version.as_deref().unwrap_or(""), version) == Ordering::Greater)
            .map(|m| m.line)
            .or_else(|| versions().next_back().map(|m| bounds(&self.doc, m.line).1));
        insert_block(&mut self.doc.lines, at, block);
        self.reparse();
        Ok(())
    }

    /// Name `milestone` in the version's `Milestone:` line (created under the header when missing).
    pub fn link(&mut self, version: &str, milestone: &str) -> Result<(), TodoError> {
        self.check_editable()?;
        let milestone = milestone.trim();
        let i = self
            .index_of(version)
            .filter(|&i| self.milestones[i].kind == EntryKind::Version)
            .ok_or_else(|| TodoError::NotFound(version.to_string()))?;
        if milestone.is_empty() {
            return Err(TodoError::Unsupported("a milestone name is needed".into()));
        }
        let (start, end) = bounds(&self.doc, self.milestones[i].line);
        let head = header(&self.doc.lines, start, end);
        if head.milestones.iter().any(|m| m.eq_ignore_ascii_case(milestone)) {
            return Ok(());
        }
        let mut names = head.milestones.clone();
        names.push(milestone.to_string());
        match head.milestones_line {
            Some(j) => {
                let raw = &self.doc.lines[j];
                let key = raw.split_once(':').map_or("Milestone", |(k, _)| k.trim()).to_string();
                self.doc.lines[j] = format!("{key}: {}", names.join(", "));
            }
            None => {
                let at = head.last_field.map_or(start, |j| j + 1);
                self.doc.lines.insert(at, format!("Milestone: {}", names.join(", ")));
            }
        }
        self.reparse();
        Ok(())
    }

    /// Move the entry's whole section (heading, header, text, items) before `before`'s section; `None` = to the end.
    pub fn move_entry(&mut self, name: &str, before: Option<&str>) -> Result<(), TodoError> {
        self.check_editable()?;
        let i = self.index_of(name).ok_or_else(|| TodoError::NotFound(name.to_string()))?;
        let target = match before {
            Some(b) => Some(self.index_of(b).ok_or_else(|| TodoError::NotFound(b.to_string()))?),
            None => None,
        };
        if target == Some(i) {
            return Ok(());
        }
        let start = self.milestones[i].line;
        let (_, end) = bounds(&self.doc, start);
        let before_line = target.map(|t| self.milestones[t].line);
        let mut block: Vec<String> = self.doc.lines.drain(start..end).collect();
        while block.last().is_some_and(|l| l.trim().is_empty()) {
            block.pop();
        }
        // The lines after the block moved up by its length, so a target below it moves up too.
        let at = before_line.map(|b| if b >= end { b - (end - start) } else { b });
        insert_block(&mut self.doc.lines, at, block);
        self.reparse();
        Ok(())
    }

    /// Re-read the entries after `doc` was edited directly.
    pub fn reparse(&mut self) {
        let text = self.doc.render();
        *self = from_doc(todo::parse_with(&text, &Style::CHECKLIST));
    }
}

/// The lanes of the Now / Next / Later board. Now: in progress. Next: planned entries up to and including the first
/// planned version (the one in progress is Now, not the boundary); without one, the first three planned entries.
/// Later: the other planned entries and the unknown statuses. Done: done and released.
pub fn board(r: &Roadmap) -> Board {
    let ms = &r.milestones;
    let boundary = ms.iter().position(|m| m.kind == EntryKind::Version && m.effective == MilestoneStatus::Planned);
    let mut b = Board::default();
    let mut planned = 0;
    for (i, m) in ms.iter().enumerate() {
        match m.effective {
            MilestoneStatus::Done | MilestoneStatus::Released => b.done.push(i),
            MilestoneStatus::InProgress => b.now.push(i),
            MilestoneStatus::Planned => {
                let next = match boundary {
                    Some(k) => i <= k,
                    None => planned < 3,
                };
                planned += 1;
                if next { b.next.push(i) } else { b.later.push(i) }
            }
            MilestoneStatus::Other(_) => b.later.push(i),
        }
    }
    b
}

/// Versions not released yet whose tag exists: `prefix` + the bare version (`v0.5.0` → `v0.5.0`, `spec-v0.5.0`), or
/// the version as written. The app suggests marking them released; it never does it by itself.
pub fn release_suggestions(r: &Roadmap, tags: &[String], prefix: &str) -> Vec<String> {
    r.milestones
        .iter()
        .filter(|m| m.kind == EntryKind::Version && m.effective != MilestoneStatus::Released)
        .filter_map(|m| m.version.as_deref())
        .filter(|v| {
            let tag = format!("{prefix}{}", bare(v));
            tags.iter().any(|t| *t == tag || t.as_str() == *v)
        })
        .map(str::to_string)
        .collect()
}

/// The first existing roadmap file of a project root.
pub fn find_roadmap(root: &Path) -> Option<PathBuf> {
    ["ROADMAP.md", "roadmap.md", "docs/ROADMAP.md", "docs/roadmap.md"].iter().map(|n| root.join(n)).find(|p| p.is_file())
}

/// Table-style roadmap (`| Version | Name | Contents |`) to the milestone format: each row becomes
/// `## <col1> — <col2>` with `Status:` from col1 and `<Header>: <cell>` lines for the other columns. Text outside
/// the table is kept (then converted like a TODO file, so bullets become items).
pub fn convert_roadmap(text: &str) -> String {
    let (lines, crlf, trailing) = todo::split(text);
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_row(&lines[i]) && lines.get(i + 1).is_some_and(|l| is_separator(l)) {
            let header = table_cells(&lines[i]);
            i += 2;
            while i < lines.len() && is_row(&lines[i]) {
                let cells = table_cells(&lines[i]);
                let c0 = cells.first().cloned().unwrap_or_default();
                let title = match cells.get(1) {
                    Some(c1) if !c1.is_empty() => format!("{} — {}", strip_title(&c0), strip_title(c1)),
                    _ => strip_title(&c0),
                };
                let low = c0.to_ascii_lowercase();
                let status = if low.contains("done") {
                    "done"
                } else if low.contains("built") || low.contains("in progress") {
                    "in progress"
                } else {
                    "planned"
                };
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push(format!("## {title}"));
                out.push(format!("Status: {status}"));
                for (k, cell) in cells.iter().enumerate().skip(2) {
                    if !cell.is_empty() {
                        out.push(format!("{}: {cell}", header.get(k).map_or("", String::as_str)));
                    }
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i].clone());
        i += 1;
    }
    todo::convert(&todo::join(&out, crlf, trailing))
}

fn is_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn is_separator(line: &str) -> bool {
    let cells = table_cells(line);
    !cells.is_empty() && cells.iter().all(|c| c.contains('-') && c.chars().all(|ch| ch == '-' || ch == ':'))
}

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Title without markdown emphasis, code marks and `(done…)` / `(built…)` annotations.
fn strip_title(s: &str) -> String {
    let mut t: String = s.chars().filter(|&c| c != '*' && c != '`').collect();
    loop {
        // ASCII lowercase keeps byte positions, so the indices below are valid in `t`.
        let lower = t.to_ascii_lowercase();
        match ["(done", "(built"].iter().filter_map(|p| lower.find(p)).min() {
            Some(i) => {
                let end = t[i..].find(')').map_or(t.len(), |e| i + e + 1);
                t.replace_range(i..end, "");
            }
            None => break,
        }
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RM: &str =
        "# Roadmap\n\n## v0.5 — UI\nStatus: In Progress\nTarget: 2026-11\n\nFree text.\n\n- [x] one\n- [ ] two\n\n## v0.6\n\n- [ ] three\n";

    fn kinds(r: &Roadmap) -> Vec<(EntryKind, String)> {
        r.milestones.iter().map(|m| (m.kind, m.title.clone())).collect()
    }

    #[test]
    fn reads_status_target_body_and_progress() {
        let r = parse_roadmap(RM);
        assert!(r.editable);
        let v5 = &r.milestones[0];
        assert_eq!((v5.status.clone(), v5.target.as_deref(), v5.body.as_str()), (MilestoneStatus::InProgress, Some("2026-11"), "Free text."));
        assert_eq!((v5.done, v5.total, v5.version.as_deref()), (1, 2, Some("v0.5")));
        let v6 = &r.milestones[1];
        assert_eq!((v6.status.clone(), v6.target.clone(), v6.body.as_str()), (MilestoneStatus::Planned, None, ""));
        assert_eq!((v6.done, v6.total), (0, 1));
        assert_eq!(parse_roadmap("## X\nStatus: Blocked\n").milestones[0].status, MilestoneStatus::Other("Blocked".into()));
        // The first word(s) decide; the rest is a note.
        let r = parse_roadmap("## v0.5 — A\nStatus: done (shipped with v0.5.1)\n\n## B\nStatus: In progress, half way\n\n## C\nStatus: next (brief to write)\n\n## v0.8 — D\nStatus: v0.8.0 done; v0.8.1 built\n\n## E\nStatus: done\n");
        let got: Vec<(&MilestoneStatus, Option<&str>)> = r.milestones.iter().map(|m| (&m.status, m.status_note.as_deref())).collect();
        assert_eq!(got[0], (&MilestoneStatus::Released, Some("done (shipped with v0.5.1)")));
        assert_eq!(got[1], (&MilestoneStatus::InProgress, Some("In progress, half way")));
        assert_eq!(got[2].0, &MilestoneStatus::Planned);
        assert_eq!(got[3], (&MilestoneStatus::Other("v0.8.0 done; v0.8.1 built".into()), Some("v0.8.0 done; v0.8.1 built")));
        assert_eq!(got[4], (&MilestoneStatus::Done, None));
        assert_eq!(MilestoneStatus::parse("donezo", EntryKind::Milestone).rank(), None);
    }

    #[test]
    fn classifies_versions_milestones_and_sections() {
        let text = "## v0.8 (was v0.10; v0.8.0 done)\n- [ ] a\n\n## Milestone: UI revamp\nVersions: v0.8, v0.9\n\n## M2 — Project tab\n- [ ] b\n\n## Direction\nJust text.\n\n## Milestones\nSome words.\n\n## 1.2\n\n## v0.4.5-beta — Pre\n\n## v0.4.5.1 not a version\n";
        let r = parse_roadmap(text);
        assert_eq!(
            kinds(&r),
            [
                (EntryKind::Version, "(was v0.10; v0.8.0 done)".to_string()),
                (EntryKind::Milestone, "UI revamp".to_string()),
                (EntryKind::Milestone, "M2 — Project tab".to_string()),
                (EntryKind::Version, String::new()),
                (EntryKind::Version, "Pre".to_string()),
            ]
        );
        assert_eq!(r.milestones[0].version.as_deref(), Some("v0.8"));
        assert_eq!(r.milestones[1].links, ["v0.8", "v0.9"]);
        assert_eq!(r.milestones[3].version.as_deref(), Some("1.2"));
        assert_eq!(r.milestones[4].version.as_deref(), Some("v0.4.5-beta"));
        assert!(r.milestones.iter().all(|m| m.title != "Direction" && m.title != "Milestones"));
    }

    #[test]
    fn released_words_depend_on_the_kind() {
        let r = parse_roadmap(
            "## v1.0\nStatus: shipped\n\n## v1.1\nStatus: Done\n\n## Milestone: A\nStatus: released\n\n## Milestone: B\nStatus: done\n",
        );
        let st: Vec<&MilestoneStatus> = r.milestones.iter().map(|m| &m.status).collect();
        assert_eq!(st, [&MilestoneStatus::Released, &MilestoneStatus::Released, &MilestoneStatus::Done, &MilestoneStatus::Done]);
        assert_eq!(MilestoneStatus::Released.label(), "released");
        assert_eq!(MilestoneStatus::Done.label(), "done");
    }

    #[test]
    fn milestone_status_is_derived_from_its_versions_and_warns_when_explicit_disagrees() {
        let text = "## Milestone: UI\nVersions: v0.5.0, v0.5.1\n\n## Milestone: Vehicles\nStatus: done\nVersions: v0.6.0\n\n## Milestone: Plain\n\n- [x] old\n\n## v0.5.0\nStatus: released\nMilestone: UI\n\n## v0.5.1\nStatus: planned\n\n## v0.6.0\nStatus: released\n";
        let r = parse_roadmap(text);
        let ui = &r.milestones[0];
        assert_eq!((ui.status.clone(), ui.explicit_status, ui.warning.clone()), (MilestoneStatus::InProgress, false, None));
        assert_eq!((ui.done, ui.total), (0, 0), "linked versions have no items");
        let vehicles = &r.milestones[1];
        assert_eq!(vehicles.effective, MilestoneStatus::Done);
        assert_eq!(vehicles.warning, None, "explicit done agrees with its only version");
        let plain = &r.milestones[2];
        assert_eq!((plain.status.clone(), plain.total), (MilestoneStatus::Planned, 1), "no versions: planned");

        let warned = parse_roadmap("## Milestone: UI\nStatus: done\nVersions: v0.5.1\n\n## v0.5.1\nStatus: planned\n");
        assert_eq!(warned.milestones[0].effective, MilestoneStatus::Done, "explicit wins");
        assert_eq!(warned.milestones[0].warning.as_deref(), Some("Status says done but v0.5.1 is planned"));
        let behind = parse_roadmap("## Milestone: UI\nStatus: planned\nVersions: v0.5.1\n\n## v0.5.1\nStatus: released\n");
        assert_eq!(behind.milestones[0].warning.as_deref(), Some("Status says planned but v0.5.1 is released"));
    }

    #[test]
    fn linked_version_items_count_in_milestone_progress() {
        let r = parse_roadmap("## Milestone: UI\nStatus: in progress\n\n- [x] own\n\n## v0.5.0\nMilestone: UI\n\n- [x] a\n- [ ] b\n");
        assert_eq!((r.milestones[0].done, r.milestones[0].total), (2, 3));
        assert_eq!((r.milestones[1].done, r.milestones[1].total), (1, 2), "a version counts its own items only");
    }

    #[test]
    fn set_status_replaces_or_inserts_under_the_heading() {
        let mut r = parse_roadmap(RM);
        r.set_status("v0.6", MilestoneStatus::Done).unwrap();
        assert_eq!(r.milestones[1].status, MilestoneStatus::Released, "a version writes released");
        assert_eq!(r.doc.lines[12], "Status: released");
        assert_eq!(r.doc.lines.len(), parse_roadmap(RM).doc.lines.len() + 1);
        r.set_status("UI", MilestoneStatus::Done).unwrap();
        assert_eq!(r.milestones[0].status, MilestoneStatus::Released);
        assert_eq!(r.milestones[0].target.as_deref(), Some("2026-11"));
        assert_eq!(r.doc.lines.len(), parse_roadmap(RM).doc.lines.len() + 1, "replaced, not added");
        r.set_status("v0.5 — UI", MilestoneStatus::Planned).unwrap();
        assert_eq!(r.milestones[0].status, MilestoneStatus::Planned, "the heading also finds the entry");
        assert_eq!(r.set_status("nope", MilestoneStatus::Done), Err(TodoError::NotFound("nope".into())));
    }

    #[test]
    fn add_milestone_and_item_edits_through_doc() {
        let mut r = parse_roadmap(RM);
        r.add_milestone("Shipping", MilestoneStatus::Planned).unwrap();
        assert_eq!(r.milestones.len(), 3);
        assert_eq!(r.doc.lines[r.doc.lines.len() - 2..], ["## Shipping".to_string(), "Status: planned".to_string()]);
        assert_eq!(r.add_milestone("Shipping", MilestoneStatus::Planned), Err(TodoError::Exists("Shipping".into())));
        let id = r.doc.add("Shipping", "Ship it").unwrap();
        r.reparse();
        assert_eq!(r.milestones[2].total, 1);
        r.doc.set_done(&id, true).unwrap();
        r.reparse();
        assert_eq!(r.milestones[2].done, 1);

        let mut prefixed = parse_roadmap("## Milestone: A\nStatus: planned\n");
        prefixed.add_milestone("B", MilestoneStatus::InProgress).unwrap();
        assert!(prefixed.render().ends_with("## Milestone: B\nStatus: in progress\n"), "{}", prefixed.render());
    }

    #[test]
    fn add_version_orders_by_semver() {
        let text = "# Roadmap\n\n## v0.9 — Nine\nStatus: planned\n\n## v0.10 — Ten\nStatus: planned\n\n## Direction\nText.\n";
        let mut r = parse_roadmap(text);
        r.add_version("v0.5.0", "Five", MilestoneStatus::Planned, None).unwrap();
        assert_eq!(r.milestones[0].version.as_deref(), Some("v0.5.0"), "before the first later version");
        r.add_version("v0.11.0", "Eleven", MilestoneStatus::Planned, Some("UI")).unwrap();
        assert_eq!(r.milestones.last().unwrap().version.as_deref(), Some("v0.11.0"), "after the last one");
        assert_eq!(r.milestones.last().unwrap().links, ["UI"]);
        assert!(r.render().contains("## Direction\nText."), "sections stay");
        assert_eq!(r.add_version("v0.5.0", "", MilestoneStatus::Planned, None), Err(TodoError::Exists("v0.5.0".into())));
        assert!(matches!(r.add_version("later", "", MilestoneStatus::Planned, None), Err(TodoError::Unsupported(_))));

        let mut empty = parse_roadmap("# Roadmap\n\n## Milestone: A\nStatus: planned\n");
        empty.add_version("v1.0.0", "First", MilestoneStatus::Released, None).unwrap();
        assert!(empty.render().ends_with("## v1.0.0 — First\nStatus: released\n"), "{}", empty.render());
    }

    #[test]
    fn link_creates_or_extends_the_milestone_line() {
        let mut r = parse_roadmap("## Milestone: UI\n\n## v0.5.0\nTarget: 2026-11\nStatus: planned\n\n## v0.5.1\nMilestone: Other\n");
        r.link("v0.5.0", "UI").unwrap();
        assert!(r.render().contains("## v0.5.0\nTarget: 2026-11\nStatus: planned\nMilestone: UI\n"), "{}", r.render());
        r.link("v0.5.1", "UI").unwrap();
        assert!(r.render().contains("## v0.5.1\nMilestone: Other, UI\n"), "{}", r.render());
        assert_eq!(r.milestones[1].links, ["UI"]);
        let before = r.render();
        r.link("v0.5.0", "ui").unwrap();
        assert_eq!(r.render(), before, "already linked: nothing changes");
        assert_eq!(r.link("UI", "UI"), Err(TodoError::NotFound("UI".into())), "only versions take links");
    }

    #[test]
    fn move_entry_moves_the_whole_section() {
        let text = "# Roadmap\n\n## v0.1 — A\nStatus: done\n\n- [x] a\n\n## v0.2 — B\nStatus: planned\n\n- [ ] b\n\n## Direction\nKept.\n";
        let mut r = parse_roadmap(text);
        r.move_entry("v0.2", Some("v0.1")).unwrap();
        assert_eq!(r.milestones.iter().map(|m| m.version.clone().unwrap()).collect::<Vec<_>>(), ["v0.2", "v0.1"]);
        assert_eq!(r.milestones[0].items.len(), 1, "items travel with the section");
        assert!(r.render().starts_with("# Roadmap\n\n## v0.2 — B\nStatus: planned\n\n- [ ] b\n\n## v0.1 — A"), "{}", r.render());
        r.move_entry("v0.2", None).unwrap();
        assert_eq!(r.milestones[1].version.as_deref(), Some("v0.2"));
        assert!(r.render().ends_with("## Direction\nKept.\n\n## v0.2 — B\nStatus: planned\n\n- [ ] b\n"), "{}", r.render());
        assert_eq!(r.move_entry("v0.2", Some("v0.2")), Ok(()));
        assert_eq!(r.move_entry("v0.2", Some("nope")), Err(TodoError::NotFound("nope".into())));
    }

    #[test]
    fn edits_keep_every_other_line() {
        let mut r = parse_roadmap(RM);
        r.add_version("v0.7.0", "Seven", MilestoneStatus::Planned, None).unwrap();
        assert!(r.render().starts_with(RM.trim_end()), "appended without touching the rest");
        r.move_entry("v0.7.0", Some("v0.5")).unwrap();
        let mut kept = r.render().lines().map(str::to_string).collect::<Vec<_>>();
        kept.retain(|l| !l.contains("v0.7.0") && !l.contains("Seven") && !l.starts_with("Status: planned"));
        assert!(kept.join("\n").contains("Free text.\n\n- [x] one\n- [ ] two\n"));
    }

    #[test]
    fn tables_are_read_only_until_converted() {
        let mut r = parse_roadmap("## A\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(!r.editable);
        assert_eq!(r.set_status("A", MilestoneStatus::Done), Err(TodoError::ReadOnly));
        assert_eq!(r.add_milestone("B", MilestoneStatus::Done), Err(TodoError::ReadOnly));
        assert_eq!(r.add_version("v1.0", "", MilestoneStatus::Planned, None), Err(TodoError::ReadOnly));
        assert_eq!(r.move_entry("A", None), Err(TodoError::ReadOnly));
    }

    #[test]
    fn table_conversion_gives_an_editable_roadmap() {
        let table = "# Roadmap\n\n## Milestones\n\n| Version | Name | Contents | Gate (must be resolved first) |\n|---|---|---|---|\n| v0.4.1 (done) | **Spec v7** (adopted as v7.1) | Lua stuff | none |\n| v0.4.2 (built, no capture) | Leftovers | Lint | none |\n| v0.5 | Next | Things | HP-1 |\n\nTail with a - bullet.\n";
        let c = convert_roadmap(table);
        let r = parse_roadmap(&c);
        assert!(r.editable, "{c}");
        let titles: Vec<(&str, &MilestoneStatus)> = r.milestones.iter().map(|m| (m.title.as_str(), &m.status)).collect();
        assert_eq!(
            titles,
            [
                ("Spec v7 (adopted as v7.1)", &MilestoneStatus::Released),
                ("Leftovers", &MilestoneStatus::InProgress),
                ("Next", &MilestoneStatus::Planned),
            ],
            "the Milestones heading with text only is a section"
        );
        assert!(r.milestones[0].body.contains("Contents: Lua stuff") && r.milestones[0].body.contains("Gate (must be resolved first): none"));
        assert!(c.contains("Tail with a - bullet."), "text outside the table is kept");
        assert!(!c.contains('|'));
    }

    #[test]
    fn board_lanes_with_versions() {
        let text = "## Milestone: Vehicles\nStatus: planned\n\n## v0.4.0\nStatus: released\n\n## v0.5.0 — UI\nStatus: in progress\n\n## Milestone: Docs\nStatus: planned\n\n## v0.6.0\nStatus: planned\n\n## v0.7.0\nStatus: planned\n\n## Milestone: Later thing\nStatus: planned\n";
        let r = parse_roadmap(text);
        let b = board(&r);
        assert_eq!(b.done, [1]);
        assert_eq!(b.now, [2]);
        assert_eq!(b.next, [0, 3, 4], "planned up to and including the first unreleased version (v0.6.0)");
        assert_eq!(b.later, [5, 6]);
        let released = parse_roadmap("## v0.1.0\nStatus: released\n\n## Milestone: X\nStatus: planned\n");
        assert_eq!(board(&released).next, [1], "no unreleased version: the planned entries go on");
    }

    #[test]
    fn board_lanes_without_versions() {
        let text = "## A\nStatus: done\n\n## B\nStatus: in progress\n\n## C\nStatus: planned\n\n## D\nStatus: planned\n\n## E\nStatus: planned\n\n## F\nStatus: planned\n\n## G\nStatus: Blocked\n";
        let b = board(&parse_roadmap(text));
        assert_eq!((b.done.clone(), b.now.clone()), (vec![0], vec![1]));
        assert_eq!(b.next, [2, 3, 4], "the first three planned");
        assert_eq!(b.later, [5, 6], "the rest, then unknown statuses");
    }

    #[test]
    fn release_suggestions_match_tags_with_prefixes() {
        let r = parse_roadmap(
            "## v0.4.0\nStatus: planned\n\n## v0.5.0\nStatus: planned\n\n## 1.0.0\nStatus: planned\n\n## v0.6.0\nStatus: released\n\n## v0.7.0\nStatus: planned\n",
        );
        let tags: Vec<String> = ["v0.4.0", "1.0.0", "v0.6.0"].iter().map(|s| s.to_string()).collect();
        assert_eq!(release_suggestions(&r, &tags, "v"), ["v0.4.0", "1.0.0"], "tag v0.4.0, or the version as written");
        assert_eq!(release_suggestions(&r, &[], "v"), Vec::<String>::new(), "no tags, no suggestions");
        let spec: Vec<String> = vec!["spec-v0.5.0".to_string()];
        assert_eq!(release_suggestions(&r, &spec, "v"), Vec::<String>::new(), "the prefix must match");
        assert_eq!(release_suggestions(&r, &spec, "spec-v"), ["v0.5.0"]);
    }

    #[test]
    fn semver_orders_numbers_and_prereleases() {
        assert_eq!(semver_cmp("v0.9", "v0.10"), Ordering::Less);
        assert_eq!(semver_cmp("1.2", "v1.2.0"), Ordering::Equal);
        assert_eq!(semver_cmp("v1.0.0-beta", "v1.0.0"), Ordering::Less);
        assert_eq!(semver_cmp("v1.0.0", "bogus"), Ordering::Less, "unparsable last");
        assert_eq!(semver_cmp("bogus", "v1.0.0"), Ordering::Greater);
    }

    #[test]
    fn find_roadmap_order() {
        let dir = std::env::temp_dir().join(format!("pa-roadmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/roadmap.md"), "").unwrap();
        // Windows folds case, so the returned spelling is the candidate's, not the file's.
        assert!(find_roadmap(&dir).is_some_and(|p| p.ends_with("docs/ROADMAP.md") || p.ends_with("docs/roadmap.md")));
        std::fs::write(dir.join("ROADMAP.md"), "").unwrap();
        assert_eq!(find_roadmap(&dir), Some(dir.join("ROADMAP.md")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
