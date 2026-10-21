//! Tasks (SPEC § 7.7, D-8): named commands per project, kept in the store, suggested from manifests, and run with
//! live output. A run is a shell command in a checkout folder; stdout and stderr are read on threads into one log,
//! and stopping kills the whole process tree (a dev server started through `npm` or `cmd` included).

use crate::db::{Result, Store};
use crate::cmd;
use rusqlite::params;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How many runs per project the GUI keeps in memory (running ones are never dropped).
pub const KEEP_RUNS: usize = 5;
/// ponytail: a run keeps its last 5,000 output lines; older ones are counted in `Log::dropped`.
const MAX_LINES: usize = 5_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    /// One shell command line (`cmd /S /C` on Windows, `sh -c` elsewhere).
    pub command: String,
    /// Folder relative to the checkout; `""` = the checkout itself.
    pub cwd: String,
    /// `KEY=VALUE` lines.
    pub env: String,
}

impl Store {
    /// A project's tasks in the order they were added.
    pub fn tasks(&self, project_id: i64) -> Result<Vec<Task>> {
        self.c
            .prepare("SELECT id, project_id, name, command, cwd, env FROM tasks WHERE project_id = ?1 ORDER BY id")?
            .query_map([project_id], |r| {
                Ok(Task { id: r.get(0)?, project_id: r.get(1)?, name: r.get(2)?, command: r.get(3)?, cwd: r.get(4)?, env: r.get(5)? })
            })?
            .collect()
    }

    pub fn add_task(&self, project_id: i64, name: &str, command: &str, cwd: &str, env: &str) -> Result<i64> {
        self.c.execute(
            "INSERT INTO tasks (project_id, name, command, cwd, env) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, name, command, cwd, env],
        )?;
        Ok(self.c.last_insert_rowid())
    }

    /// Save name, command, folder and environment.
    pub fn update_task(&self, t: &Task) -> Result<()> {
        self.c
            .execute("UPDATE tasks SET name = ?2, command = ?3, cwd = ?4, env = ?5 WHERE id = ?1", params![t.id, t.name, t.command, t.cwd, t.env])
            .map(|_| ())
    }

    pub fn delete_task(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM tasks WHERE id = ?1", [id]).map(|_| ())
    }
}

/// Commands the manifests in `root` suggest, as (name, command): `cargo build/test/run` for `Cargo.toml`, every
/// `package.json` script through the package manager its lock file names, `docker compose up/down` for a compose file.
pub fn suggestions(root: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    if root.join("Cargo.toml").is_file() {
        for verb in ["build", "test", "run"] {
            out.push((format!("cargo {verb}"), format!("cargo {verb}")));
        }
    }
    if let Some(json) = std::fs::read_to_string(root.join("package.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        let pm = [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lockb", "bun"), ("bun.lock", "bun")]
            .into_iter()
            .find(|(lock, _)| root.join(lock).is_file())
            .map_or("npm", |(_, pm)| pm);
        if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
            for name in scripts.keys() {
                out.push((name.clone(), format!("{pm} run {name}")));
            }
        }
    }
    if ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"].iter().any(|f| root.join(f).is_file()) {
        out.push(("compose up".into(), "docker compose up".into()));
        out.push(("compose down".into(), "docker compose down".into()));
    }
    out
}

/// `KEY=VALUE` lines; blank lines and lines without `=` are skipped.
pub fn parse_env(env: &str) -> Vec<(String, String)> {
    env.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.to_string())).filter(|(k, _)| !k.is_empty()).collect()
}

/// A task's folder: the checkout, or a relative folder inside it (no root, no `..`).
fn task_dir(root: &Path, cwd: &str) -> io::Result<std::path::PathBuf> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return Ok(root.to_path_buf());
    }
    if !Path::new(cwd).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{cwd:?} is not a folder inside the checkout")));
    }
    Ok(root.join(cwd))
}

