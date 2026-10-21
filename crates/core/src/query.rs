//! The TODOs query (after YouTrack's search): words that must all appear in an item, and `key:value` terms.
//! `-` before a word or term negates it; `"…"` keeps spaces together (`in:"Done recently"`). Keys:
//! `project:` (name contains), `in:` (section or milestone contains), `is:open` / `is:done`, `kind:<word>` (the
//! text starts with `<word>:`, as captures write `Idea:` and `Bug:`), `source:todo` / `source:roadmap` and
//! `has:issue` (a `(#n)` link). Values compare case-insensitively; an unknown `key:value` is a plain word.

/// What a query looks at in one item.
pub struct Item<'a> {
    pub project: &'a str,
    pub section: &'a str,
    pub text: &'a str,
    pub done: bool,
    pub roadmap: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Term {
    Word(String),
    Project(String),
    In(String),
    Done(bool),
    Kind(String),
    Roadmap(bool),
    Issue,
}

/// A parsed query: terms that must all hold (negated ones must not).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query(Vec<(bool, Term)>);

/// The keys, for completion.
pub const KEYS: [&str; 6] = ["project:", "in:", "is:", "kind:", "source:", "has:"];

impl Query {
    pub fn parse(s: &str) -> Query {
        Query(tokens(s).into_iter().filter_map(|t| term(&t)).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the query says `is:open` or `is:done` itself.
    pub fn has_status(&self) -> bool {
        self.0.iter().any(|(_, t)| matches!(t, Term::Done(_)))
    }

    pub fn matches(&self, it: &Item) -> bool {
        let text = it.text.to_lowercase();
        self.0.iter().all(|(neg, t)| {
            let hit = match t {
                Term::Word(w) => text.contains(w.as_str()),
                Term::Project(p) => it.project.to_lowercase().contains(p.as_str()),
                Term::In(s) => it.section.to_lowercase().contains(s.as_str()),
                Term::Done(d) => it.done == *d,
                Term::Kind(k) => text.trim_start().strip_prefix(k.as_str()).is_some_and(|rest| rest.starts_with(':')),
                Term::Roadmap(r) => it.roadmap == *r,
                Term::Issue => crate::github::issue_ref(it.text).is_some(),
            };
            hit != *neg
        })
    }
}

/// Words split on whitespace outside quotes; quotes are dropped.
fn tokens(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn term(token: &str) -> Option<(bool, Term)> {
    let (neg, t) = match token.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, token),
    };
    let lower = t.to_lowercase();
    let term = match lower.split_once(':') {
        Some(("project", v)) if !v.is_empty() => Term::Project(v.into()),
        Some(("in", v)) if !v.is_empty() => Term::In(v.into()),
        Some(("is", "open")) => Term::Done(false),
        Some(("is", "done")) => Term::Done(true),
        Some(("kind", v)) if !v.is_empty() => Term::Kind(v.into()),
        Some(("source", "todo")) => Term::Roadmap(false),
        Some(("source", "roadmap")) => Term::Roadmap(true),
        Some(("has", "issue")) => Term::Issue,
        // A key still being typed (`project:`) restricts nothing yet.
        Some((k, "")) if KEYS.contains(&format!("{k}:").as_str()) => return None,
        _ => Term::Word(lower),
    };
    Some((neg, term))
}

/// Completions for the last word of `text`: whole queries with that word completed. A key prefix (`pro`) offers
/// keys; `key:partial` offers values (`projects` and `sections` for `project:` and `in:`, quoted when they hold
/// spaces). At most 8.
pub fn suggest(text: &str, projects: &[String], sections: &[String]) -> Vec<String> {
    let start = text.rfind(char::is_whitespace).map_or(0, |i| i + 1);
    let (head, last) = text.split_at(start);
    let (neg, last) = match last.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", last),
    };
    if last.is_empty() {
        return Vec::new();
    }
    let lower = last.to_lowercase();
    let quote = |v: &str| if v.contains(' ') { format!("\"{v}\"") } else { v.to_string() };
    let values: Vec<String> = match lower.split_once(':') {
        None => return KEYS.iter().filter(|k| k.starts_with(&lower) && **k != lower).map(|k| format!("{head}{neg}{k}")).take(8).collect(),
        Some(("project", _)) => projects.to_vec(),
        Some(("in", _)) => sections.to_vec(),
        Some(("is", _)) => vec!["open".into(), "done".into()],
        Some(("kind", _)) => vec!["idea".into(), "bug".into()],
        Some(("source", _)) => vec!["todo".into(), "roadmap".into()],
        Some(("has", _)) => vec!["issue".into()],
        Some(_) => return Vec::new(),
    };
    let (key, partial) = lower.split_once(':').unwrap_or_default();
    let partial = partial.trim_start_matches('"');
    let mut seen = Vec::new();
    for v in values {
        if v.to_lowercase().contains(partial) && !v.eq_ignore_ascii_case(partial) && !seen.contains(&v) {
            seen.push(v);
        }
    }
    seen.iter().take(8).map(|v| format!("{head}{neg}{key}:{} ", quote(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item<'a>(project: &'a str, section: &'a str, text: &'a str, done: bool, roadmap: bool) -> Item<'a> {
        Item { project, section, text, done, roadmap }
    }

    #[test]
    fn words_and_terms_all_have_to_hold() {
        let bug = item("trailmix-web", "Inbox", "Bug: crash on start (#12)", false, false);
        let idea = item("ProjectArchitect", "Done recently", "Idea: dark board", true, true);
        let q = |s: &str| (Query::parse(s).matches(&bug), Query::parse(s).matches(&idea));
        assert_eq!(q(""), (true, true));
        assert_eq!(q("CRASH start"), (true, false), "every word, any case");
        assert_eq!(q("project:trailmix"), (true, false));
        assert_eq!(q("in:\"done recently\""), (false, true), "quotes keep spaces");
        assert_eq!(q("is:done"), (false, true));
        assert_eq!(q("kind:bug"), (true, false));
        assert_eq!(q("source:roadmap"), (false, true));
        assert_eq!(q("has:issue"), (true, false));
        assert_eq!(q("-has:issue -kind:bug"), (false, true), "negation");
        assert_eq!(q("project:"), (true, true), "a key still being typed restricts nothing");
        assert!(Query::parse("dark is:open").has_status() && !Query::parse("dark").has_status());
    }

    #[test]
    fn completes_keys_and_values() {
        let projects = vec!["trailmix".to_string(), "trailmix-web".into(), "pixel-forge".into()];
        let sections = vec!["Done recently".to_string(), "Inbox".into()];
        assert_eq!(suggest("crash pro", &projects, &sections), ["crash project:"]);
        assert_eq!(suggest("project:tra", &projects, &sections), ["project:trailmix ", "project:trailmix-web "]);
        assert_eq!(suggest("-in:do", &projects, &sections), ["-in:\"Done recently\" "]);
        assert_eq!(suggest("is:", &projects, &sections), ["is:open ", "is:done "]);
        assert!(suggest("crash ", &projects, &sections).is_empty() && suggest("x:y", &projects, &sections).is_empty());
    }
}
