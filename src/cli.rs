//! Command-line interface for codeguard.

use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

use crate::types::{Config, Severity};

/// codeguard - test, benchmark, profile and safely auto-fix Python, Rust and C++ repositories.
#[derive(Debug, Parser)]
#[command(name = "codeguard", version, author, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to codeguard.toml config file.
    #[arg(long, global = true, default_value = "codeguard.toml")]
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
    /// Initialize a codeguard.toml config and rules/ directory for a project.
    Init {
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
}

impl RuntimeConfig {
    pub fn from_cli(cli: &Cli, config: Config) -> crate::Result<Self> {
        config.validate()?;
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
        })
    }
}
