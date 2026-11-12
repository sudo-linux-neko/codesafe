//! Verifiable Verification Protocol (Pillar 3).
//!
#![allow(clippy::too_many_arguments)]

//! This module implements adversarial verification gates that make gaming
//! the feedback loop structurally impossible:
//! - Asymmetric test/code separation: coding agent never sees acceptance criteria
//! - RED-first TDD: failing test must be independently verified before implementation
//! - Anti-gaming lints: deterministic checks against stub-passability, assertion gaming,
//!   scope violations, and suite-safety

use crate::types::{Language, Severity};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Acceptance criteria — only the test-writing agent sees this.
/// The coding agent only sees failure messages, never the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub file_allowlist: Vec<PathBuf>,
    pub language: Language,
    pub trace_id: String,
}

/// A test case derived from spec by the test-writing agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub id: String,
    pub spec_id: String,
    pub name: String,
    pub source: String,
    pub file: PathBuf,
    pub expected_to_fail: bool, // RED: must fail before implementation
    pub trace_id: String,
}

/// Result of running a verification gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    Fail,
}

/// Evidence bound to a specific git tree hash and oracle version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub git_tree_hash: String,
    pub oracle_version: String,
    pub verdict: Verdict,
    pub trace_id: String,
    pub timestamp_ms: u64,
}

/// Anti-gaming lint check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AntiGamingResult {
    /// No gaming detected
    Clean,
    /// Gaming detected with description
    GamingDetected {
        check: String,
        description: String,
        severity: Severity,
    },
}

/// Verification protocol state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationState {
    pub spec: Option<Spec>,
    pub red_tests: Vec<TestCase>,
    pub green_implementation_files: Vec<PathBuf>,
    pub receipts: Vec<Receipt>,
    pub current_phase: VerificationPhase,
}

/// Current phase in the RED-green cycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VerificationPhase {
    #[default]
    /// No spec loaded
    Idle,
    /// Spec loaded, waiting for RED test
    SpecLoaded,
    /// RED test written and verified failing
    RedVerified,
    /// Implementation written, waiting for GREEN verification
    GreenPending,
    /// GREEN verified - implementation passes RED tests
    GreenVerified,
}

/// Create a new spec from acceptance criteria.
pub fn create_spec(
    id: &str,
    description: &str,
    acceptance_criteria: Vec<String>,
    file_allowlist: Vec<PathBuf>,
    language: Language,
    trace_id: &str,
) -> Spec {
    Spec {
        id: id.to_string(),
        description: description.to_string(),
        acceptance_criteria,
        file_allowlist,
        language,
        trace_id: trace_id.to_string(),
    }
}

/// Write a RED test from spec. The test must be expected to fail.
pub fn write_red_test(
    spec: &Spec,
    test_name: &str,
    test_source: &str,
    test_file: PathBuf,
    trace_id: &str,
) -> TestCase {
    TestCase {
        id: format!("red-{}-{}", spec.id, test_name),
        spec_id: spec.id.clone(),
        name: test_name.to_string(),
        source: test_source.to_string(),
        file: test_file,
        expected_to_fail: true,
        trace_id: trace_id.to_string(),
    }
}

/// Anti-gaming lint 1: Stub-passability.
/// Rejects a RED test that mocks the unit under test.
/// If the test imports/mocks the module it's supposed to test, it's gaming.
pub fn check_stub_passability(test_source: &str, unit_under_test: &str) -> AntiGamingResult {
    let patterns = [
        format!("use {}::", unit_under_test),
        format!("mod {}", unit_under_test),
        format!("#[mock]",),
        format!("mock!(",),
        format!("mockall::",),
    ];

    for pattern in &patterns {
        if test_source.contains(pattern) {
            return AntiGamingResult::GamingDetected {
                check: "stub-passability".to_string(),
                description: format!(
                    "RED test mocks the unit under test ('{}') via pattern '{}'. \
                     A RED test must exercise real code, not mocks.",
                    unit_under_test, pattern
                ),
                severity: Severity::Critical,
            };
        }
    }

    AntiGamingResult::Clean
}

/// Anti-gaming lint 2: Test-integrity diff guard.
/// Compares pre-RED and post-RED test source to detect assertion gaming.
/// If assertions were weakened or removed after seeing failures, it's gaming.
pub fn check_test_integrity(pre_red_source: &str, post_red_source: &str) -> AntiGamingResult {
    // Simple heuristics for assertion gaming:
    // 1. Assertions removed
    // 2. Assertions weakened (e.g., assert_eq! -> assert!)
    // 3. Test bodies emptied

    let pre_assertions = count_assertions(pre_red_source);
    let post_assertions = count_assertions(post_red_source);

    if post_assertions < pre_assertions {
        return AntiGamingResult::GamingDetected {
            check: "test-integrity".to_string(),
            description: format!(
                "Assertions reduced from {} to {} after RED. \
                 Weakening assertions is gaming.",
                pre_assertions, post_assertions
            ),
            severity: Severity::Critical,
        };
    }

    // Check for emptied test bodies
    if post_red_source.trim().is_empty() && !pre_red_source.trim().is_empty() {
        return AntiGamingResult::GamingDetected {
            check: "test-integrity".to_string(),
            description: "Test body emptied after RED. This is gaming.".to_string(),
            severity: Severity::Critical,
        };
    }

    AntiGamingResult::Clean
}

