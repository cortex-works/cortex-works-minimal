//! # Synchronous Shell Executor — CortexACT
//!
//! Runs a shell command **synchronously** with a hard timeout.
//! Intended for short, fast commands (`git diff`, `cargo check`, `ls -la`).
//! Not intended for long-running watch mode or background servers.
//!
//! ## Cross-platform shell
//! - Unix  (macOS / Linux): `sh -c <command>`
//! - Windows             : `cmd /C <command>`
//!
//! ## Timeout design
//!
//! The child process is spawned with piped stdout/stderr.
//! A monitoring thread drains output and waits; the main thread cancels via
//! `recv_timeout`.  On expiry the child is force-killed:
//! - Unix    : `kill -9 <pid>`
//! - Windows : `taskkill /PID <pid> /F`
//! No `libc` dependency is required — only stdlib + one extra process spawn.
//!
//! ## PATH augmentation
//! The MCP server may be started by an IDE with a reduced `PATH` that omits
//! user-local tool directories.  `run_sync` prepends the following directories
//! when they exist and are not already in `PATH`:
//!
//! **Unix (macOS / Linux)**:  `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin`
//!
//! **Windows**: `%USERPROFILE%\.cargo\bin`, `%USERPROFILE%\AppData\Roaming\npm`

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Result returned when the command completes (or times out).
#[derive(Debug, Serialize)]
pub struct ShellResult {
    /// Combined command that was run.
    pub command: String,
    /// Standard output, UTF-8.
    pub stdout: String,
    /// Standard error, UTF-8.
    pub stderr: String,
    /// Exit code (None if timed out or killed before exit).
    pub exit_code: Option<i32>,
    /// Whether the timeout fired before the command finished.
    pub timed_out: bool,
    /// Wall-clock execution time in milliseconds.
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct GroupMap {
    file: Option<usize>,
    line: Option<usize>,
    severity: Option<usize>,
    message: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct MatcherDef {
    pattern: String,
    groups: GroupMap,
}

#[derive(Debug, Clone)]
struct CompiledMatcher {
    regex: Regex,
    groups: GroupMap,
}

static MATCHERS: OnceLock<Result<HashMap<String, CompiledMatcher>, String>> = OnceLock::new();

fn load_matchers() -> Result<HashMap<String, CompiledMatcher>, String> {
    let raw = include_str!("../matchers.json");
    let parsed: HashMap<String, MatcherDef> =
        serde_json::from_str(raw).map_err(|e| format!("invalid matchers.json: {e}"))?;
    let mut out = HashMap::new();
    for (name, def) in parsed {
        let regex = Regex::new(&def.pattern)
            .map_err(|e| format!("invalid regex for matcher '{name}': {e}"))?;
        out.insert(
            name.to_lowercase(),
            CompiledMatcher {
                regex,
                groups: def.groups,
            },
        );
    }
    Ok(out)
}

fn get_matcher(name: &str) -> Option<CompiledMatcher> {
    let key = name.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    match MATCHERS.get_or_init(load_matchers) {
        Ok(map) => map.get(&key).cloned(),
        Err(_) => None,
    }
}

fn strip_ansi(input: &str) -> String {
    let clean = strip_ansi_escapes::strip(input.as_bytes());
    String::from_utf8_lossy(&clean).into_owned()
}

fn combine_output_stripped(stdout: &str, stderr: &str) -> String {
    let out = strip_ansi(stdout);
    let err = strip_ansi(stderr);
    match (out.trim().is_empty(), err.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => out,
        (true, false) => err,
        (false, false) => format!("{}\n{}", out, err),
    }
}

fn capture_text(caps: &regex::Captures<'_>, idx: Option<usize>) -> Option<String> {
    idx.and_then(|i| caps.get(i).map(|m| m.as_str().trim().to_string()))
        .filter(|s| !s.is_empty())
}

fn extract_problems(text: &str, matcher: &CompiledMatcher) -> Vec<Value> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(caps) = matcher.regex.captures(line) {
            let message = caps
                .get(matcher.groups.message)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if message.is_empty() {
                continue;
            }

            let mut obj = Map::new();
            if let Some(file) = capture_text(&caps, matcher.groups.file) {
                obj.insert("file".to_string(), Value::String(file));
            }
            if let Some(line_str) = capture_text(&caps, matcher.groups.line)
                && let Ok(line_num) = line_str.parse::<u64>()
            {
                obj.insert("line".to_string(), Value::Number(line_num.into()));
            }
            if let Some(sev) = capture_text(&caps, matcher.groups.severity) {
                obj.insert("severity".to_string(), Value::String(sev));
            }
            obj.insert("error".to_string(), Value::String(message));
            out.push(Value::Object(obj));
        }
    }
    out
}

fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub fn run_sync_with_problem_matcher(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: u64,
    problem_matcher: Option<&str>,
) -> Result<String> {
    let result = run_sync(command, cwd, timeout_secs)?;
    let failed = result.timed_out || result.exit_code.unwrap_or(1) > 0;

    let stripped = combine_output_stripped(&result.stdout, &result.stderr);

    if !failed {
        return Ok(if stripped.trim().is_empty() {
            "Execution successful. (no output)".to_string()
        } else {
            stripped
        });
    }

    if let Some(name) = problem_matcher
        && let Some(matcher) = get_matcher(name)
    {
        let extracted = extract_problems(&stripped, &matcher);
        if !extracted.is_empty() {
            return Ok(serde_json::to_string(&extracted).unwrap_or_else(|_| "[]".to_string()));
        }
    }

    let tail = tail_lines(&stripped, 15);
    if tail.is_empty() {
        Ok("Execution failed. (no output)".to_string())
    } else {
        Ok(tail)
    }
}



/// Build a `Command` that runs `command` via the platform-appropriate shell.
///
/// - Unix    : `sh -c <command>`  (also augments PATH — see [`augment_unix_path`])
/// - Windows : `cmd /C <command>` (also augments PATH — see [`augment_windows_path`])
fn build_shell_command(command: &str) -> Command {
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        augment_unix_path(&mut cmd);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        augment_windows_path(&mut cmd);
        cmd
    }
}

/// Unix only — prepend common user-local tool directories to the child's
/// `PATH` when they exist but are absent from the current process `PATH`.
///
/// This is necessary on Linux where IDEs (e.g. VS Code) may start the MCP
/// server with a reduced `PATH` that does not include `~/.cargo/bin` or
/// `~/.local/bin`, causing commands like `cargo`, `node`, and pip-installed
/// scripts to be silently unavailable inside `cortex_act_shell_exec`.
///
/// Directories added (in prepend order, highest priority first):
/// 1. `~/.cargo/bin`  — Rust toolchain installed via rustup
/// 2. `~/.local/bin`  — pipx / pip --user / manual scripts
/// 3. `/usr/local/bin` — Homebrew (macOS), system packages
#[cfg(not(windows))]
fn augment_unix_path(cmd: &mut Command) {
    let home = match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => h,
        _ => return,
    };
    let current_path = std::env::var("PATH").unwrap_or_default();
    // Split on `:` so we do exact component comparisons, not substring matches.
    let path_components: Vec<&str> = current_path.split(':').collect();

    let candidates = [
        format!("{home}/.cargo/bin"),
        format!("{home}/.local/bin"),
        "/usr/local/bin".to_string(),
    ];

    let mut prepend: Vec<String> = Vec::new();
    for dir in &candidates {
        if !path_components.contains(&dir.as_str()) && std::path::Path::new(dir).is_dir() {
            prepend.push(dir.clone());
        }
    }

    if !prepend.is_empty() {
        let new_path = format!("{}:{current_path}", prepend.join(":"));
        cmd.env("PATH", new_path);
    }
}

/// Windows only — prepend common user-local tool directories to the child's
/// `PATH` when they exist but are absent from the current process `PATH`.
///
/// IDEs (e.g. VS Code) may start the MCP server with a restricted `PATH` that
/// omits user directories, causing commands like `cargo`, `npx`, and `tsc` to
/// be unavailable inside `cortex_act_shell_exec`.
///
/// Directories added (in prepend order, highest priority first):
/// 1. `%USERPROFILE%\.cargo\bin`            — Rust toolchain via rustup
/// 2. `%USERPROFILE%\AppData\Roaming\npm`   — npm global binaries (npx, tsc…)
#[cfg(windows)]
fn augment_windows_path(cmd: &mut Command) {
    let userprofile = match std::env::var("USERPROFILE") {
        Ok(h) if !h.is_empty() => h,
        // Fall back to HOMEDRIVE+HOMEPATH if USERPROFILE is absent.
        _ => {
            let drive = std::env::var("HOMEDRIVE").unwrap_or_default();
            let path  = std::env::var("HOMEPATH").unwrap_or_default();
            let combined = format!("{drive}{path}");
            if combined.trim().is_empty() {
                return;
            }
            combined
        }
    };

    let current_path = std::env::var("PATH").unwrap_or_default();
    if let Some(new_path) = augmented_windows_path(&current_path, &userprofile) {
        cmd.env("PATH", new_path);
    }
}

