//! Command-line interface for codesafe.
//!
//! Every command supports --output (human|json|jsonl) and --schema for
//! agent-facing introspection. The generate-skill subcommand emits an
//! agent skill document for LLM tool-use.

use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;
use std::str::FromStr;

use crate::aci::OutputMode;
use crate::types::{Config, Severity};

/// codesafe - test, benchmark, profile and safely auto-fix Python, Rust and C++ repositories.
///
/// Epistemic infrastructure for agentic coding: independently certifies
/// whether agent work is correct, making gaming of the feedback loop
/// structurally impossible.
#[derive(Debug, Parser)]
#[command(name = "codesafe", version, author, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to codesafe.toml config file.
    #[arg(long, global = true, default_value = "codesafe.toml")]
    pub config: PathBuf,

    /// Number of parallel jobs.
    #[arg(long, global = true, short = 'j')]
    pub jobs: Option<usize>,

    /// Timeout in seconds per tool invocation.
    #[arg(long, global = true)]
    pub timeout: Option<u64>,

    /// Fail with non-zero exit code if any finding has severity at least this level.
    #[arg(long, global = true)]
    pub fail_on: Option<String>,

    /// Do not modify any files; report only.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Allow running fix on a dirty git tree.
    #[arg(long, global = true)]
    pub force: bool,

    /// Output mode: human, json, or jsonl.
    #[arg(long, global = true, default_value = "human")]
    pub output: String,
}

