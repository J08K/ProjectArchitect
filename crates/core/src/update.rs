//! Updating the app from its GitHub releases (D-49) through the `gh` CLI, which holds the login (the repository is
//! private). A `v*` tag's release carries the MSI and the RPM (`.github/workflows/release.yml`). Parsing is kept
//! apart from running `gh`, so the tests need neither `gh` nor the network.

use crate::git::{GitError, exec};
use std::path::{Path, PathBuf};

/// The repository the releases come from.
pub const REPO: &str = "J08K/ProjectArchitect";

#[derive(Clone, Debug, PartialEq)]
pub struct Latest {
    /// `v1.2.3`.
    pub tag: String,
    /// File names of the attached installers.
    pub assets: Vec<String>,
}

impl Latest {
    /// The version without the tag prefix (`1.2.3`).
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }

    /// The first attached file ending in `ext` (`.msi`, `.rpm`).
    pub fn asset(&self, ext: &str) -> Option<&str> {
        self.assets.iter().map(String::as_str).find(|n| n.ends_with(ext))
    }

    /// Newer than the running build.
    pub fn is_newer(&self) -> bool {
        newer(&self.tag, env!("CARGO_PKG_VERSION"))
    }
}

/// The newest release (`gh release view` skips drafts and pre-releases).
pub fn latest_release() -> Result<Latest, GitError> {
    let out = exec("gh", None, &["release", "view", "--repo", REPO, "--json", "tagName,assets"], None, None).map_err(gh_hint)?;
    parse_release(&out).ok_or_else(|| GitError(format!("unexpected gh output: {out}")))
}

/// Download one asset of `tag` into `dir` (replacing an earlier download); returns its path.
pub fn download(tag: &str, asset: &str, dir: &Path) -> Result<PathBuf, GitError> {
    std::fs::create_dir_all(dir).map_err(|e| GitError(format!("{}: {e}", dir.display())))?;
    let d = dir.display().to_string();
    exec("gh", None, &["release", "download", tag, "--repo", REPO, "--pattern", asset, "--dir", &d, "--clobber"], None, None).map_err(gh_hint)?;
    Ok(dir.join(asset))
}

/// Run the MSI per-user and wait (`/passive`: a progress bar, no questions; never a reboot). Windows only.
pub fn install_msi(msi: &Path) -> Result<(), GitError> {
    let status = crate::cmd("msiexec")
        .arg("/i")
        .arg(msi)
        .args(["/passive", "/norestart"])
        .status()
        .map_err(|e| GitError(format!("msiexec: {e}")))?;
    match status.code() {
        // 3010: done, a restart would finish it (not for a per-user install, but not a failure either).
        Some(0 | 3010) => Ok(()),
        Some(1602) => Err(GitError("The installation was cancelled".into())),
        Some(1618) => Err(GitError("Another installation is in progress; try again when it is done".into())),
        code => Err(GitError(format!("msiexec failed (exit code {code:?})"))),
    }
}

/// Turn gh's start failure and login message into something the Settings row can show as it is.
fn gh_hint(e: GitError) -> GitError {
    let t = &e.0;
    if t.starts_with("gh: ") {
        GitError(format!("The GitHub CLI (gh) is not installed or not on PATH ({t})"))
    } else if t.contains("gh auth login") {
        GitError("gh is not signed in: run `gh auth login`".into())
    } else if t.contains("release not found") {
        GitError(format!("{REPO} has no release yet"))
    } else {
        e
    }
}

fn parse_release(json: &str) -> Option<Latest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v["tagName"].as_str()?.to_string();
    let assets = v["assets"].as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    Some(Latest { tag, assets })
}

/// `latest` (a tag, `v` prefix optional) is a higher version than `current`.
// ponytail: x.y.z numbers only; a pre-release or build suffix (`-rc1`, `+b`) is ignored, so `1.0.0-rc1` = `1.0.0`.
pub fn newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<[u64; 3]> {
        let v = v.trim().trim_start_matches('v');
        let core = v.split(['-', '+']).next()?;
        let mut it = core.split('.').map(|n| n.parse::<u64>().ok());
        Some([it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)])
    }
    matches!((parts(latest), parts(current)), (Some(l), Some(c)) if l > c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(newer("v0.2.0", "0.1.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("v1.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("v0.1.0", "0.2.0"));
        assert!(!newer("v1.0.0-rc1", "1.0.0"));
        assert!(!newer("nightly", "0.1.0"));
    }

    #[test]
    fn release_json() {
        let json = r#"{"assets":[{"name":"ProjectArchitect-0.2.0-x64.msi","size":1},{"name":"projectarchitect-0.2.0-1.x86_64.rpm","size":2}],"tagName":"v0.2.0"}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version(), "0.2.0");
        assert_eq!(r.asset(".msi"), Some("ProjectArchitect-0.2.0-x64.msi"));
        assert_eq!(r.asset(".rpm"), Some("projectarchitect-0.2.0-1.x86_64.rpm"));
        assert_eq!(parse_release(r#"{"assets":[],"tagName":"v1.0.0"}"#).unwrap().asset(".msi"), None);
        assert_eq!(parse_release("not json"), None);
    }

    #[test]
    fn gh_errors() {
        assert!(gh_hint(GitError("gh: program not found".into())).0.contains("not installed"));
        assert!(gh_hint(GitError("To get started with GitHub CLI, please run:  gh auth login".into())).0.contains("not signed in"));
        assert!(gh_hint(GitError("release not found".into())).0.contains("no release"));
    }
}
