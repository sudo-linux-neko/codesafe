//! Shared data types for codeguard.

use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::Digest;

/// Detected programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    Rust,
    Cpp,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Python => write!(f, "python"),
            Language::Rust => write!(f, "rust"),
            Language::Cpp => write!(f, "cpp"),
        }
    }
}

/// Finding severity for CI gating (`--fail-on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}

/// Parse a severity from a CLI string, case-insensitive.
impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Severity::Info),
            "warning" | "warn" => Ok(Severity::Warning),
            "error" | "err" => Ok(Severity::Error),
            "critical" | "crit" => Ok(Severity::Critical),
            other => Err(format!("unknown severity: {other}")),
        }
    }
}

/// Whether a rule may be auto-applied without review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSafety {
    Safe,
    Review,
}

impl std::fmt::Display for RuleSafety {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSafety::Safe => write!(f, "safe"),
            RuleSafety::Review => write!(f, "review"),
        }
    }
}

/// Status of a finding after a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingStatus {
    Fixed,
    Reported,
}

impl std::fmt::Display for FindingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FindingStatus::Fixed => write!(f, "fixed"),
            FindingStatus::Reported => write!(f, "reported"),
        }
    }
}

/// Outcome of running one external tool once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub skipped: bool,
    pub skip_reason: Option<String>,
}

impl ToolResult {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            tool: String::new(),
            command: String::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            skipped: true,
            skip_reason: Some("tool not found".into()),
        }
    }
}

/// One detected issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub language: Language,
    pub file: String,
    pub line: Option<u32>,
    pub category: String,
    pub status: FindingStatus,
    pub description: String,
    pub fix: Option<String>,
    pub evidence: String,
}

/// One benchmark measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
    pub unit: String,
    pub change_pct: Option<f64>,
}

/// One AI-actionable tip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTip {
    pub file: String,
    pub line: Option<u32>,
    pub concrete_change: String,
    pub why_safe: String,
    pub how_to_verify: String,
}

/// One editable rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub language: Language,
    pub trigger: String,
    pub action: String,
    pub safety: RuleSafety,
    pub explanation: String,
}

/// Aggregated summary of a run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Summary {
    pub language: Option<Language>,
    pub tools_found: usize,
    pub tools_missing: usize,
    pub tests_passed: bool,
    pub tests_total: u64,
    pub tests_failed: u64,
    pub findings_total: usize,
    pub findings_fixed: usize,
    pub benchmarks_total: usize,
    pub benchmarks_improved: usize,
    pub benchmarks_regressed: usize,
}

/// Full report emitted as report.md and report.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Report {
    pub path: String,
    pub summary: Summary,
    pub findings: Vec<Finding>,
    pub benchmarks: Vec<BenchmarkResult>,
    pub ai_tips: Vec<AiTip>,
    pub tool_results: Vec<ToolResult>,
}

impl Report {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }
}

/// Key/value config loaded from `codeguard.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub jobs: Option<usize>,
    pub timeout_s: Option<u64>,
    pub fail_on: Option<String>,
    pub dry_run: Option<bool>,
    pub force: Option<bool>,
    pub bench_runs: Option<u32>,
    pub regression_threshold_pct: Option<f64>,
    pub rules_dir: Option<String>,
}

impl Config {
    pub fn jobs(&self) -> usize {
        self.jobs.unwrap_or(1)
    }
    pub fn timeout_s(&self) -> Option<u64> {
        self.timeout_s
    }
    pub fn fail_on(&self) -> Option<Severity> {
        self.fail_on
            .as_ref()
            .and_then(|s| s.parse::<Severity>().ok())
    }
    pub fn bench_runs(&self) -> u32 {
        self.bench_runs.unwrap_or(3)
    }
    pub fn regression_threshold_pct(&self) -> f64 {
        self.regression_threshold_pct.unwrap_or(5.0)
    }

    /// Validate config values. Returns an error for nonsensical settings
    /// (zero jobs, zero timeout, etc.).
    pub fn validate(&self) -> Result<()> {
        if let Some(jobs) = self.jobs {
            if jobs == 0 {
                return Err(crate::Error::Config("jobs must be at least 1".into()));
            }
            if jobs > 64 {
                return Err(crate::Error::Config("jobs must not exceed 64".into()));
            }
        }
        if let Some(timeout) = self.timeout_s
            && timeout == 0
        {
            return Err(crate::Error::Config(
                "timeout_s must be at least 1 second".into(),
            ));
        }
        Ok(())
    }
}

pub type FileHash = String;

/// Per-file content hash used for caching.
pub fn hash_file(path: &std::path::Path) -> crate::Result<FileHash> {
    let bytes =
        std::fs::read(path).map_err(|e| crate::Error::Cache(format!("reading {path:?}: {e}")))?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// Hash a directory's relevant files into a single cache key.
pub fn hash_files(files: &[std::path::PathBuf]) -> crate::Result<FileHash> {
    let mut hasher = sha2::Sha256::new();
    for path in files {
        if let Ok(bytes) = std::fs::read(path) {
            hasher.update(&bytes);
        }
        hasher.update(path.to_string_lossy().as_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        assert!(Severity::Critical > Severity::Error);
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn severity_from_str_case_insensitive() {
        assert_eq!("CRITICAL".parse::<Severity>(), Ok(Severity::Critical));
        assert_eq!("warn".parse::<Severity>(), Ok(Severity::Warning));
        assert!("bogus".parse::<Severity>().is_err());
    }

    #[test]
    fn tool_result_success() {
        let mut r = ToolResult::default();
        assert!(!r.success());
        r.exit_code = Some(0);
        assert!(r.success());
        r.exit_code = Some(1);
        assert!(!r.success());
    }

    #[test]
    fn hash_file_is_deterministic() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let h1 = hash_file(&dir.join("a.txt")).unwrap();
        let h2 = hash_file(&dir.join("a.txt")).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("codeguard-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
