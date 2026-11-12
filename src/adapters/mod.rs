//! Per-language adapters.
//!
//! Each language implements `LanguageAdapter`, orchestrating existing tools
//! (pytest, cargo, clang-tidy, ...) instead of reimplementing them.

use crate::Result;
use crate::types::{BenchmarkResult, Language, Rule, ToolResult};
use std::path::Path;

pub mod cpp;
pub mod python;
pub mod rust;

/// A ranked profiler hotspot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Hotspot {
    pub function: String,
    pub file: String,
    pub line: Option<u32>,
    pub percent_runtime: f64,
    pub samples: u64,
}

/// Result of applying one fix candidate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FixResult {
    pub rule_id: String,
    pub file: String,
    pub applied: bool,
    pub reverted: bool,
    pub reason: String,
}

/// Context passed to every adapter invocation.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub path: &'a std::path::Path,
    pub jobs: usize,
    pub timeout_s: Option<u64>,
    pub dry_run: bool,
    pub force: bool,
}

/// Every language plugin implements this trait.
pub trait LanguageAdapter: Send + Sync {
    fn language(&self) -> Language;
    /// True if this adapter applies to the given directory.
    fn detect(&self, path: &std::path::Path) -> Result<bool>;
    /// Build the project (compile). Returns skipped if no build system.
    fn build(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run the test suite.
    fn test(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run the linter.
    fn lint(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run a fuzzer briefly.
    fn fuzz(&self, ctx: Context<'_>) -> Result<ToolResult>;
    /// Run benchmarks and return measurements.
    fn bench(&self, ctx: Context<'_>) -> Result<Vec<BenchmarkResult>>;
    /// Profile and return ranked hotspots.
    fn profile(&self, ctx: Context<'_>) -> Result<Vec<Hotspot>>;
    /// Apply safe rules (filtered by the caller) and report what was applied.
    fn fix(&self, _ctx: Context<'_>, _rules: &[Rule]) -> Result<Vec<FixResult>> {
        Ok(vec![])
    }
}

/// Convenience aliases for collections used by the CLI.
pub type AdapterList = Vec<Box<dyn LanguageAdapter>>;
pub type ToolResults = Vec<ToolResult>;

/// Run a command, capturing output and timing. Never panics on failure;
/// a non-zero exit is reported in ToolResult. A timeout kills the process.
pub fn run_command(
    cwd: &Path,
    program: &str,
    args: &[&str],
    timeout_s: Option<u64>,
) -> Result<ToolResult> {
    let start = std::time::Instant::now();
    let mut cmd = std::process::Command::new(program);
    cmd.current_dir(cwd);
    // Cargo writes its target dir next to the manifest by default; keep it per-repo
    // (and off any noexec mount) unless the caller already set CARGO_TARGET_DIR.
    if program == "cargo" && std::env::var("CARGO_TARGET_DIR").is_err() {
        let target_dir = cwd.join(".codeguard").join("target");
        let _ = std::fs::create_dir_all(&target_dir);
        cmd.env("CARGO_TARGET_DIR", target_dir);
    }
    cmd.args(args);
    // Capture stdout/stderr so timeout kills still yield partial output.
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| crate::Error::CommandFailed {
        command: program.to_string(),
        exit: None,
        stderr: e.to_string(),
    })?;

    let output = match timeout_s {
        Some(secs) => {
            let deadline = start + std::time::Duration::from_secs(secs);
            loop {
                match child.try_wait().map_err(|e| crate::Error::CommandFailed {
                    command: program.to_string(),
                    exit: None,
                    stderr: e.to_string(),
                })? {
                    Some(_status) => {
                        break child.wait_with_output().map_err(|e| {
                            crate::Error::CommandFailed {
                                command: program.to_string(),
                                exit: None,
                                stderr: e.to_string(),
                            }
                        })?;
                    }
                    None => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            let output = child.wait_with_output().map_err(|e| {
                                crate::Error::CommandFailed {
                                    command: program.to_string(),
                                    exit: None,
                                    stderr: e.to_string(),
                                }
                            })?;
                            let elapsed = start.elapsed().as_millis() as u64;
                            return Ok(ToolResult {
                                tool: program.to_string(),
                                command: format!("{} {}", program, args.join(" ")),
                                exit_code: output.status.code(),
                                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                                stderr: format!(
                                    "timed out after {secs}s\n{}",
                                    String::from_utf8_lossy(&output.stderr)
                                ),
                                duration_ms: elapsed,
                                skipped: false,
                                skip_reason: None,
                            });
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
        None => child
            .wait_with_output()
            .map_err(|e| crate::Error::CommandFailed {
                command: program.to_string(),
                exit: None,
                stderr: e.to_string(),
            })?,
    };

    let duration = start.elapsed().as_millis() as u64;
    let exit_code = output.status.code();
    Ok(ToolResult {
        tool: program.to_string(),
        command: format!("{} {}", program, args.join(" ")),
        exit_code,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        duration_ms: duration,
        skipped: false,
        skip_reason: None,
    })
}

/// Run a command in `path` with `sh -c`, capturing output. Never panics on failure;
/// a timeout kills the process.
pub fn run_command_in_dir(
    path: &Path,
    command: &str,
    timeout_s: Option<u64>,
) -> Result<ToolResult> {
    let start = std::time::Instant::now();
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd.current_dir(path);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| crate::Error::CommandFailed {
        command: command.to_string(),
        exit: None,
        stderr: e.to_string(),
    })?;

    let output = match timeout_s {
        Some(secs) => {
            let deadline = start + std::time::Duration::from_secs(secs);
            loop {
                match child.try_wait().map_err(|e| crate::Error::CommandFailed {
                    command: command.to_string(),
                    exit: None,
                    stderr: e.to_string(),
                })? {
                    Some(_status) => {
                        break child.wait_with_output().map_err(|e| {
                            crate::Error::CommandFailed {
                                command: command.to_string(),
                                exit: None,
                                stderr: e.to_string(),
                            }
                        })?;
                    }
                    None => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            break child.wait_with_output().map_err(|e| {
                                crate::Error::CommandFailed {
                                    command: command.to_string(),
                                    exit: None,
                                    stderr: e.to_string(),
                                }
                            })?;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
        None => child
            .wait_with_output()
            .map_err(|e| crate::Error::CommandFailed {
                command: command.to_string(),
                exit: None,
                stderr: e.to_string(),
            })?,
    };

    let duration = start.elapsed().as_millis() as u64;
    Ok(ToolResult {
        tool: "rule-action".into(),
        command: command.to_string(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        duration_ms: duration,
        skipped: false,
        skip_reason: None,
    })
}

/// Check a single tool is available on PATH; return a skipped ToolResult if not.
pub fn require_tool(tool: &str) -> Result<ToolResult> {
    match which::which(tool) {
        Ok(_) => Ok(ToolResult {
            tool: tool.to_string(),
            command: format!("which {tool}"),
            exit_code: Some(0),
            stdout: format!("{tool} found"),
            stderr: String::new(),
            duration_ms: 0,
            skipped: false,
            skip_reason: None,
        }),
        Err(_) => Ok(ToolResult {
            tool: tool.to_string(),
            command: format!("which {tool}"),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            skipped: true,
            skip_reason: Some(format!("{tool} not found on PATH")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_reports_non_zero_exit() {
        let res = run_command(Path::new("."), "sh", &["-c", "exit 3"], None).unwrap();
        assert_eq!(res.exit_code, Some(3));
        assert!(!res.success());
    }

    #[test]
    fn missing_tool_is_skipped_not_error() {
        let res = require_tool("definitely-not-a-real-codeguard-tool-xyz").unwrap();
        assert!(res.skipped);
        assert!(res.skip_reason.is_some());
        assert_eq!(res.exit_code, None);
    }
}