#[cfg(windows)]
fn augmented_windows_path(current_path: &str, userprofile: &str) -> Option<String> {
    // On Windows, PATH is separated by `;`.  Use case-insensitive comparison
    // because the Windows file-system is case-insensitive.
    let path_lower: Vec<String> = current_path
        .split(';')
        .map(|s| s.trim().to_lowercase())
        .collect();

    let candidates = [
        format!("{userprofile}\\.cargo\\bin"),
        format!("{userprofile}\\AppData\\Roaming\\npm"),
    ];

    let mut prepend: Vec<String> = Vec::new();
    for dir in &candidates {
        let dir_lower = dir.to_lowercase();
        if !path_lower.iter().any(|c| *c == dir_lower)
            && std::path::Path::new(dir).is_dir()
        {
            prepend.push(dir.clone());
        }
    }

    (!prepend.is_empty()).then(|| format!("{};{current_path}", prepend.join(";")))
}

/// Run `command` synchronously via the platform shell.  Blocks until done
/// or `timeout_secs` elapsed.
///
/// On timeout the child process is force-killed:
/// - Unix    : `kill -9 <pid>`
/// - Windows : `taskkill /PID <pid> /F`
pub fn run_sync(command: &str, cwd: Option<&str>, timeout_secs: u64) -> Result<ShellResult> {
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let start = Instant::now();

    let mut cmd = build_shell_command(command);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Close stdin so SSH / sshpass never inherit the MCP server's stdio
        // pipe and hang waiting for input (e.g. `ssh host 'python3 -'`).
        .stdin(Stdio::null());

    // Unix: put the child in its own process group so that a timeout kill
    // signal reaches sshpass, ssh, and all remote-forwarded children — not
    // just the `sh -c` wrapper.
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let child = cmd.spawn().context("Failed to spawn command")?;
    let pid = child.id();

    // Spawn a thread that drains stdout+stderr and waits for the process.
    let (tx, rx) = mpsc::channel::<std::io::Result<std::process::Output>>();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        // ── Process finished within timeout ──────────────────────────────
        Ok(Ok(output)) => Ok(ShellResult {
            command: command.to_string(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
            timed_out: false,
            duration_ms: start.elapsed().as_millis() as u64,
        }),

        // ── Child wait_with_output() itself returned an IO error ──────────
        Ok(Err(e)) => Err(e).context("wait_with_output failed"),

        // ── Timeout — force-kill the child process ────────────────────────
        Err(_recv_timeout) => {
            // Best-effort kill.  Uses only stdlib + one extra process spawn
            // so no libc / windows-sys dependency is needed.
            //
            // Unix: kill the entire process group (negative PID) first so
            // that sshpass, ssh, and any remote-forwarded processes are all
            // terminated.  Fall back to killing the individual PID as well.
            #[cfg(not(windows))]
            {
                let _ = Command::new("kill")
                    .args(["-9", &format!("-{pid}")])
                    .output();
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
            }
            #[cfg(windows)]
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .output();

            // Drain the thread so we don't leak it (finishes quickly after kill).
            let _ = rx.recv_timeout(Duration::from_secs(2));

            Ok(ShellResult {
                command: command.to_string(),
                stdout: String::new(),
                stderr: format!(
                    "Timed out after {}s. Process PID {} killed.",
                    timeout_secs, pid
                ),
                exit_code: None,
                timed_out: true,
                duration_ms: start.elapsed().as_millis() as u64,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn augmented_windows_path_prepends_cargo_and_npm_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let userprofile = dir.path().join("user");
        let cargo_bin = userprofile.join(".cargo").join("bin");
        let npm_bin = userprofile.join("AppData").join("Roaming").join("npm");
        std::fs::create_dir_all(&cargo_bin).expect("create cargo dir");
        std::fs::create_dir_all(&npm_bin).expect("create npm dir");

        let existing = r#"C:\Windows\System32"#;
        let new_path = augmented_windows_path(existing, &userprofile.to_string_lossy())
            .expect("should augment windows path");

        let parts: Vec<&str> = new_path.split(';').collect();
        assert_eq!(parts[0], cargo_bin.to_string_lossy());
        assert_eq!(parts[1], npm_bin.to_string_lossy());
        assert!(parts.iter().any(|part| *part == existing));
    }

    #[cfg(windows)]
    #[test]
    fn augmented_windows_path_skips_existing_entries_case_insensitively() {
        let dir = tempfile::tempdir().expect("tempdir");
        let userprofile = dir.path().join("user");
        let cargo_bin = userprofile.join(".cargo").join("bin");
        let npm_bin = userprofile.join("AppData").join("Roaming").join("npm");
        std::fs::create_dir_all(&cargo_bin).expect("create cargo dir");
        std::fs::create_dir_all(&npm_bin).expect("create npm dir");

        let existing = format!(
            "{};{}",
            cargo_bin.to_string_lossy().to_string().to_uppercase(),
            npm_bin.to_string_lossy()
        );

        assert!(augmented_windows_path(&existing, &userprofile.to_string_lossy()).is_none());
    }
}
