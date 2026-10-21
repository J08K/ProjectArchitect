//! The TODO file (SPEC § 7.2, D-1, D-5): a markdown file whose items follow one [`Style`], parsed and edited losslessly.
//!
//! [`parse`] keeps every line. [`TodoDoc::render`] gives the text back byte for byte; an edit changes only the lines
//! of the item it touches. Item ids hash section, text and occurrence, so they survive edits elsewhere in the file.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// What an item line starts with: `- [ ] x`, `- x` / `* x`, `1. x`, or a bare line (TODO.txt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Checkbox,
    Bullet,
    Numbered,
    Line,
}

/// How an item is marked done: `[x]`, moved into the `##` section named, or removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    /// `[x]`. Only meaningful for checkboxes.
    Tick,
    /// Done items sit in this section (newest first, as in "Done recently").
    MoveTo(String),
    Delete,
}

/// A project's TODO convention. Stored per project as [`Style::encode`]; [`Style::detect`] guesses it from the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Style {
    pub marker: Marker,
    pub completion: Completion,
}

impl Style {
    /// The app format (SPEC § 7.2): checkboxes, ticked when done.
    pub const CHECKLIST: Style = Style { marker: Marker::Checkbox, completion: Completion::Tick };

    /// Guesses the style of `text`. Any checkbox makes it [`Style::CHECKLIST`]. Otherwise bullets, then numbered items,
    /// decide; done items go to the first `## Done…` section if there is one, else they are deleted. A file with no
    /// markdown structure is a list of lines. Text with no items at all is the app format.
    pub fn detect(text: &str) -> Style {
        let lines = split(text).0;
        let kinds = classify(&lines, None);
        let markers: Vec<Marker> =
            lines.iter().zip(&kinds).filter(|(_, k)| **k == Kind::Item).filter_map(|(l, _)| list_item(l)).map(|li| li.marker).collect();
        let done = lines
            .iter()
            .zip(&kinds)
            .filter(|(_, k)| **k == Kind::Heading)
            .map(|(l, _)| heading_title(l))
            .find(|t| t.to_ascii_lowercase().starts_with("done"));
        let completion = done.map_or(Completion::Delete, Completion::MoveTo);
        if markers.contains(&Marker::Checkbox) {
            Style::CHECKLIST
        } else if markers.contains(&Marker::Bullet) {
            Style { marker: Marker::Bullet, completion }
        } else if markers.contains(&Marker::Numbered) {
            Style { marker: Marker::Numbered, completion }
        } else if plain_lines(&lines) {
            Style { marker: Marker::Line, completion: Completion::Delete }
        } else {
            Style::CHECKLIST
        }
    }

    /// The storage form: `checkbox`, `bullet:delete`, `bullet:move:Done recently`, `line:delete`, ...
    pub fn encode(&self) -> String {
        let m = match self.marker {
            Marker::Checkbox => "checkbox",
            Marker::Bullet => "bullet",
            Marker::Numbered => "numbered",
            Marker::Line => "line",
        };
        match &self.completion {
            Completion::Tick => m.to_string(),
            Completion::MoveTo(s) => format!("{m}:move:{s}"),
            Completion::Delete => format!("{m}:delete"),
        }
    }

    /// Reads [`Style::encode`]'s form. `None` for anything else, and for a bare non-checkbox marker (ticks are checkboxes only).
    pub fn decode(s: &str) -> Option<Style> {
        let mut parts = s.splitn(3, ':');
        let marker = match parts.next()? {
            "checkbox" => Marker::Checkbox,
            "bullet" => Marker::Bullet,
            "numbered" => Marker::Numbered,
            "line" => Marker::Line,
            _ => return None,
        };
        let completion = match (parts.next(), parts.next()) {
            (None, _) if marker == Marker::Checkbox => Completion::Tick,
            (Some("delete"), None) => Completion::Delete,
            (Some("move"), Some(title)) if !title.is_empty() => Completion::MoveTo(title.to_string()),
            _ => return None,
        };
        Some(Style { marker, completion })
    }

    /// Text for the UI, e.g. "Checkboxes, ticked when done" or "Bullets, moved to “Done recently” when done".
    pub fn label(&self) -> String {
        let what = match self.marker {
            Marker::Checkbox => "Checkboxes",
            Marker::Bullet => "Bullets",
            Marker::Numbered => "Numbered items",
            Marker::Line => "Plain lines",
        };
        let when = match &self.completion {
            Completion::Tick => "ticked when done".to_string(),
            Completion::MoveTo(s) => format!("moved to “{s}” when done"),
            Completion::Delete => "deleted when done".to_string(),
        };
        format!("{what}, {when}")
    }
}