/// The output of a run so far.
#[derive(Debug, Default)]
pub struct Log {
    /// (from stderr, line), oldest first.
    pub lines: VecDeque<(bool, String)>,
    /// Lines dropped from the front to keep the log at its cap.
    pub dropped: usize,
    /// `None` while running; then the exit code (`None` inside when there is none, e.g. killed).
    pub exit: Option<Option<i32>>,
    /// Both pipes are closed: the log holds all the output.
    pub complete: bool,
}

impl Log {
    fn push(&mut self, err: bool, line: String) {
        if self.lines.len() >= MAX_LINES {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back((err, line));
    }

    /// The lines as one text.
    pub fn text(&self) -> String {
        self.lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// A started task. Dropping a running one stops it, so nothing outlives the app (or an MCP call).
pub struct Run {
    pub task_id: i64,
    pub name: String,
    pub command: String,
    pub started: Instant,
    pid: u32,
    log: Arc<Mutex<Log>>,
    stopped: Arc<AtomicBool>,
}

impl Run {
    /// Start `task` in `root` (its checkout). `notify` is called whenever output arrives or the run ends (the GUI
    /// passes a repaint request).
    pub fn start(task: &Task, root: &Path, notify: impl Fn() + Send + Sync + 'static) -> io::Result<Run> {
        let dir = task_dir(root, &task.cwd)?;
        if !dir.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} is not a folder", dir.display())));
        }
        let mut c = shell(&task.command);
        c.current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in parse_env(&task.env) {
            c.env(k, v);
        }
        let mut child = c.spawn()?;
        let pid = child.id();
        let log = Arc::new(Mutex::new(Log::default()));
        let notify: Notify = Arc::new(notify);
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            readers.push(reader(out, false, &log, &notify));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(reader(err, true, &log, &notify));
        }
        let (wait_log, wait_notify) = (log.clone(), notify.clone());
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code());
            // The exit is known once the process ends, even when a grandchild still holds the pipes open.
            lock(&wait_log).exit = Some(code);
            wait_notify();
            for r in readers {
                let _ = r.join();
            }
            lock(&wait_log).complete = true;
            wait_notify();
        });
        Ok(Run { task_id: task.id, name: task.name.clone(), command: task.command.clone(), started: Instant::now(), pid, log, stopped: Arc::default() })
    }

    pub fn running(&self) -> bool {
        lock(&self.log).exit.is_none()
    }

    /// Stopped with [`Run::stop`] (its exit code then says little).
    pub fn was_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Kill the process and everything it started.
    pub fn stop(&self) {
        if !self.running() {
            return;
        }
        self.stopped.store(true, Ordering::Relaxed);
        let pid = self.pid.to_string();
        #[cfg(windows)]
        let _ = cmd("taskkill").args(["/T", "/F", "/PID", &pid]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        #[cfg(not(windows))]
        let _ = cmd("kill").args(["-TERM", "--", &format!("-{pid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }

    /// Wait until the run ends, at most `timeout`, and then up to half a second more for the rest of its output
    /// (a grandchild may keep the pipes open). Returns whether it ended.
    pub fn wait(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        while self.running() {
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let grace = Instant::now() + Duration::from_millis(500);
        while !self.log().complete && Instant::now() < grace {
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// The output so far (hold the guard briefly: the reader threads wait for it).
    pub fn log(&self) -> MutexGuard<'_, Log> {
        lock(&self.log)
    }

    /// Time since start.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.stop();
    }
}

fn lock(log: &Mutex<Log>) -> MutexGuard<'_, Log> {
    log.lock().unwrap_or_else(|e| e.into_inner())
}

/// The shell that runs a task's command line. On Windows `cmd /S /C "<line>"` keeps the line's own quotes; elsewhere
/// the command gets its own process group so [`Run::stop`] reaches its children.
fn shell(line: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = cmd("cmd");
        c.raw_arg(format!("/S /C \"{line}\""));
        c
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        let mut c = cmd("sh");
        c.arg("-c").arg(line).process_group(0);
        c
    }
}

type Notify = Arc<dyn Fn() + Send + Sync>;

/// A thread that reads lines from a pipe into the log; invalid UTF-8 (a console code page) is replaced, `\r` endings
/// dropped.
fn reader(pipe: impl Read + Send + 'static, err: bool, log: &Arc<Mutex<Log>>, notify: &Notify) -> std::thread::JoinHandle<()> {
    let (log, notify) = (log.clone(), notify.clone());
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        while matches!(reader.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
            let line = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
            lock(&log).push(err, line);
            notify();
            buf.clear();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pa-tasks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn task(command: &str) -> Task {
        Task { id: 1, project_id: 1, name: "t".into(), command: command.into(), cwd: String::new(), env: String::new() }
    }

    #[test]
    fn store_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let p = s.add_project("p", None, None, &["x".into()]).unwrap();
        let id = s.add_task(p, "test", "cargo test", "", "").unwrap();
        let mut t = s.tasks(p).unwrap().remove(0);
        assert_eq!((t.id, t.command.as_str()), (id, "cargo test"));
        t.env = "RUST_LOG=debug".into();
        s.update_task(&t).unwrap();
        assert_eq!(s.tasks(p).unwrap()[0].env, "RUST_LOG=debug");
        s.delete_task(id).unwrap();
        assert!(s.tasks(p).unwrap().is_empty());
        s.add_task(p, "x", "y", "", "").unwrap();
        s.remove_project(p).unwrap();
        assert!(s.tasks(p).unwrap().is_empty(), "tasks cascade");
    }

    #[test]
    fn suggestions_from_manifests() {
        let d = dir("suggest");
        assert!(suggestions(&d).is_empty());
        std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(d.join("package.json"), r#"{"scripts": {"dev": "vite", "build": "vite build"}}"#).unwrap();
        std::fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(d.join("compose.yaml"), "services: {}\n").unwrap();
        let commands: Vec<String> = suggestions(&d).into_iter().map(|(_, c)| c).collect();
        // Scripts in file order (serde_json `preserve_order`).
        assert_eq!(
            commands,
            ["cargo build", "cargo test", "cargo run", "pnpm run dev", "pnpm run build", "docker compose up", "docker compose down"]
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn env_lines_and_folders() {
        assert_eq!(parse_env("A=1\n\nB = x=y\nnot a pair\n=v"), [("A".to_string(), "1".to_string()), ("B".into(), " x=y".into())]);
        let root = Path::new("/r");
        assert!(task_dir(root, "../x").is_err());
        assert!(task_dir(root, "/abs").is_err());
        assert_eq!(task_dir(root, " apps/pa ").unwrap(), root.join("apps/pa"));
    }

    #[test]
    fn runs_with_output_env_folder_and_exit_code() {
        let d = dir("run");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub").join("marker.txt"), "").unwrap();
        let mut t = task(if cfg!(windows) { "echo %GREETING% && dir /b && echo oops 1>&2 && exit 3" } else { "echo $GREETING && ls && echo oops >&2 && exit 3" });
        t.cwd = "sub".into();
        t.env = "GREETING=hello".into();
        let run = Run::start(&t, &d, || {}).unwrap();
        assert!(run.wait(Duration::from_secs(20)));
        let log = run.log();
        assert!(log.complete);
        assert_eq!(log.exit, Some(Some(3)));
        let text = log.text();
        assert!(text.contains("hello") && text.contains("marker.txt"), "{text}");
        assert!(log.lines.iter().any(|(err, l)| *err && l.trim() == "oops"), "{:?}", log.lines);
        drop(log);
        assert!(!run.was_stopped());
        assert!(Run::start(&task("echo"), &d.join("missing"), || {}).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn stop_kills_a_long_running_task() {
        let d = dir("stop");
        let run = Run::start(&task(if cfg!(windows) { "ping -n 30 127.0.0.1" } else { "sleep 30" }), &d, || {}).unwrap();
        assert!(!run.wait(Duration::from_millis(300)), "still running");
        run.stop();
        assert!(run.wait(Duration::from_secs(10)), "stopped");
        assert!(run.was_stopped());
        let _ = std::fs::remove_dir_all(d);
    }
}
