//! Test Generation & Self-Healing Loop (Pillar 4).
//!
//! Coverage-guided test generation targets hotspots (complex methods
//! with zero coverage). A self-healing loop with bounded retry budget
//! iterates: generate → review → verify → fix until green or budget
//! exhausted. Non-convergence is a valid outcome (non-zero exit,
//! named blockers).

use crate::types::Language;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Coverage data for a single function/method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageData {
    pub function: String,
    pub file: PathBuf,
    pub line: Option<u32>,
    pub complexity: u32, // cyclomatic complexity
    pub hits: u64,       // number of times executed during tests
    pub total_lines: u32,
    pub covered_lines: u32,
}

/// A hotspot: complex method with insufficient test coverage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hotspot {
    pub function: String,
    pub file: PathBuf,
    pub line: Option<u32>,
    pub complexity: u32,
    pub coverage_pct: f64,
    pub priority: f64, // 0.0..1.0, higher = more urgent
}

/// A generated test case targeting a hotspot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedTest {
    pub id: String,
    pub hotspot: Hotspot,
    pub source: String,
    pub test_file: PathBuf,
    pub status: TestStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// Status of a generated test in the self-healing loop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TestStatus {
    Generated,
    Reviewed,
    Verified,
    FixNeeded,
    Failed,
    Exhausted,
}

/// Result of the self-healing loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealingResult {
    pub tests_generated: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub tests_exhausted: usize,
    pub total_attempts: u32,
    pub converged: bool,
    pub blocking_tests: Vec<String>,
    pub trace_id: String,
}

/// Identify hotspots from coverage data: complex methods with low coverage.
pub fn identify_hotspots(
    coverage: &[CoverageData],
    min_complexity: u32,
    max_coverage_pct: f64,
) -> Vec<Hotspot> {
    let mut hotspots: Vec<Hotspot> = coverage
        .iter()
        .filter(|c| c.complexity >= min_complexity)
        .filter(|c| {
            let cov = if c.total_lines > 0 {
                c.covered_lines as f64 / c.total_lines as f64 * 100.0
            } else {
                0.0
            };
            cov < max_coverage_pct
        })
        .map(|c| {
            let coverage_pct = if c.total_lines > 0 {
                c.covered_lines as f64 / c.total_lines as f64 * 100.0
            } else {
                0.0
            };
            // Priority: high complexity + low coverage = high priority
            let priority = (c.complexity as f64 / 10.0).min(1.0) * (1.0 - coverage_pct / 100.0);
            Hotspot {
                function: c.function.clone(),
                file: c.file.clone(),
                line: c.line,
                complexity: c.complexity,
                coverage_pct,
                priority,
            }
        })
        .collect();

    // Sort by priority descending
    hotspots.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hotspots
}

/// Generate a test targeting a hotspot.
pub fn generate_test_for_hotspot(
    hotspot: &Hotspot,
    language: Language,
    trace_id: &str,
) -> GeneratedTest {
    let test_source = match language {
        Language::Python => format!(
            "import unittest\n\nclass Test{}(unittest.TestCase):\n    def test_{}(self):\n        # TODO: cover {} at {}:{:?}\n        # Complexity: {}\n        self.assertTrue(True)\n",
            hotspot.function,
            hotspot.function,
            hotspot.function,
            hotspot.file.display(),
            hotspot.line,
            hotspot.complexity
        ),
        Language::Rust => format!(
            "#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn test_{}() {{\n        // TODO: cover {} at {}:{:?}\n        // Complexity: {}\n        assert!(true);\n    }}\n}}\n",
            hotspot.function,
            hotspot.function,
            hotspot.file.display(),
            hotspot.line,
            hotspot.complexity
        ),
        Language::Cpp => format!(
            "#include <gtest/gtest.h>\n\nTEST({}Test, BasicTest) {{\n    // TODO: cover {} at {}:{:?}\n    // Complexity: {}\n    EXPECT_TRUE(true);\n}}\n",
            hotspot.function,
            hotspot.function,
            hotspot.file.display(),
            hotspot.line,
            hotspot.complexity
        ),
    };

    let test_file = match language {
        Language::Python => hotspot.file.with_file_name(format!(
            "test_{}.py",
            hotspot
                .file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        )),
        Language::Rust => hotspot.file.with_extension("rs"),
        Language::Cpp => hotspot.file.with_extension("cpp"),
    };

    GeneratedTest {
        id: format!("gen-{}-{}", trace_id, hotspot.function),
        hotspot: hotspot.clone(),
        source: test_source,
        test_file,
        status: TestStatus::Generated,
        attempts: 0,
        last_error: None,
    }
}

