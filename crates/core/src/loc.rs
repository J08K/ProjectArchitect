//! Lines of code per language, counted by tokei (it honours .gitignore and skips hidden folders, so `target/`
//! and `node_modules/` are left out in repositories that ignore them). Read-only; can take a second on big trees,
//! so callers run it in the background.

use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocRow {
    pub language: String,
    pub files: usize,
    pub code: usize,
    pub comments: usize,
    pub blanks: usize,
}

impl LocRow {
    pub fn lines(&self) -> usize {
        self.code + self.comments + self.blanks
    }
}

/// One row per language (most code first), embedded languages (code blocks in Markdown, ...) folded into their host.
pub fn count(root: &Path) -> Vec<LocRow> {
    let mut langs = tokei::Languages::new();
    langs.get_statistics(&[root], &[], &tokei::Config::default());
    let mut rows: Vec<LocRow> = langs
        .iter()
        .map(|(t, l)| (t, l.summarise()))
        .filter(|(_, l)| !l.reports.is_empty())
        .map(|(t, l)| LocRow { language: t.name().to_string(), files: l.reports.len(), code: l.code, comments: l.comments, blanks: l.blanks })
        .collect();
    rows.sort_by(|a, b| b.code.cmp(&a.code).then_with(|| a.language.cmp(&b.language)));
    rows
}

/// The sum of all rows (language "Total").
pub fn total(rows: &[LocRow]) -> LocRow {
    rows.iter().fold(LocRow { language: "Total".into(), ..Default::default() }, |mut t, r| {
        t.files += r.files;
        t.code += r.code;
        t.comments += r.comments;
        t.blanks += r.blanks;
        t
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_rust_and_python() {
        let dir = std::env::temp_dir().join(format!("pa-loc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "// comment\nfn main() {\n\n    println!(\"x\");\n}\n").unwrap();
        std::fs::write(dir.join("b.py"), "x = 1\n").unwrap();
        let rows = super::count(&dir);
        let rust = rows.iter().find(|r| r.language == "Rust").expect("rust row");
        assert_eq!((rust.files, rust.code, rust.comments, rust.blanks), (1, 3, 1, 1));
        assert_eq!(super::total(&rows).code, 4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