#[derive(Debug, Parser)]
pub enum Command {
    /// Detect languages and build systems, list available tools.
    Scan {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Build and run tests, lint, sanitizers.
    Test {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Run benchmarks, profile, rank hotspots.
    Bench {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Apply safe rule-based fixes with verification.
    Fix {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Write report.md and report.json.
    Report {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Scan, test, bench, fix, report.
    All {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Initialize a codesafe.toml config and rules/ directory for a project.
    Init {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
    /// Emit a JSON schema for all commands and flags (agent introspection).
    #[command(hide = true)]
    Schema,
    /// Generate an agent skill document for LLM tool-use.
    #[command(hide = true)]
    GenerateSkill {
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
}

/// Resolved runtime configuration: config file merged with CLI overrides.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeConfig {
    pub jobs: usize,
    pub timeout_s: Option<u64>,
    pub fail_on: Option<Severity>,
    pub dry_run: bool,
    pub force: bool,
    pub bench_runs: u32,
    pub regression_threshold_pct: f64,
    pub rules_dir: Option<String>,
    pub config_path: PathBuf,
    pub output_mode: OutputMode,
}

impl RuntimeConfig {
    pub fn from_cli(cli: &Cli, config: Config) -> crate::Result<Self> {
        config.validate()?;
        let output_mode = OutputMode::from_str(&cli.output).map_err(crate::Error::Config)?;
        Ok(Self {
            jobs: cli.jobs.unwrap_or(config.jobs()),
            timeout_s: cli.timeout.or(config.timeout_s()),
            fail_on: cli
                .fail_on
                .as_ref()
                .and_then(|s| s.parse::<Severity>().ok())
                .or(config.fail_on()),
            dry_run: cli.dry_run || config.dry_run.unwrap_or(false),
            force: cli.force || config.force.unwrap_or(false),
            bench_runs: config.bench_runs(),
            regression_threshold_pct: config.regression_threshold_pct(),
            rules_dir: config.rules_dir.clone(),
            config_path: cli.config.clone(),
            output_mode,
        })
    }
}

/// JSON schema for all codesafe commands (agent introspection).
pub fn schema_json() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "codesafe CLI schema",
        "description": "Agent-facing schema for codesafe commands and flags.",
        "type": "object",
        "properties": {
            "config": { "type": "string", "description": "Path to codesafe.toml config file" },
            "jobs": { "type": "integer", "minimum": 1, "maximum": 64, "description": "Number of parallel jobs" },
            "timeout": { "type": "integer", "minimum": 1, "description": "Timeout in seconds per tool invocation" },
            "fail_on": { "type": "string", "enum": ["info", "warning", "error", "critical"], "description": "Fail if any finding has severity at least this level" },
            "dry_run": { "type": "boolean", "description": "Do not modify any files; report only" },
            "force": { "type": "boolean", "description": "Allow running fix on a dirty git tree" },
            "output": { "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode" }
        },
        "commands": {
            "scan": { "description": "Detect languages and build systems, list available tools", "args": { "path": { "type": "string", "default": "." } } },
            "test": { "description": "Build and run tests, lint, sanitizers", "args": { "path": { "type": "string", "default": "." } } },
            "bench": { "description": "Run benchmarks, profile, rank hotspots", "args": { "path": { "type": "string", "default": "." } } },
            "fix": { "description": "Apply safe rule-based fixes with verification", "args": { "path": { "type": "string", "default": "." } } },
            "report": { "description": "Write report.md and report.json", "args": { "path": { "type": "string", "default": "." } } },
            "all": { "description": "Scan, test, bench, fix, report", "args": { "path": { "type": "string", "default": "." } } },
            "init": { "description": "Initialize a codesafe.toml config and rules/ directory", "args": { "path": { "type": "string", "default": "." } } },
            "schema": { "description": "Emit this JSON schema", "args": {} },
            "generate-skill": { "description": "Generate an agent skill document", "args": { "path": { "type": "string", "default": "." } } }
        },
        "output_envelope": {
            "description": "Every json/jsonl response follows this envelope: { ok, result, error, meta }",
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "result": {},
                "error": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" },
                        "category": { "type": "string" },
                        "message": { "type": "string" },
                        "suggestion": { "type": "string" },
                        "is_retryable": { "type": "boolean" },
                        "trace_id": { "type": "string" }
                    }
                },
                "meta": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "path": { "type": "string" },
                        "timestamp_ms": { "type": "integer" },
                        "trace_id": { "type": "string" },
                        "output_mode": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// Agent skill document for LLM tool-use (e.g., tooli, OpenAI function calling).
pub fn skill_document(path: &str) -> String {
    format!(
        r#"{{
  "name": "codesafe",
  "description": "Test, benchmark, profile and safely auto-fix Python/Rust/C++ repositories. Epistemic infrastructure for agentic coding.",
  "version": "0.2.0",
  "commands": [
    {{
      "name": "codesafe scan",
      "description": "Detect languages, build systems, and available tools in a repository.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe test",
      "description": "Build and run tests, lint, sanitizers across all detected languages.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "fail_on": {{ "type": "string", "enum": ["info", "warning", "error", "critical"], "description": "Fail if any finding has severity at least this level" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe bench",
      "description": "Run benchmarks and detect regressions.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe fix",
      "description": "Apply safe rule-based fixes with verification (checkpoint → apply → verify → commit/revert).",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "force": {{ "type": "boolean", "description": "Allow running fix on a dirty git tree" }},
          "dry_run": {{ "type": "boolean", "description": "Report what would change without modifying files" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe report",
      "description": "Write report.md (human) and report.json (AI-actionable tips).",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe all",
      "description": "Run scan → test → bench → fix → report in one command.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Repository path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }},
    {{
      "name": "codesafe init",
      "description": "Initialize a codesafe.toml config and rules/ directory for a new project.",
      "parameters": {{
        "type": "object",
        "properties": {{
          "path": {{ "type": "string", "description": "Project path (default: .)" }},
          "output": {{ "type": "string", "enum": ["human", "json", "jsonl"], "description": "Output mode (default: human)" }}
        }},
        "required": [],
        "additionalProperties": false
      }}
    }}
  ],
  "repository": "{path}",
  "capabilities": [
    "structured_output",
    "parsed_diagnostics",
    "recovery_hints",
    "anti_gaming_verification",
    "receipt_bound_verdicts"
  ]
}}"#
    )
}