/// A `##` heading.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    /// 0-based line of the heading.
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TodoItem {
    /// Stable: 12 hex chars of FNV-1a 64 over `section\0text\0occurrence`.
    pub id: String,
    /// The `##` title above the item; `""` before the first section.
    pub section: String,
    /// 0-based first line.
    pub line: usize,
    /// Exclusive end of the item's continuation lines (children are separate items).
    pub end: usize,
    /// Indent in levels (two spaces, a tab counts as four).
    pub depth: usize,
    pub done: bool,
    /// First line and continuation lines, joined with `\n`, trimmed.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TodoDoc {
    /// Every line without its terminator, verbatim. Edits change these and re-parse.
    pub lines: Vec<String>,
    pub sections: Vec<Section>,
    /// All items of the style in document order, children included.
    pub items: Vec<TodoItem>,
    /// Always true now: every style can be edited. Other list lines and prose stay verbatim.
    pub editable: bool,
    pub style: Style,
    crlf: bool,
    trailing_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TodoError {
    /// The file cannot be edited (only roadmaps with tables get this now).
    ReadOnly,
    NotFound(String),
    /// A section (or milestone) with that title exists already.
    Exists(String),
    /// The style cannot do this: reopening an item that was deleted when done, or ticking a non-checkbox.
    Unsupported(String),
}

impl fmt::Display for TodoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TodoError::ReadOnly => write!(f, "the file has list items the app cannot edit; convert it to the app format first"),
            TodoError::NotFound(id) => write!(f, "no item with id {id}"),
            TodoError::Exists(title) => write!(f, "{title:?} already exists"),
            TodoError::Unsupported(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for TodoError {}

#[derive(Debug)]
pub enum SaveError {
    /// The file changed since it was read; nothing was written.
    Changed,
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Changed => write!(f, "file changed outside the app; reload it"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// Parse a TODO file in the style it detects. Any text round-trips through [`TodoDoc::render`].
pub fn parse(text: &str) -> TodoDoc {
    parse_with(text, &Style::detect(text))
}

/// Parse a TODO file in a known style (a project's stored one, or [`Style::CHECKLIST`] for roadmaps).
pub fn parse_with(text: &str, style: &Style) -> TodoDoc {
    let (lines, crlf, trailing_newline) = split(text);
    build(lines, crlf, trailing_newline, style.clone())
}

impl TodoDoc {
    pub fn render(&self) -> String {
        join(&self.lines, self.crlf, self.trailing_newline)
    }

    pub fn item(&self, id: &str) -> Option<&TodoItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|i| !i.done).count()
    }

    /// Marks an item done or open, per the style: a tick changes only the box; `MoveTo` moves the item (with its
    /// children) to the top of the done section, or to the first other section when reopened; `Delete` removes it
    /// when done and cannot reopen it.
    pub fn set_done(&mut self, id: &str, done: bool) -> Result<(), TodoError> {
        self.writable()?;
        let (line, was) = {
            let item = self.find(id)?;
            (item.line, item.done)
        };
        match self.style.completion.clone() {
            Completion::Tick => {
                if self.style.marker != Marker::Checkbox {
                    return Err(TodoError::Unsupported("only checkboxes can be ticked".into()));
                }
                if was == done {
                    return Ok(());
                }
                let cb = list_item(&self.lines[line]).ok_or_else(|| TodoError::NotFound(id.to_string()))?;
                self.lines[line].replace_range(cb.at + 1..cb.at + 2, if done { "x" } else { " " });
                self.refresh();
            }
            Completion::MoveTo(section) => {
                if was != done {
                    if done {
                        self.move_block(line, &section, true);
                    } else {
                        let open = self.open_section(&section);
                        self.move_block(line, &open, false);
                    }
                }
            }
            Completion::Delete => {
                if !done {
                    return Err(TodoError::Unsupported("items deleted when done cannot be reopened".into()));
                }
                self.remove(id)?;
            }
        }
        Ok(())
    }

    /// Replaces the item's text (first line and continuation lines); the marker and children stay.
    pub fn set_text(&mut self, id: &str, text: &str) -> Result<(), TodoError> {
        self.writable()?;
        let (line, end) = {
            let item = self.find(id)?;
            (item.line, item.end)
        };
        let prefix = self.lines[line][..item_text_off(&self.lines[line], self.style.marker)].to_string();
        splice_lines(&mut self.lines, line, end, text_lines(&prefix, text));
        self.refresh();
        Ok(())
    }

    /// Appends a top-level item to `section` (after its last top-level item with children). A missing section is
    /// created at the end; `""` is the area before the first heading. Returns the new id.
    pub fn add(&mut self, section: &str, text: &str) -> Result<String, TodoError> {
        self.writable()?;
        let prev = self.items.iter().rev().find(|i| i.depth == 0 && i.section == section).map(|i| i.line);
        let block = text_lines(&self.marker_for(0, prev), text);
        let at = self.insert_block(section, block, false);
        self.refresh();
        Ok(self.id_at(at))
    }

    /// Like [`TodoDoc::add`], but a missing section is created above the first `##` heading (an inbox on top)
    /// instead of at the end. Returns the new id.
    pub fn add_on_top(&mut self, section: &str, text: &str) -> Result<String, TodoError> {
        let first = self.sections.first().map(|s| s.line);
        let (Some(first), false) = (first, section.is_empty() || self.region(section).is_some()) else {
            return self.add(section, text);
        };
        self.writable()?;
        let mut block = vec![format!("## {section}"), String::new()];
        block.extend(text_lines(&self.marker_for(0, None), text));
        block.push(String::new());
        splice_lines(&mut self.lines, first, first, block);
        self.refresh();
        Ok(self.id_at(first + 2))
    }

    /// Appends a child to the item, after its existing children. Returns the new id.
    pub fn add_child(&mut self, parent_id: &str, text: &str) -> Result<String, TodoError> {
        self.writable()?;
        let parent = self.find(parent_id)?.line;
        let indent = indent_width(&self.lines[parent]) + 2;
        let end = subtree_end(&self.lines, &self.kinds(), parent);
        let prev = self.items.iter().rev().find(|i| i.line > parent && i.line < end && indent_width(&self.lines[i.line]) == indent).map(|i| i.line);
        let block = text_lines(&self.marker_for(indent, prev), text);
        splice_lines(&mut self.lines, end, end, block);
        self.refresh();
        Ok(self.id_at(end))
    }

    /// Removes the item with its children.
    pub fn remove(&mut self, id: &str) -> Result<(), TodoError> {
        self.writable()?;
        let line = self.find(id)?.line;
        let (start, end) = self.block_at(line);
        splice_lines(&mut self.lines, start, end, Vec::new());
        self.refresh();
        Ok(())
    }

    /// Swaps the item (with its children) with its previous or next sibling, `delta` times. Moving past the first or
    /// last sibling does nothing.
    pub fn move_by(&mut self, id: &str, delta: i32) -> Result<(), TodoError> {
        self.writable()?;
        let mut line = self.find(id)?.line;
        let down = delta > 0;
        for _ in 0..delta.unsigned_abs() {
            let siblings = self.siblings(line);
            let Some(pos) = siblings.iter().position(|&l| l == line) else { break };
            let other = if down { siblings.get(pos + 1) } else { pos.checked_sub(1).and_then(|p| siblings.get(p)) };
            let Some(&other) = other else { break };
            let (a, b) = if down { (line, other) } else { (other, line) };
            let (a0, a1) = self.block_at(a);
            let (b0, b1) = self.block_at(b);
            // Layout [A][gap][B] becomes [B][gap][A]; the gap (blank lines) stays where it was.
            let gap = b0 - a1;
            let mut new = self.lines[b0..b1].to_vec();
            new.extend_from_slice(&self.lines[a1..b0]);
            new.extend_from_slice(&self.lines[a0..a1]);
            splice_lines(&mut self.lines, a0, b1, new);
            line = if down { a0 + (b1 - b0) + gap } else { a0 };
            self.refresh();
        }
        Ok(())
    }

    /// Moves the item with its children to the end of `section`, re-indented to depth 0. Its id changes.
    pub fn move_to(&mut self, id: &str, section: &str) -> Result<(), TodoError> {
        self.writable()?;
        let line = self.find(id)?.line;
        self.move_block(line, section, false);
        Ok(())
    }

    /// Moves the item with its children to just before `target`, re-indented to the target's depth (drag and drop).
    /// Dropping an item onto itself or one of its own children does nothing. Its id may change.
    pub fn move_before(&mut self, id: &str, target: &str) -> Result<(), TodoError> {
        self.writable()?;
        let (line, at) = (self.find(id)?.line, self.find(target)?.line);
        let (start, end) = self.block_at(line);
        if (start..end).contains(&at) {
            return Ok(());
        }
        let (from, to) = (indent_width(&self.lines[line]), indent_width(&self.lines[at]));
        let pad = " ".repeat(to);
        let block: Vec<String> = self.lines[start..end].iter().map(|l| dedent(l, from)).map(|l| if l.trim().is_empty() { l } else { format!("{pad}{l}") }).collect();
        splice_lines(&mut self.lines, start, end, Vec::new());
        let at = if at > start { at - (end - start) } else { at };
        splice_lines(&mut self.lines, at, at, block);
        self.refresh();
        Ok(())
    }

    /// Moves every done top-level item (with children) to a `## Done` section, created at the end if missing.
    /// Returns how many moved. Only ticked checkboxes are cleared: the other styles have nothing to clear.
    pub fn clear_done(&mut self) -> usize {
        if !self.editable || self.style.completion != Completion::Tick {
            return 0;
        }
        let mut moved = 0;
        while let Some(line) = self.items.iter().find(|i| i.done && i.depth == 0 && i.section != "Done").map(|i| i.line) {
            self.move_block(line, "Done", false);
            moved += 1;
        }
        moved
    }

    fn writable(&self) -> Result<(), TodoError> {
        if self.editable { Ok(()) } else { Err(TodoError::ReadOnly) }
    }

    fn find(&self, id: &str) -> Result<&TodoItem, TodoError> {
        self.item(id).ok_or_else(|| TodoError::NotFound(id.to_string()))
    }

    fn id_at(&self, line: usize) -> String {
        self.items.iter().find(|i| i.line == line).map(|i| i.id.clone()).unwrap_or_default()
    }

    /// Re-parse after `lines` changed.
    fn refresh(&mut self) {
        let lines = std::mem::take(&mut self.lines);
        *self = build(lines, self.crlf, self.trailing_newline, self.style.clone());
    }

    fn kinds(&self) -> Vec<Kind> {
        classify(&self.lines, Some(self.style.marker))
    }

    /// The marker of a new item at `indent`, after the item on line `prev`. Numbered items count on from it
    /// (ponytail: no renumbering of the others).
    fn marker_for(&self, indent: usize, prev: Option<usize>) -> String {
        let pad = " ".repeat(indent);
        match self.style.marker {
            Marker::Checkbox => format!("{pad}- [ ]"),
            Marker::Bullet => format!("{pad}-"),
            Marker::Numbered => {
                let n = prev.and_then(|l| list_item(&self.lines[l])).map_or(0, |li| li.number);
                format!("{pad}{}.", n + 1)
            }
            Marker::Line => pad,
        }
    }

    /// `(first line, exclusive end)` of the item at `line` with its children.
    fn block_at(&self, line: usize) -> (usize, usize) {
        (line, subtree_end(&self.lines, &self.kinds(), line))
    }

    /// Lines of the items that share `line`'s section, indent and parent, in document order.
    fn siblings(&self, line: usize) -> Vec<usize> {
        let kinds = self.kinds();
        let ends: Vec<usize> = self.items.iter().map(|i| subtree_end(&self.lines, &kinds, i.line)).collect();
        let indent: Vec<usize> = self.items.iter().map(|i| indent_width(&self.lines[i.line])).collect();
        // The parent is the nearest earlier item with less indent whose children reach this item.
        let parent: Vec<Option<usize>> =
            (0..self.items.len()).map(|k| (0..k).rev().find(|&j| indent[j] < indent[k] && ends[j] > self.items[k].line)).collect();
        let Some(me) = self.items.iter().position(|i| i.line == line) else { return Vec::new() };
        (0..self.items.len())
            .filter(|&k| self.items[k].section == self.items[me].section && indent[k] == indent[me] && parent[k] == parent[me])
            .map(|k| self.items[k].line)
            .collect()
    }

    /// Moves the block at `line` into `section`: appended, or inserted at the top when `top`.
    fn move_block(&mut self, line: usize, section: &str, top: bool) {
        let (start, end) = self.block_at(line);
        let width = indent_width(&self.lines[line]);
        let block: Vec<String> = self.lines[start..end].iter().map(|l| dedent(l, width)).collect();
        splice_lines(&mut self.lines, start, end, Vec::new());
        self.refresh();
        self.insert_block(section, block, top);
        self.refresh();
    }

    /// Where a reopened item goes: the end of the first section that is not `done`, or before the first heading.
    fn open_section(&self, done: &str) -> String {
        self.sections.iter().find(|s| s.title != done).map_or_else(String::new, |s| s.title.clone())
    }

    /// Insert `block` into `section` (appended after its last top-level item, or first when `top`); returns the line
    /// it starts at. Does not re-parse.
    fn insert_block(&mut self, section: &str, block: Vec<String>, top: bool) -> usize {
        if self.lines.is_empty() {
            self.trailing_newline = true;
        }
        let at = match self.region(section) {
            Some((start, end)) if top => (start..end).find(|&i| !self.lines[i].trim().is_empty()).unwrap_or(end),
            Some((start, end)) => {
                let kinds = self.kinds();
                match self.items.iter().rev().find(|i| i.depth == 0 && i.line >= start && i.line < end) {
                    Some(item) => subtree_end(&self.lines, &kinds, item.line),
                    None => (start..end).rev().find(|&i| !self.lines[i].trim().is_empty()).map_or(start, |i| i + 1),
                }
            }
            None => {
                if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("## {section}"));
                self.lines.len()
            }
        };
        splice_lines(&mut self.lines, at, at, block);
        at
    }

    /// Body lines of a section (`""` = before the first heading), as `[start, end)`; `None` for a missing section.
    fn region(&self, section: &str) -> Option<(usize, usize)> {
        let n = self.lines.len();
        if section.is_empty() {
            return Some((0, self.sections.first().map_or(n, |s| s.line)));
        }
        let k = self.sections.iter().position(|s| s.title == section)?;
        Some((self.sections[k].line + 1, self.sections.get(k + 1).map_or(n, |s| s.line)))
    }
}

/// Reads a TODO file: `(text, parsed)`.
pub fn load(path: &Path) -> io::Result<(String, TodoDoc)> {
    let text = std::fs::read_to_string(path)?;
    let doc = parse(&text);
    Ok((text, doc))
}

/// The first existing TODO file of a project root.
pub fn find_file(root: &Path) -> Option<PathBuf> {
    ["TODO.md", "TODOS.md", "TODO.txt", "todo.md", "docs/TODO.md"].iter().map(|n| root.join(n)).find(|p| p.is_file())
}

/// Write `new_text` to `path` if the file still reads as `read_text` (a missing file reads as `""`). Atomic: a `.tmp`
/// file, then a rename over the original.
pub fn save(path: &Path, read_text: &str, new_text: &str) -> Result<(), SaveError> {
    let now = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    if now != read_text {
        return Err(SaveError::Changed);
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, new_text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Copy `path` to `data_dir/backups/<unix secs>-<absolute path with separators and ':' as '_'>`.
pub fn backup(data_dir: &Path, path: &Path) -> io::Result<PathBuf> {
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let name = crate::norm(path).replace(['\\', '/', ':'], "_");
    let dest = dir.join(format!("{}-{name}", crate::now_secs()));
    std::fs::copy(path, &dest)?;
    Ok(dest)
}

/// Convert another style to the app format: see [`convert_to`].
pub fn convert(text: &str) -> String {
    convert_to(text, &Style::CHECKLIST)
}

/// Convert `text` to the style `to`. Every list item becomes the target's marker, indentation, children and
/// continuation lines kept. In a file with no markdown at all, every text line is an item. Done items follow the
/// target: a checkbox target ticks them; `MoveTo` moves them to the top of their section; `Delete` moves them to a
/// `## Done` section as plain items (nothing is dropped). Headings and other prose stay verbatim.
pub fn convert_to(text: &str, to: &Style) -> String {
    let (lines, crlf, trailing) = split(text);
    let src = Style::detect(text);
    // ponytail: "no markdown at all" is the only trigger, so a markdown-free .md file is converted the same way.
    let plain = !lines.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with(['#', '|']) || t.starts_with("```") || t.starts_with("~~~") || list_item(l).is_some()
    });
    let kinds = classify(&lines, None);

    // Stage 1: every item becomes a checkbox, ticked when it is done in the source style.
    let mut section = String::new();
    let mut carrier = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let lead = &line[..indent_len(line)];
        let in_src_done = matches!(&src.completion, Completion::MoveTo(s) if *s == section);
        match kinds[i] {
            Kind::Heading => {
                section = heading_title(line);
                carrier.push(line.clone());
            }
            Kind::Item => {
                let content = &line[item_text_off(line, Marker::Checkbox)..];
                let done = match list_item(line).filter(|li| li.marker == Marker::Checkbox) {
                    Some(li) => li.done,
                    None => struck_or_done(content),
                };
                carrier.push(carrier_line(lead, content, done || in_src_done));
            }
            Kind::Text if plain => {
                let body = line.trim_start_matches([' ', '\t']);
                carrier.push(carrier_line(lead, body, struck_or_done(body) || in_src_done));
            }
            _ => carrier.push(line.clone()),
        }
    }
    let mut doc = build(carrier, crlf, trailing, Style::CHECKLIST);
    // ponytail: a tick only means something on a checkbox, so any other "tick" target deletes when done.
    let to = match (&to.completion, to.marker) {
        (Completion::Tick, m) if m != Marker::Checkbox => Style { marker: m, completion: Completion::Delete },
        _ => to.clone(),
    };
    if to == Style::CHECKLIST {
        return join(&doc.lines, crlf, trailing);
    }

    // Stage 2: done items that are not yet where the target keeps them move there, with their children. A done child
    // of a moved item comes along, so every moved block is a whole subtree.
    let dest = match &to.completion {
        Completion::MoveTo(s) => s.clone(),
        _ => "Done".to_string(),
    };
    let mut picks: Vec<(usize, usize)> = Vec::new();
    for item in doc.items.iter().filter(|i| i.done && i.section != dest) {
        if picks.last().is_some_and(|&(_, end)| item.line < end) {
            continue;
        }
        picks.push(doc.block_at(item.line));
    }
    let mut block = Vec::new();
    for &(s, e) in &picks {
        let width = indent_width(&doc.lines[s]);
        block.extend(doc.lines[s..e].iter().map(|l| dedent(l, width)));
    }
    for &(s, e) in picks.iter().rev() {
        splice_lines(&mut doc.lines, s, e, Vec::new());
    }
    doc.refresh();
    if !block.is_empty() {
        doc.insert_block(&dest, block, matches!(to.completion, Completion::MoveTo(_)));
    }
    doc.refresh();

    // Stage 3: the target marker on every item; numbers run per section (ponytail: not per level).
    let mut edits = Vec::with_capacity(doc.items.len());
    let (mut current, mut number) = (String::new(), 0);
    for item in &doc.items {
        if item.section != current {
            current = item.section.clone();
            number = 0;
        }
        number += 1;
        let line = &doc.lines[item.line];
        let lead = &line[..indent_len(line)];
        let content = &line[item_text_off(line, Marker::Checkbox)..];
        let marker = match to.marker {
            Marker::Checkbox => "- [ ] ".to_string(),
            Marker::Bullet => "- ".to_string(),
            Marker::Numbered => format!("{number}. "),
            Marker::Line => String::new(),
        };
        edits.push((item.line, format!("{lead}{marker}{content}").trim_end().to_string()));
    }
    for (line, text) in edits {
        doc.lines[line] = text;
    }
    join(&doc.lines, crlf, trailing)
}