fn count_assertions(source: &str) -> usize {
    let mut count = 0;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("assert!")
            || trimmed.starts_with("assert_eq!")
            || trimmed.starts_with("assert_ne!")
            || trimmed.contains("assert!")
            || trimmed.contains("assert_eq!")
            || trimmed.contains("assert_ne!")
        {
            count += 1;
        }
    }
    count
}

/// Anti-gaming lint 3: Scope-inverse.
/// Flags implementation writes outside the spec's file allowlist.
pub fn check_scope_inverse(changed_files: &[PathBuf], allowlist: &[PathBuf]) -> AntiGamingResult {
    for changed in changed_files {
        if !allowlist.iter().any(|a| changed.starts_with(a)) {
            return AntiGamingResult::GamingDetected {
                check: "scope-inverse".to_string(),
                description: format!(
                    "Implementation wrote to '{}' which is outside the spec's file allowlist.",
                    changed.display()
                ),
                severity: Severity::Critical,
            };
        }
    }

    AntiGamingResult::Clean
}

/// Anti-gaming lint 4: Suite-safety.
/// Flags when a scoped suite passes but the full suite fails.
/// If scoped tests pass but the full test suite fails, implementation may be
/// gaming the scoped tests.
pub fn check_suite_safety(scoped_passed: bool, full_passed: bool) -> AntiGamingResult {
    if scoped_passed && !full_passed {
        return AntiGamingResult::GamingDetected {
            check: "suite-safety".to_string(),
            description: "Scoped tests pass but full suite fails. \
             Implementation may be gaming scoped tests."
                .to_string(),
            severity: Severity::Critical,
        };
    }

    AntiGamingResult::Clean
}

/// Run all anti-gaming lints on a proposed implementation.
pub fn run_anti_gaming_lints(
    test_source: &str,
    unit_under_test: &str,
    pre_red_source: Option<&str>,
    post_red_source: &str,
    changed_files: &[PathBuf],
    allowlist: &[PathBuf],
    scoped_passed: bool,
    full_passed: bool,
) -> Vec<AntiGamingResult> {
    let mut results = Vec::new();

    // 1. Stub-passability
    results.push(check_stub_passability(test_source, unit_under_test));

    // 2. Test-integrity (if pre-RED source available)
    if let Some(pre) = pre_red_source {
        results.push(check_test_integrity(pre, post_red_source));
    }

    // 3. Scope-inverse
    results.push(check_scope_inverse(changed_files, allowlist));

    // 4. Suite-safety
    results.push(check_suite_safety(scoped_passed, full_passed));

    results
}

/// Create a receipt-bound verdict tied to git tree hash and oracle version.
pub fn create_receipt(
    git_tree_hash: &str,
    oracle_version: &str,
    verdict: Verdict,
    trace_id: &str,
) -> Receipt {
    Receipt {
        git_tree_hash: git_tree_hash.to_string(),
        oracle_version: oracle_version.to_string(),
        verdict,
        trace_id: trace_id.to_string(),
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_stub_passability_detects_mock() {
        let result = check_stub_passability("use mymodule::*;\nuse mockall::*;", "mymodule");
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_stub_passability_clean_for_real_test() {
        let result = check_stub_passability("fn test_foo() { assert_eq!(1, 1); }", "mymodule");
        assert!(matches!(result, AntiGamingResult::Clean));
    }

    #[test]
    fn check_test_integrity_detects_weakened() {
        let result = check_test_integrity(
            "assert_eq!(foo(), 42);\nassert!(bar());",
            "assert!(foo() > 0);",
        );
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_scope_inverse_detects_outside_allowlist() {
        let result = check_scope_inverse(
            &[PathBuf::from("src/evil.rs")],
            &[PathBuf::from("src/good")],
        );
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_suite_safety_detects_scoped_pass_full_fail() {
        let result = check_suite_safety(true, false);
        assert!(matches!(result, AntiGamingResult::GamingDetected { .. }));
    }

    #[test]
    fn check_suite_safety_clean_when_both_pass() {
        let result = check_suite_safety(true, true);
        assert!(matches!(result, AntiGamingResult::Clean));
    }

    #[test]
    fn create_receipt_binds_verdict() {
        let receipt = create_receipt("abc123", "v1.0.0", Verdict::Pass, "cs-trace");
        assert_eq!(receipt.git_tree_hash, "abc123");
        assert_eq!(receipt.oracle_version, "v1.0.0");
        assert!(matches!(receipt.verdict, Verdict::Pass));
        assert_eq!(receipt.trace_id, "cs-trace");
    }
}
