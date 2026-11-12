//! codeguard - test, benchmark, profile and safely auto-fix Python, Rust and C++ repositories.
//!
//! Orchestrates existing per-language tools behind a single trait. Each language is a plugin.
//! Safe rules are auto-applied inside a verified git-checkpoint loop; everything else is
//! report-only with AI-actionable tips.

pub mod adapters;
pub mod bench;
pub mod cache;
pub mod cli;
pub mod config;
pub mod git;
pub mod profile;
pub mod report;
pub mod rule_engine;
pub mod scan;
pub mod types;

use thiserror::Error;

/// Result type used throughout codeguard.
pub type Result<T> = std::result::Result<T, Error>;

/// Fatal/recoverable errors. None of these use `unwrap` internally.
#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error at {path}: {message}")]
    Toml { path: String, message: String },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("command not found: {0}")]
    CommandNotFound(String),
    #[error("command failed: {command} (exit {exit:?})\nstderr: {stderr}")]
    CommandFailed {
        command: String,
        exit: Option<i32>,
        stderr: String,
    },
    #[error("config error: {0}")]
    Config(String),
    #[error("rule error: {0}")]
    Rule(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("no language detected in {path}")]
    NoLanguageDetected { path: String },
    #[error("aborted: {0}")]
    Aborted(String),
}

/// Serde-friendly TOML error wrapper.
impl From<toml::de::Error> for Error {
    fn from(err: toml::de::Error) -> Self {
        Error::Toml {
            path: "<toml>".into(),
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_is_clear() {
        let err = Error::CommandNotFound("cargo".into());
        assert_eq!(err.to_string(), "command not found: cargo");
    }

    #[test]
    fn command_failed_captures_stderr() {
        let err = Error::CommandFailed {
            command: "pytest".into(),
            exit: Some(1),
            stderr: "ModuleNotFoundError".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("pytest"));
        assert!(msg.contains("ModuleNotFoundError"));
    }
}