/// `lead` (indentation) + `- [ ] content`, or `- [x]` when `done`.
fn carrier_line(lead: &str, content: &str, done: bool) -> String {
    format!("{lead}- [{}] {}", if done { 'x' } else { ' ' }, content.trim_end()).trim_end().to_string()
}

/// The old heuristic for non-checkbox items: struck through, or says `(done)`.
fn struck_or_done(content: &str) -> bool {
    content.starts_with("~~") || content.to_ascii_lowercase().contains("(done)")
}

/// No markdown structure (headings, tables, fences) and at least one non-empty line.
fn plain_lines(lines: &[String]) -> bool {
    let structured = lines.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with(['#', '|']) || t.starts_with("```") || t.starts_with("~~~")
    });
    !structured && lines.iter().any(|l| !l.trim().is_empty())
}

/// Lines without terminators, whether the text was CRLF throughout, and whether it ended with a newline.
pub(crate) fn split(text: &str) -> (Vec<String>, bool, bool) {
    if text.is_empty() {
        return (Vec::new(), false, false);
    }
    let bytes = text.as_bytes();
    let crlf = text.contains('\n') && text.match_indices('\n').all(|(i, _)| i > 0 && bytes[i - 1] == b'\r');
    let trailing = text.ends_with('\n');
    let mut pieces: Vec<&str> = text.split('\n').collect();
    let n = pieces.len();
    if trailing {
        pieces.pop();
    }
    // Only lines followed by `\n` lose their `\r`, so a lone `\r` at the very end survives.
    let lines = pieces.iter().enumerate().map(|(i, p)| if crlf && i + 1 < n { p.strip_suffix('\r').unwrap_or(p) } else { p }.to_string()).collect();
    (lines, crlf, trailing)
}