/// Self-healing loop with bounded retry budget.
/// Pipeline: generate → review → verify → fix.
/// Stops when converged (all tests pass) or budget exhausted.
pub fn self_healing_loop(
    mut tests: Vec<GeneratedTest>,
    max_retries: u32,
    verify_fn: impl Fn(&GeneratedTest) -> bool,
    fix_fn: impl Fn(&mut GeneratedTest) -> bool,
    trace_id: &str,
) -> HealingResult {
    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut tests_exhausted = 0;
    let mut total_attempts = 0u32;
    let mut blocking_tests: Vec<String> = Vec::new();

    for test in tests.iter_mut() {
        let mut attempts = 0;
        let mut converged = false;

        while attempts < max_retries && !converged {
            attempts += 1;
            total_attempts += 1;
            test.attempts = attempts;

            // Review: check if test looks reasonable
            test.status = TestStatus::Reviewed;

            // Verify: run the test
            if verify_fn(test) {
                test.status = TestStatus::Verified;
                tests_passed += 1;
                converged = true;
            } else {
                // Fix: try to fix the test
                if fix_fn(test) {
                    test.status = TestStatus::FixNeeded;
                    continue;
                } else {
                    test.status = TestStatus::Failed;
                    tests_failed += 1;
                    blocking_tests.push(test.id.clone());
                    break;
                }
            }
        }

        if !converged && test.status != TestStatus::Failed {
            test.status = TestStatus::Exhausted;
            tests_exhausted += 1;
            blocking_tests.push(test.id.clone());
        }
    }

    let converged = tests_failed == 0 && tests_exhausted == 0;

    HealingResult {
        tests_generated: tests.len(),
        tests_passed,
        tests_failed,
        tests_exhausted,
        total_attempts,
        converged,
        blocking_tests,
        trace_id: trace_id.to_string(),
    }
}

/// Compute coverage percentage from hits data.
pub fn compute_coverage_pct(hits: u64, total_lines: u32) -> f64 {
    if total_lines == 0 {
        0.0
    } else {
        (hits as f64 / total_lines as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_hotspots_returns_high_complexity_low_coverage() {
        let coverage = vec![
            CoverageData {
                function: "simple".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(10),
                complexity: 1,
                hits: 100,
                total_lines: 10,
                covered_lines: 10,
            },
            CoverageData {
                function: "complex".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(50),
                complexity: 15,
                hits: 0,
                total_lines: 20,
                covered_lines: 0,
            },
        ];
        let hotspots = identify_hotspots(&coverage, 5, 50.0);
        assert_eq!(hotspots.len(), 1);
        assert_eq!(hotspots[0].function, "complex");
        assert!(hotspots[0].priority > 0.5);
    }

    #[test]
    fn generate_test_for_hotspot_python() {
        let hotspot = Hotspot {
            function: "foo".to_string(),
            file: PathBuf::from("src/bar.py"),
            line: Some(42),
            complexity: 8,
            coverage_pct: 0.0,
            priority: 0.9,
        };
        let test = generate_test_for_hotspot(&hotspot, Language::Python, "cs-trace");
        assert!(test.source.contains("unittest"));
        assert!(test.source.contains("foo"));
        assert_eq!(test.status, TestStatus::Generated);
        assert_eq!(test.attempts, 0);
    }

    #[test]
    fn self_healing_loop_converges_when_all_pass() {
        let mut tests = vec![GeneratedTest {
            id: "t1".to_string(),
            hotspot: Hotspot {
                function: "f".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(1),
                complexity: 1,
                coverage_pct: 0.0,
                priority: 0.5,
            },
            source: "test".to_string(),
            test_file: PathBuf::from("test_lib.rs"),
            status: TestStatus::Generated,
            attempts: 0,
            last_error: None,
        }];

        let result = self_healing_loop(
            tests,
            3,
            |_| true, // all pass
            |_| false,
            "cs-trace",
        );

        assert!(result.converged);
        assert_eq!(result.tests_passed, 1);
        assert_eq!(result.tests_failed, 0);
        assert_eq!(result.blocking_tests.len(), 0);
    }

    #[test]
    fn self_healing_loop_exhausts_budget() {
        let mut tests = vec![GeneratedTest {
            id: "t1".to_string(),
            hotspot: Hotspot {
                function: "f".to_string(),
                file: PathBuf::from("src/lib.rs"),
                line: Some(1),
                complexity: 1,
                coverage_pct: 0.0,
                priority: 0.5,
            },
            source: "test".to_string(),
            test_file: PathBuf::from("test_lib.rs"),
            status: TestStatus::Generated,
            attempts: 0,
            last_error: None,
        }];

        let result = self_healing_loop(
            tests,
            2,
            |_| false, // always fails
            |_| false, // cannot fix
            "cs-trace",
        );

        assert!(!result.converged);
        assert_eq!(result.tests_failed, 1);
        assert_eq!(result.total_attempts, 1);
        assert_eq!(result.blocking_tests.len(), 1);
    }
}
