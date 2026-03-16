//! # Synchronous Shell Executor — CortexACT
//!
//! Runs a shell command **synchronously** with a hard timeout.
//! Intended for short, fast commands (`git diff`, `cargo check`, `ls -la`).
//! Not intended for long-running watch mode or background servers.
//!
//! ## Timeout design
//!
//! The child process is spawned with piped stdout/stderr.
//! A monitoring thread polls the start time and issues SIGKILL via
//! `kill -9 <pid>` when the timeout is exceeded — no `libc` dependency needed.

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



/// Run `command` synchronously via `sh -c`. Blocks until done or `timeout_secs` elapsed.
///
/// On timeout the child process is killed via `kill -9 <pid>`.
pub fn run_sync(command: &str, cwd: Option<&str>, timeout_secs: u64) -> Result<ShellResult> {
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let start = Instant::now();

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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

        // ── Timeout — kill the process ────────────────────────────────────
        Err(_recv_timeout) => {
            // Best-effort SIGKILL. Works on macOS / Linux.
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();

            // Drain the thread so we don't leak it (it will finish quickly
            // after the process is killed).
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