pub(crate) fn join(lines: &[String], crlf: bool, trailing_newline: bool) -> String {
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut s = lines.join(nl);
    if trailing_newline && !lines.is_empty() {
        s.push_str(nl);
    }
    s
}

/// What a line is to the parser. Lines inside a code fence are `Text` (or `Blank`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Blank,
    Text,
    Fence,
    /// A `##` heading.
    Heading,
    /// A line of the style's marker (any list line when classifying for conversion).
    Item,
    /// A list line of another marker, or a `#` line in a bare-line style: kept verbatim.
    Other,
}

/// A list line: its marker, the checkbox state, the byte offsets of its marker (`[` for a checkbox) and its text,
/// and its number.
#[derive(Clone, Copy, Debug)]
struct ListLine {
    marker: Marker,
    done: bool,
    at: usize,
    text_off: usize,
    number: usize,
}

/// `want` `None`: every list line is an `Item`. A bare-line style makes every non-heading line an `Item`.
fn classify(lines: &[String], want: Option<Marker>) -> Vec<Kind> {
    let mut fence = false;
    lines
        .iter()
        .map(|line| {
            let t = line.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                fence = !fence;
                return Kind::Fence;
            }
            if line.trim().is_empty() {
                return Kind::Blank;
            }
            if fence {
                return Kind::Text;
            }
            if line.starts_with("## ") || line == "##" {
                return Kind::Heading;
            }
            match want {
                Some(Marker::Line) if line.starts_with('#') => Kind::Other,
                Some(Marker::Line) => Kind::Item,
                _ => match list_item(line) {
                    Some(li) if want.is_none_or(|w| w == li.marker) => Kind::Item,
                    Some(_) => Kind::Other,
                    None => Kind::Text,
                },
            }
        })
        .collect()
}

