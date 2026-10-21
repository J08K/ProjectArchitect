//! Code TODO comments (SPEC § 7.3, D-14): keyword matches on the lines of tracked source files. Read-only.
//! ponytail: keyword match on lines, not a comment parser; a string that contains `// TODO` counts too.

use crate::info;
use std::path::Path;

/// Stop after this many results.
pub const MAX_RESULTS: usize = 5_000;
/// Larger files are skipped (generated or minified code).
const MAX_FILE_BYTES: u64 = 1_048_576;
const KEYWORDS: [&str; 4] = ["TODO", "FIXME", "HACK", "XXX"];
/// A keyword counts only after one of these on its line.
const MARKERS: [&str; 7] = ["//", "#", "--", "/*", "*", "<!--", ";"];

#[derive(Clone, Debug, PartialEq)]
pub struct CodeTodo {
    /// Relative to the checkout, `/` separators.
    pub file: String,
    /// 1-based.
    pub line: usize,
    pub keyword: String,
    /// The rest of the line after the keyword, trimmed of `:` and spaces, at most 200 characters.
    pub text: String,
}

/// Scan the tracked source files of a checkout.
pub fn scan(root: &Path) -> Vec<CodeTodo> {
    scan_files(root, &info::files(root))
}

/// [`scan`] over a file list the caller already has (the info collector reuses its own).
pub fn scan_files(root: &Path, files: &[String]) -> Vec<CodeTodo> {
    let mut out = Vec::new();
    for file in files {
        if info::language(file).is_none() {
            continue;
        }
        let path = root.join(file);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        // Not UTF-8 (binary or another encoding) reads as an error and is skipped.
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let rel = file.replace('\\', "/");
        for (i, line) in text.lines().enumerate() {
            if let Some((keyword, text)) = find(line) {
                out.push(CodeTodo { file: rel.clone(), line: i + 1, keyword: keyword.to_string(), text });
                if out.len() >= MAX_RESULTS {
                    return out;
                }
            }
        }
    }
    out
}

/// The first keyword on the line that is a whole word, after a comment marker, with its trailing text.
fn find(line: &str) -> Option<(&'static str, String)> {
    KEYWORDS
        .iter()
        .filter_map(|&kw| {
            line.match_indices(kw)
                .find(|&(at, _)| whole_word(line, at, kw.len()) && MARKERS.iter().any(|m| line[..at].contains(m)))
                .map(|(at, _)| (at, kw))
        })
        .min_by_key(|&(at, _)| at)
        .map(|(at, kw)| (kw, text_after(line, at + kw.len())))
}

fn whole_word(line: &str, at: usize, len: usize) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let before = line[..at].chars().next_back().is_none_or(|c| !is_word(c));
    let after = line[at + len..].chars().next().is_none_or(|c| !is_word(c));
    before && after
}

fn text_after(line: &str, from: usize) -> String {
    line[from..].trim_matches(|c: char| c == ':' || c.is_whitespace()).chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_in_comments_match() {
        let hits = [
            ("// TODO: fix parsing", "TODO", "fix parsing"),
            ("    # FIXME later", "FIXME", "later"),
            ("let x = 1; // HACK workaround", "HACK", "workaround"),
            ("/* XXX */", "XXX", "*/"),
            ("-- TODO sql", "TODO", "sql"),
            ("<!-- TODO -->", "TODO", "-->"),
            ("x = 2 * y  # TODO", "TODO", ""),
        ];
        for (line, kw, text) in hits {
            assert_eq!(find(line), Some((kw, text.to_string())), "{line}");
        }
    }

    #[test]
    fn non_comments_and_partial_words_do_not_match() {
        for line in ["let label = \"TODO\";", "const TODO_LIMIT: u32 = 3;", "// todo lowercase", "// XXXX", "fn todos() {}", "let x = TODO;"] {
            assert_eq!(find(line), None, "{line}");
        }
    }

    #[test]
    fn scans_source_files_only() {
        let dir = std::env::temp_dir().join(format!("pa-codetodo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    // TODO: wire it up\n    let s = \"TODO\";\n}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "// TODO not a language\n").unwrap();
        std::fs::write(dir.join("bin.py"), [0xff, 0xfe, b'#', b' ', b'T', b'O', b'D', b'O']).unwrap();
        let found = scan(&dir);
        assert_eq!(found, [CodeTodo { file: "src/main.rs".into(), line: 2, keyword: "TODO".into(), text: "wire it up".into() }]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