fn build(lines: Vec<String>, crlf: bool, trailing_newline: bool, style: Style) -> TodoDoc {
    let kinds = classify(&lines, Some(style.marker));
    let mut sections = Vec::new();
    let mut items = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut section = String::new();
    for (i, line) in lines.iter().enumerate() {
        match kinds[i] {
            Kind::Heading => {
                section = heading_title(line);
                sections.push(Section { title: section.clone(), line: i });
            }
            Kind::Item => {
                let indent = indent_width(line);
                let mut parts = vec![line[item_text_off(line, style.marker)..].trim()];
                let mut end = i + 1;
                for j in i + 1..lines.len() {
                    if kinds[j] != Kind::Text || indent_width(&lines[j]) <= indent {
                        break;
                    }
                    parts.push(lines[j].trim());
                    end = j + 1;
                }
                let text = parts.join("\n").trim().to_string();
                let occurrence = {
                    let count = seen.entry(format!("{section}\0{text}")).or_insert(0);
                    *count += 1;
                    *count - 1
                };
                let done = match &style.completion {
                    Completion::Tick => list_item(line).is_some_and(|li| li.marker == Marker::Checkbox && li.done),
                    Completion::MoveTo(s) => *s == section,
                    Completion::Delete => false,
                };
                items.push(TodoItem {
                    id: item_id(&section, &text, occurrence),
                    section: section.clone(),
                    line: i,
                    end,
                    depth: indent / 2,
                    done,
                    text,
                });
            }
            _ => {}
        }
    }
    TodoDoc { lines, sections, items, editable: true, style, crlf, trailing_newline }
}

/// Title of a `##` heading line.
fn heading_title(line: &str) -> String {
    line.trim_start_matches("##").trim().to_string()
}

/// 12 hex chars of FNV-1a 64 over `section\0text\0occurrence`.
fn item_id(section: &str, text: &str, occurrence: usize) -> String {
    let key = format!("{section}\0{text}\0{occurrence}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")[..12].to_string()
}

/// Indent in columns (tabs count as four).
fn indent_width(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// Bytes of leading indentation.
fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Byte offset of the item text: after the list marker (and checkbox), or after the indentation for a bare line.
fn item_text_off(line: &str, marker: Marker) -> usize {
    match (marker, list_item(line)) {
        (Marker::Line, _) | (_, None) => indent_len(line),
        (_, Some(li)) => li.text_off,
    }
}

/// The marker of a list line and the text after it: `(marker, number, rest)`; `number` is 0 for bullets.
fn marker_rest(body: &str) -> Option<(Marker, usize, &str)> {
    if body == "-" || body == "*" {
        return Some((Marker::Bullet, 0, ""));
    }
    if let Some(rest) = body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
        return Some((Marker::Bullet, 0, rest));
    }
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !matches!(body.as_bytes().get(digits).copied(), Some(b'.' | b')')) {
        return None;
    }
    let number: usize = body[..digits].parse().unwrap_or(0);
    match body.as_bytes().get(digits + 1).copied() {
        None => Some((Marker::Numbered, number, "")),
        Some(b' ') => Some((Marker::Numbered, number, &body[digits + 2..])),
        Some(_) => None,
    }
}

/// `Some` for any list line (checkbox, bullet or numbered), `None` for any other line.
fn list_item(line: &str) -> Option<ListLine> {
    let body = line.trim_start_matches([' ', '\t']);
    let (marker, number, rest) = marker_rest(body)?;
    let rest_off = line.len() - rest.len();
    let boxed = rest.get(..3).is_some_and(|b| matches!(b, "[ ]" | "[x]" | "[X]")) && rest.as_bytes().get(3).is_none_or(|&c| c == b' ');
    if boxed {
        let text = rest[3..].trim_start();
        return Some(ListLine {
            marker: Marker::Checkbox,
            done: rest.as_bytes()[1] != b' ',
            at: rest_off,
            text_off: line.len() - text.len(),
            number: 0,
        });
    }
    Some(ListLine { marker, done: false, at: line.len() - body.len(), text_off: rest_off, number })
}

/// Exclusive end of the item at `i` with its children: after the last non-blank line indented deeper than it.
fn subtree_end(lines: &[String], kinds: &[Kind], i: usize) -> usize {
    let width = indent_width(&lines[i]);
    let mut end = i + 1;
    for j in i + 1..lines.len() {
        match kinds[j] {
            Kind::Blank => {}
            Kind::Heading => break,
            _ if indent_width(&lines[j]) <= width => break,
            _ => end = j + 1,
        }
    }
    end
}

/// A list item's lines: `prefix` (indent, marker, checkbox) before the first line of `text`, continuation lines
/// aligned under the text. Blank lines in `text` are dropped: they would end the item. ponytail: in a bare-line
/// style, a multi-line text becomes nested lines rather than one item.
fn text_lines(prefix: &str, text: &str) -> Vec<String> {
    let head = if prefix.is_empty() || prefix.ends_with(' ') { prefix.to_string() } else { format!("{prefix} ") };
    let pad = " ".repeat(head.chars().count());
    let parts: Vec<&str> = text.split('\n').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return vec![head.trim_end().to_string()];
    }
    parts.iter().enumerate().map(|(k, p)| if k == 0 { format!("{head}{p}") } else { format!("{pad}{p}") }).collect()
}

/// `line` without its first `width` columns of indentation. Blank lines are unchanged.
fn dedent(line: &str, width: usize) -> String {
    if line.trim().is_empty() {
        return line.to_string();
    }
    let mut cols = 0;
    let mut cut = 0;
    for (i, c) in line.char_indices() {
        if cols >= width || (c != ' ' && c != '\t') {
            break;
        }
        cols += if c == '\t' { 4 } else { 1 };
        cut = i + 1;
    }
    line[cut..].to_string()
}

fn splice_lines(lines: &mut Vec<String>, start: usize, end: usize, new: Vec<String>) {
    let tail = lines.split_off(end);
    lines.truncate(start);
    lines.extend(new);
    lines.extend(tail);
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "# TODO\n\nIntro prose.\n\n## Core\n\n- [ ] First item, which\n      wraps onto a second line.\n  - [ ] Child\n  - [x] Done child\n- [x] Done item\n\n| a | b |\n|---|---|\n\n## Later\n\n```\n- [ ] inside a fence\n```\n\n- [ ] Later item\n";
    /// A TODO.md like chess-clock's: bullets, done items in "Done recently".
    const CLOCK: &str =
        "# TODO\n\n## Done recently\n\n- Shipped one (brief x,\n  second line).\n- Shipped two\n\n## Later\n\n- Open one\n  - Child\n- Open two\n";
    /// A TODOS.md like trailmix-web's: bullets, no done section, CRLF.
    const TRAIL: &str = "# TODOs\r\n\r\n## Decision needed\r\n\r\n- **Sync:** question?\r\n- **Import:** review.\r\n\r\n## Backlog\r\n\r\n### Frontend\r\n\r\n- Item a\r\n";
    /// A TODO.md like pixel-forge's: checkboxes.
    const PIXEL: &str = "# TODO\n\n## Audio\n\n- [ ] Clock drift.\n      more\n- [x] Done thing\n  - [ ] child\n";
    /// A TODO.txt like trailmix's: bare lines, CRLF.
    const TXT: &str = "Search all\r\n  nested line\r\n\r\nOffline mode\r\n";
    const NUMBERED: &str = "## List\n1. one\n2. two\n";

    fn id(d: &TodoDoc, text: &str) -> String {
        d.items.iter().find(|i| i.text == text).unwrap().id.clone()
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-todo-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn style(marker: Marker, completion: Completion) -> Style {
        Style { marker, completion }
    }

    fn move_to(title: &str) -> Completion {
        Completion::MoveTo(title.into())
    }

    #[test]
    fn round_trip_is_exact() {
        for t in ["", "\n", "x", "a\r\nb\r\n", "a\r\nb", "a\r\nb\n", "a\r", "\r\n", APP, "- [ ] a\t\n\t- [ ] b\n\n\n", "## X\n- [ ] y\r\n"] {
            assert_eq!(parse(t).render(), t, "{t:?}");
        }
        for s in [Style::CHECKLIST, style(Marker::Bullet, move_to("Done")), style(Marker::Line, Completion::Delete)] {
            for t in [CLOCK, TRAIL, TXT, NUMBERED, APP] {
                assert_eq!(parse_with(t, &s).render(), t, "{s:?}");
            }
        }
    }

    #[test]
    fn detects_each_style() {
        assert_eq!(Style::detect(CLOCK), style(Marker::Bullet, move_to("Done recently")));
        assert_eq!(Style::detect(TRAIL), style(Marker::Bullet, Completion::Delete));
        assert_eq!(Style::detect(PIXEL), Style::CHECKLIST);
        assert_eq!(Style::detect(APP), Style::CHECKLIST);
        assert_eq!(Style::detect(TXT), style(Marker::Line, Completion::Delete));
        assert_eq!(Style::detect(NUMBERED), style(Marker::Numbered, Completion::Delete));
        assert_eq!(Style::detect(""), Style::CHECKLIST);
        assert_eq!(Style::detect("# Title\n## Sec\n"), Style::CHECKLIST);
        assert_eq!(Style::detect("## Done\n- x\n"), style(Marker::Bullet, move_to("Done")));
    }

    #[test]
    fn counts_per_fixture() {
        let d = parse(CLOCK);
        assert_eq!((d.items.len(), d.open_count()), (5, 3));
        let d = parse(TRAIL);
        assert_eq!((d.items.len(), d.open_count()), (3, 3));
        let d = parse(PIXEL);
        assert_eq!((d.items.len(), d.open_count()), (3, 2));
        let d = parse(TXT);
        assert_eq!((d.items.len(), d.open_count()), (3, 3));
        assert_eq!(d.items[1].depth, 1);
    }

    #[test]
    fn encode_decode_and_label() {
        let all = [
            Style::CHECKLIST,
            style(Marker::Checkbox, move_to("Done")),
            style(Marker::Checkbox, Completion::Delete),
            style(Marker::Bullet, move_to("Done recently")),
            style(Marker::Bullet, Completion::Delete),
            style(Marker::Numbered, move_to("Done: old")),
            style(Marker::Numbered, Completion::Delete),
            style(Marker::Line, Completion::Delete),
        ];
        for s in &all {
            assert_eq!(Style::decode(&s.encode()).as_ref(), Some(s), "{}", s.encode());
        }
        assert_eq!(Style::CHECKLIST.encode(), "checkbox");
        assert_eq!(style(Marker::Bullet, move_to("Done recently")).encode(), "bullet:move:Done recently");
        assert_eq!(style(Marker::Line, Completion::Delete).encode(), "line:delete");
        for bad in ["", "bullet", "bullet:tick", "bullet:move:", "bullet:move", "nope", "line:delete:x"] {
            assert_eq!(Style::decode(bad), None, "{bad:?}");
        }
        assert_eq!(Style::CHECKLIST.label(), "Checkboxes, ticked when done");
        assert_eq!(style(Marker::Bullet, move_to("Done recently")).label(), "Bullets, moved to “Done recently” when done");
        assert_eq!(style(Marker::Bullet, Completion::Delete).label(), "Bullets, deleted when done");
        assert_eq!(style(Marker::Line, Completion::Delete).label(), "Plain lines, deleted when done");
    }

    #[test]
    fn move_style_done_and_reopen() {
        let mut d = parse(CLOCK);
        let open_two = id(&d, "Open two");
        d.set_done(&open_two, true).unwrap();
        assert_eq!(d.lines[4], "- Open two", "newest first, at the top of the done section");
        // A move changes the section, so the id changes too.
        assert_eq!(d.item(&id(&d, "Open two")).unwrap().section, "Done recently");
        assert_eq!(d.open_count(), 2);
        d.set_done(&id(&d, "Open two"), false).unwrap();
        assert_eq!(d.item(&id(&d, "Open two")).unwrap().section, "Later", "reopened at the end of the first other section");
        assert_eq!(d.open_count(), 3);
        let child = id(&d, "Child");
        d.set_done(&child, true).unwrap();
        assert_eq!(d.item(&id(&d, "Child")).unwrap().depth, 0, "a moved child becomes top level");
        assert_eq!(d.clear_done(), 0, "clear done does nothing for this style");
    }

    #[test]
    fn move_style_creates_missing_section_and_keeps_other_lines() {
        let text = "## Todo\n- a\n\nProse stays.\n";
        let mut d = parse_with(text, &style(Marker::Bullet, move_to("Done")));
        let a = id(&d, "a");
        d.set_done(&a, true).unwrap();
        assert_eq!(d.render(), "## Todo\n\nProse stays.\n\n## Done\n- a\n", "section created at the end");
        let b = d.add("Todo", "b").unwrap();
        assert_eq!(d.item(&b).unwrap().section, "Todo");
        assert!(!d.item(&b).unwrap().done);
    }

    #[test]
    fn delete_style_removes_when_done_and_cannot_reopen() {
        let mut d = parse(TRAIL);
        let a = id(&d, "Item a");
        d.set_done(&a, true).unwrap();
        assert!(d.item(&a).is_none());
        assert_eq!(d.items.len(), 2);
        let w = id(&d, "**Sync:** question?");
        assert!(matches!(d.set_done(&w, false), Err(TodoError::Unsupported(_))));
        assert!(d.item(&w).is_some());
        let added = d.add("Decision needed", "New").unwrap();
        assert_eq!(d.lines[6], "- New", "after the section's last item");
        assert_eq!(d.item(&added).unwrap().text, "New");
    }

    #[test]
    fn numbered_items_count_on_and_lines_are_plain() {
        let mut d = parse(NUMBERED);
        d.add("List", "three").unwrap();
        assert_eq!(d.lines.last().unwrap(), "3. three");
        let one = id(&d, "one");
        d.set_text(&one, "uno").unwrap();
        assert_eq!(d.lines[1], "1. uno");
        let mut t = parse(TXT);
        t.add("", "New").unwrap();
        assert_eq!(t.lines.last().unwrap(), "New");
        let offline = id(&t, "Offline mode");
        t.set_done(&offline, true).unwrap();
        assert!(t.item(&offline).is_none(), "deleted when done");
        assert_eq!(t.lines, vec!["Search all", "  nested line", "", "New"]);
    }

    #[test]
    fn checkbox_move_style_keeps_boxes_open() {
        let mut d = parse_with("## Todo\n- [ ] a\n## Done\n- [ ] b\n", &style(Marker::Checkbox, move_to("Done")));
        assert_eq!(d.open_count(), 1);
        let a = id(&d, "a");
        d.set_done(&a, true).unwrap();
        assert_eq!(d.lines, vec!["## Todo", "## Done", "- [ ] a", "- [ ] b"]);
        assert_eq!(d.open_count(), 0);
    }

    #[test]
    fn convert_follows_done_state_to_the_target() {
        let c = convert_to(CLOCK, &Style::CHECKLIST);
        assert_eq!(parse(&c).open_count(), 3, "done items are ticked: {c}");
        let c = convert_to(CLOCK, &style(Marker::Bullet, Completion::Delete));
        assert!(c.contains("## Done\n- Shipped one (brief x,"), "{c}");
        let d = parse_with(&c, &style(Marker::Bullet, Completion::Delete));
        assert_eq!(d.items.len(), 5, "nothing dropped: {c}");
        let c = convert_to(TRAIL, &style(Marker::Checkbox, move_to("Done")));
        assert_eq!(parse_with(&c, &style(Marker::Checkbox, move_to("Done"))).items.len(), 3);
    }

    #[test]
    fn convert_between_all_styles_keeps_every_item_text() {
        let fixtures = [CLOCK, TRAIL, PIXEL, TXT, NUMBERED, APP];
        let targets = [
            Style::CHECKLIST,
            style(Marker::Checkbox, move_to("Done")),
            style(Marker::Bullet, Completion::Delete),
            style(Marker::Bullet, move_to("Done recently")),
            style(Marker::Numbered, Completion::Delete),
            style(Marker::Line, Completion::Delete),
        ];
        for src in fixtures {
            let items = parse(src).items;
            for to in &targets {
                let out = convert_to(src, to);
                for item in &items {
                    for part in item.text.split('\n') {
                        assert!(out.contains(part), "{to:?} lost {part:?} from {src:?}:\n{out}");
                    }
                }
            }
        }
    }

    #[test]
    fn parses_sections_items_and_continuations() {
        let d = parse(APP);
        assert_eq!(d.sections.iter().map(|s| (s.title.as_str(), s.line)).collect::<Vec<_>>(), [("Core", 4), ("Later", 15)]);
        assert!(d.editable);
        assert_eq!(d.items.len(), 5);
        let first = &d.items[0];
        assert_eq!((first.section.as_str(), first.line, first.end, first.depth, first.done), ("Core", 6, 8, 0, false));
        assert_eq!(first.text, "First item, which\nwraps onto a second line.");
        let child = &d.items[1];
        assert_eq!((child.text.as_str(), child.line, child.end, child.depth), ("Child", 8, 9, 1));
        assert!(d.items[2].done && d.items[3].done);
        assert_eq!(d.items[4].section, "Later");
        assert_eq!(d.open_count(), 3);
        assert!(d.items.iter().all(|i| i.id.len() == 12 && i.id.bytes().all(|b| b.is_ascii_hexdigit())));
        let ids: std::collections::HashSet<_> = d.items.iter().map(|i| &i.id).collect();
        assert_eq!(ids.len(), 5);
        let bullets = parse("## A\n- plain\n");
        assert!(bullets.editable, "bullet files are editable now");
        assert_eq!(bullets.style, style(Marker::Bullet, Completion::Delete));
    }

    #[test]
    fn tabs_count_as_four_columns() {
        let d = parse("- [ ] a\n\t- [ ] b\n");
        assert_eq!(d.items[1].depth, 2);
    }

    #[test]
    fn ids_survive_unrelated_edits_and_repeat_by_occurrence() {
        let mut d = parse(APP);
        let later = id(&d, "Later item");
        let child = id(&d, "Child");
        d.add("Core", "New thing").unwrap();
        d.set_done(&child, true).unwrap();
        assert_eq!(id(&d, "Later item"), later);
        assert_eq!(id(&d, "Child"), child);
        let twins = parse("## A\n- [ ] x\n- [ ] x\n");
        assert_ne!(twins.items[0].id, twins.items[1].id);
        assert_eq!(twins.items[0].id, parse("## A\n- [ ] x\n- [ ] x\n").items[0].id);
        assert_ne!(parse("## A\n- [ ] x\n").items[0].id, parse("## B\n- [ ] x\n").items[0].id);
    }

    #[test]
    fn set_done_changes_only_the_box() {
        let mut d = parse(APP);
        let before = d.lines.clone();
        let first = id(&d, "First item, which\nwraps onto a second line.");
        d.set_done(&first, true).unwrap();
        let mut expect = before.clone();
        expect[6] = "- [x] First item, which".into();
        assert_eq!(d.lines, expect);
        assert!(d.item(&first).unwrap().done);
        d.set_done(&first, false).unwrap();
        assert_eq!(d.lines, before);
    }

    #[test]
    fn set_text_keeps_marker_children_and_aligns_lines() {
        let mut d = parse(APP);
        let child = id(&d, "Child");
        d.set_text(&child, "Renamed\nover two lines").unwrap();
        assert_eq!(d.lines[8], "  - [ ] Renamed");
        assert_eq!(d.lines[9], "        over two lines");
        assert_eq!(d.lines[10], "  - [x] Done child");
        assert_eq!(d.item(&id(&d, "Renamed\nover two lines")).unwrap().text, "Renamed\nover two lines");
    }

    #[test]
    fn add_places_items_and_creates_sections() {
        let mut d = parse(APP);
        let new = d.add("Core", "Added").unwrap();
        assert_eq!(d.lines[11], "- [ ] Added");
        assert_eq!(d.item(&new).unwrap().section, "Core");

        d.add("Ideas", "Idea").unwrap();
        let n = d.lines.len();
        assert_eq!(d.lines[n - 3..].iter().map(String::as_str).collect::<Vec<_>>(), ["", "## Ideas", "- [ ] Idea"]);
        assert_eq!(d.sections.last().unwrap().title, "Ideas");

        let mut p = parse(APP);
        p.add("", "Top").unwrap();
        assert_eq!(p.lines[3], "- [ ] Top");
        assert_eq!(p.items[0].section, "");
    }

    #[test]
    fn add_on_top_creates_the_section_above_the_first() {
        let mut d = parse(CLOCK);
        let id = d.add_on_top("Inbox", "Idea: one\nwhy").unwrap();
        assert!(d.render().starts_with("# TODO\n\n## Inbox\n\n- Idea: one\n  why\n\n## Done recently\n"), "{}", d.render());
        assert_eq!(d.item(&id).unwrap().text, "Idea: one\nwhy");
        d.add_on_top("Inbox", "two").unwrap();
        assert!(d.render().contains("## Inbox\n\n- Idea: one\n  why\n- two\n\n## Done"), "{}", d.render());
        assert!(!d.item(&id).unwrap().done, "a capture is open, also in a move-to-section style");
    }

    #[test]
    fn add_child_goes_after_existing_children() {
        let mut d = parse(APP);
        let parent = id(&d, "First item, which\nwraps onto a second line.");
        let c = d.add_child(&parent, "Sub").unwrap();
        assert_eq!(d.lines[10], "  - [ ] Sub");
        assert_eq!(d.item(&c).unwrap().depth, 1);
    }

    #[test]
    fn remove_takes_children_along() {
        let mut d = parse(APP);
        let first = id(&d, "First item, which\nwraps onto a second line.");
        d.remove(&first).unwrap();
        assert!(d.item(&first).is_none());
        assert!(d.items.iter().all(|i| i.text != "Child" && i.text != "Done child"));
        assert_eq!(d.items.len(), 2);
        assert_eq!(d.lines[6], "- [x] Done item");
    }

    #[test]
    fn move_by_swaps_siblings_with_children() {
        let mut d = parse(APP);
        let first = id(&d, "First item, which\nwraps onto a second line.");
        d.move_by(&first, -1).unwrap();
        assert_eq!(d.lines, parse(APP).lines, "first sibling: nothing to do");
        d.move_by(&first, 1).unwrap();
        assert_eq!(d.lines[6], "- [x] Done item");
        assert_eq!(d.lines[7], "- [ ] First item, which");
        assert_eq!(d.lines[8], "      wraps onto a second line.");
        assert_eq!(d.lines[9], "  - [ ] Child");
        assert_eq!(d.lines[10], "  - [x] Done child");
        assert_eq!(d.lines[11], "");
        assert_eq!(d.items[0].text, "Done item");
        // A child moves among its own siblings only.
        let child = id(&d, "Child");
        d.move_by(&child, 1).unwrap();
        assert_eq!(d.items.iter().filter(|i| i.depth == 1).map(|i| i.text.as_str()).collect::<Vec<_>>(), ["Done child", "Child"]);
    }

    #[test]
    fn move_to_reindents_and_appends() {
        let mut d = parse(APP);
        let child = id(&d, "Child");
        d.move_to(&child, "Later").unwrap();
        let moved = d.items.iter().find(|i| i.text == "Child").unwrap();
        assert_eq!((moved.section.as_str(), moved.depth), ("Later", 0));
        assert_eq!(d.lines.last().unwrap(), "- [ ] Child");
        let child = id(&d, "Child");
        d.move_to(&child, "Brand new").unwrap();
        assert_eq!(d.sections.last().unwrap().title, "Brand new");
    }

    #[test]
    fn move_before_reorders_across_sections_and_depths() {
        let mut d = parse(APP);
        // Down within a section: Done item before First item.
        let (first, done) = (id(&d, "First item, which\nwraps onto a second line."), id(&d, "Done item"));
        d.move_before(&done, &first).unwrap();
        assert_eq!(&d.lines[6..11], ["- [x] Done item", "- [ ] First item, which", "      wraps onto a second line.", "  - [ ] Child", "  - [x] Done child"]);
        // Into another section, with its children.
        let (first, later) = (id(&d, "First item, which\nwraps onto a second line."), id(&d, "Later item"));
        d.move_before(&first, &later).unwrap();
        let tail: Vec<&str> = d.lines.iter().rev().take(5).rev().map(|s| s.as_str()).collect();
        assert_eq!(tail, ["- [ ] First item, which", "      wraps onto a second line.", "  - [ ] Child", "  - [x] Done child", "- [ ] Later item"]);
        // A child dropped on a top-level item becomes top level; onto its own child does nothing.
        let (child, later) = (id(&d, "Child"), id(&d, "Later item"));
        d.move_before(&child, &later).unwrap();
        assert_eq!(d.items.iter().find(|i| i.text == "Child").unwrap().depth, 0);
        let (first, done_child) = (id(&d, "First item, which\nwraps onto a second line."), id(&d, "Done child"));
        let before = d.lines.clone();
        d.move_before(&first, &done_child).unwrap();
        assert_eq!(d.lines, before);
    }

    #[test]
    fn clear_done_moves_top_level_done_items() {
        let mut d = parse(APP);
        assert_eq!(d.clear_done(), 1, "the done child stays with its parent");
        assert_eq!(d.sections.last().unwrap().title, "Done");
        assert_eq!(d.lines[d.lines.len() - 1], "- [x] Done item");
        assert_eq!(d.clear_done(), 0);
        assert!(d.items.iter().any(|i| i.text == "Done child"));
    }

    #[test]
    fn plain_bullets_stay_verbatim_in_a_checklist() {
        let text = "## A\n- plain\n- [ ] ok\n";
        let mut d = parse_with(text, &Style::CHECKLIST);
        let ok = id(&d, "ok");
        d.set_done(&ok, true).unwrap();
        let x = d.add("A", "x").unwrap();
        d.remove(&x).unwrap();
        assert_eq!(d.lines, ["## A", "- plain", "- [x] ok"]);
        assert_eq!(d.clear_done(), 1);
        assert_eq!(parse("- [ ] a\n").set_done("nope", true), Err(TodoError::NotFound("nope".into())));
    }

    #[test]
    fn conversion_handles_each_style() {
        let md = "# T\n\n## Sec\n\n* first\n  continued line\n1. numbered\n- ~~struck~~\n- shipped (done)\n- [ ] already\n\nProse stays.\n\n```\n- code\n```\n";
        let c = convert(md);
        assert_eq!(
            c,
            "# T\n\n## Sec\n\n- [ ] first\n  continued line\n- [ ] numbered\n- [x] ~~struck~~\n- [x] shipped (done)\n- [ ] already\n\nProse stays.\n\n```\n- code\n```\n"
        );
        let d = parse(&c);
        assert!(d.editable);
        assert_eq!(d.items.len(), 5);
        assert_eq!(d.open_count(), 3);
    }

    #[test]
    fn plain_text_lines_become_items_and_keep_crlf() {
        let c = convert("Search all\r\n  nested line\r\n\r\nOffline mode\r\n");
        assert_eq!(c, "- [ ] Search all\r\n  - [ ] nested line\r\n\r\n- [ ] Offline mode\r\n");
        let d = parse(&c);
        assert!(d.editable);
        assert_eq!(d.items.len(), 3);
    }

    #[test]
    fn save_checks_for_changes_and_creates_missing_files() {
        let dir = tmp("save");
        let f = dir.join("TODO.md");
        save(&f, "", "- [ ] a\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "- [ ] a\n");
        assert!(matches!(save(&f, "", "x"), Err(SaveError::Changed)));
        assert!(!dir.join("TODO.md.tmp").exists());
        save(&f, "- [ ] a\n", "- [x] a\n").unwrap();
        assert!(matches!(save(&f, "- [ ] a\n", "y"), Err(SaveError::Changed)));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "- [x] a\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backup_and_find_file() {
        let dir = tmp("backup");
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/TODO.md"), "- [ ] a\n").unwrap();
        assert_eq!(find_file(&dir), Some(dir.join("docs/TODO.md")));
        let data = dir.join("data");
        let copy = backup(&data, &dir.join("docs/TODO.md")).unwrap();
        assert!(copy.starts_with(data.join("backups")));
        let name = copy.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with("TODO.md") && !name.contains(['\\', '/', ':']), "{name}");
        assert_eq!(std::fs::read_to_string(copy).unwrap(), "- [ ] a\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
