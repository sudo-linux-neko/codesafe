//! Structured diagnostics parser.
//!
//! Parses raw tool output (compiler errors, linter output, test failures)
//! into structured, localized findings with recovery hints. Never truncates
//! diagnostics; uses trace_id for full-context lookup.

use crate::types::Severity;
use serde::{Deserialize, Serialize};

/// A single parsed diagnostic from any tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDiagnostic {
    pub trace_id: String,
    pub tool: String,
    pub severity: Severity,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub error_type: String,
    pub message: String,
    pub context_lines: Vec<String>,
    pub suggestion: Option<String>,
    pub confidence: f64,
    pub is_retryable: bool,
}

impl ParsedDiagnostic {
    /// Convert to a `Finding` with recovery hint attached.
    pub fn to_finding(&self) -> crate::types::Finding {
        let confidence = self.confidence;
        let mut finding = crate::types::Finding::new(
            &format!("{}-{}", self.tool, self.trace_id),
            self.severity,
            match self.tool.as_str() {
                "python3" => crate::types::Language::Python,
                "ruff" => crate::types::Language::Python,
                "pytest" => crate::types::Language::Python,
                "cargo" => crate::types::Language::Rust,
                "rustc" => crate::types::Language::Rust,
                "clang++" | "g++" | "clang" | "gcc" => crate::types::Language::Cpp,
                _ => crate::types::Language::Python,
            },
            &self.error_type,
            &self.message,
        );
        finding.file = self.file.clone().unwrap_or_default();
        finding.line = self.line;
        finding.column = self.column;
        finding.error_type = Some(self.error_type.clone());
        finding.context_lines = self.context_lines.clone();
        finding.trace_id = Some(self.trace_id.clone());
        finding.recovery_hint = Some(crate::types::RecoveryHint {
            action: self.suggestion.clone().unwrap_or_default(),
            fix: self.suggestion.clone(),
            confidence,
        });
        finding
    }
}

/// Parse Python compiler output (SyntaxError, etc.) into structured diagnostics.
pub fn parse_python_compiler(stderr: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Python SyntaxError pattern:
    //   File "/path/to/file.py", line 6, in <module>
    //     print("\nChoose a
    //           ^
    // SyntaxError: unterminated string literal (detected at line 6)
    let lines: Vec<&str> = stderr.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("File \"") && trimmed.contains(", line ") {
            // Extract file and line
            let file = extract_python_file(trimmed);
            let line_num = extract_python_line(trimmed);

            // Collect context lines (the offending code + caret)
            let mut context = Vec::new();
            if i + 1 < lines.len() {
                context.push(lines[i + 1].to_string());
            }
            if i + 2 < lines.len() {
                context.push(lines[i + 2].to_string());
            }

            // Look for the actual error message on subsequent lines
            let mut error_type = "SyntaxError".to_string();
            let mut message = String::new();
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.starts_with("SyntaxError:")
                    || l.starts_with("IndentationError:")
                    || l.starts_with("TabError:")
                {
                    error_type = l.split(':').next().unwrap_or("Error").to_string();
                    message = l[l.find(':').map(|p| p + 2).unwrap_or(0)..]
                        .trim()
                        .to_string();
                    break;
                }
                j += 1;
            }

            if message.is_empty() && i + 3 < lines.len() {
                message = lines[i + 3].trim().to_string();
            }

            let file_display = file.as_deref().unwrap_or("<unknown>").to_string();
            let line_display = line_num.map_or("?".to_string(), |n| n.to_string());
            let error_type_clone = error_type.clone();

            diagnostics.push(ParsedDiagnostic {
                trace_id: trace_id.to_string(),
                tool: "python3".to_string(),
                severity: Severity::Error,
                file,
                line: line_num,
                column: None,
                error_type,
                message,
                context_lines: context,
                suggestion: Some(format!(
                    "fix the {error_type_clone} at {file_display}:{line_display}"
                )),
                confidence: 0.95,
                is_retryable: false,
            });

            i = j + 1;
        } else {
            i += 1;
        }
    }

    diagnostics
}

/// Parse ruff output into structured diagnostics.
pub fn parse_ruff(output: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Ruff output format:
    // /path/to/file.py:10:5: E501 Line too long (88 > 88 characters)
    // /path/to/file.py:15:1: F401 'os' imported but unused
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        // Match: file:line:col: CODE message
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() >= 4 {
            let file = parts[0].trim().to_string();
            let line_num: Option<u32> = parts[1].trim().parse().ok();
            let col: Option<u32> = parts[2].trim().parse().ok();
            let rest = parts[3].trim();

            // Extract rule code (e.g., E501, F401)
            let rule_end = rest.find(' ').unwrap_or(rest.len());
            let error_type = rest[..rule_end].to_string();
            let message = rest[rule_end..].trim().to_string();
            let file_clone = file.clone();

            diagnostics.push(ParsedDiagnostic {
                trace_id: trace_id.to_string(),
                tool: "ruff".to_string(),
                severity: Severity::Warning,
                file: Some(file),
                line: line_num,
                column: col,
                error_type,
                message,
                context_lines: vec![line.to_string()],
                suggestion: Some(format!("run `ruff check --fix {file_clone}`")),
                confidence: 0.9,
                is_retryable: true,
            });
        }
    }

    diagnostics
}

/// Parse pytest output into structured diagnostics.
pub fn parse_pytest(output: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Pytest output format:
    //   /path/to/test_file.py::test_function FAILED
    //   assert expected == actual
    //   E   AssertionError: ...
    for line in output.lines() {
        if line.contains("FAILED") || line.contains("ERROR") {
            // Extract test node id
            let parts: Vec<&str> = line.split("::").collect();
            if parts.len() >= 2 {
                let file = parts[0].trim().to_string();
                let test_name = parts[1].split_whitespace().next().unwrap_or("").to_string();
                let file_clone = file.clone();

                diagnostics.push(ParsedDiagnostic {
                    trace_id: trace_id.to_string(),
                    tool: "pytest".to_string(),
                    severity: Severity::Error,
                    file: Some(file),
                    line: None,
                    column: None,
                    error_type: if line.contains("ERROR") {
                        "CollectionError".to_string()
                    } else {
                        "TestFailed".to_string()
                    },
                    message: format!("test {test_name} failed"),
                    context_lines: vec![line.to_string()],
                    suggestion: Some(format!("inspect the failure in {file_clone}::{test_name}")),
                    confidence: 0.85,
                    is_retryable: true,
                });
            }
        }
    }

    diagnostics
}

/// Parse generic compiler output (gcc, clang, rustc) into structured diagnostics.
pub fn parse_generic_compiler(output: &str, tool: &str, trace_id: &str) -> Vec<ParsedDiagnostic> {
    let mut diagnostics = Vec::new();

    // Generic format: file:line:col: error: message
    // or: file:line: warning: message
    for line in output.lines() {
        if line.contains("error:") || line.contains("warning:") || line.contains("Error:") {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            if parts.len() >= 4 {
                let file = parts[0].trim().to_string();
                let line_num: Option<u32> = parts[1].trim().parse().ok();
                let col: Option<u32> = parts[2].trim().parse().ok();
                let rest = parts[3].trim();

                let severity = if rest.to_lowercase().contains("error") {
                    Severity::Error
                } else {
                    Severity::Warning
                };

                let error_type = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("error")
                    .to_string();
                let message = rest[rest.find(' ').map(|p| p + 1).unwrap_or(0)..]
                    .trim()
                    .to_string();

                diagnostics.push(ParsedDiagnostic {
                    trace_id: trace_id.to_string(),
                    tool: tool.to_string(),
                    severity,
                    file: Some(file),
                    line: line_num,
                    column: col,
                    error_type,
                    message,
                    context_lines: vec![line.to_string()],
                    suggestion: None,
                    confidence: 0.8,
                    is_retryable: false,
                });
            }
        }
    }

    diagnostics
}

/// Attach recovery hints to parsed diagnostics using heuristics.
pub fn attach_recovery_hints(diagnostics: &mut [ParsedDiagnostic]) {
    for d in diagnostics.iter_mut() {
        if d.suggestion.is_none() {
            d.suggestion = Some(match d.tool.as_str() {
                "python3" => format!(
                    "review the {} at {}:{:?} and correct the source",
                    d.error_type,
                    d.file.as_deref().unwrap_or("<unknown>"),
                    d.line
                ),
                "ruff" => format!(
                    "run `ruff check --fix {}`",
                    d.file.as_deref().unwrap_or(".")
                ),
                "pytest" => format!(
                    "run `pytest {} -x` to see the full failure trace",
                    d.file.as_deref().unwrap_or(".")
                ),
                _ => "review the diagnostic and apply a manual fix".to_string(),
            });
        }
    }

    // Set confidence based on diagnostic quality (file+line precision)
    for d in diagnostics.iter_mut() {
        if d.file.is_some() && d.line.is_some() {
            d.confidence = d.confidence.max(0.9);
        } else if d.file.is_some() {
            d.confidence = d.confidence.max(0.6);
        } else {
            d.confidence = d.confidence.max(0.3);
        }
    }
}

fn extract_python_file(line: &str) -> Option<String> {
    // File "/path/to/file.py", line 6
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn extract_python_line(line: &str) -> Option<u32> {
    // , line 6,
    let marker = ", line ";
    let start = line.find(marker)? + marker.len();
    let end = line[start..].find(',')? + start;
    line[start..end].trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_python_syntax_error() {
        let stderr = r#"
  File "/home/user/test.py", line 6, in <module>
    print("\nChoose a
          ^
SyntaxError: unterminated string literal (detected at line 6)
"#;
        let diags = parse_python_compiler(stderr, "cs-test-1");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].tool, "python3");
        assert_eq!(diags[0].error_type, "SyntaxError");
        assert!(diags[0].message.contains("unterminated string literal"));
        assert_eq!(diags[0].file.as_deref(), Some("/home/user/test.py"));
        assert_eq!(diags[0].line, Some(6));
        assert!(!diags[0].context_lines.is_empty());
        assert_eq!(diags[0].trace_id, "cs-test-1");
    }

    #[test]
    fn parse_ruff_output() {
        let output = r#"
/home/user/src/main.py:10:5: E501 Line too long (88 > 88 characters)
/home/user/src/main.py:15:1: F401 'os' imported but unused
"#;
        let diags = parse_ruff(output, "cs-test-2");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].error_type, "E501");
        assert_eq!(diags[0].line, Some(10));
        assert_eq!(diags[0].column, Some(5));
        assert_eq!(diags[1].error_type, "F401");
        assert_eq!(diags[1].line, Some(15));
    }

    #[test]
    fn parse_pytest_failures() {
        let output = r#"
/home/user/test_foo.py::test_bar FAILED
/home/user/test_foo.py::test_baz ERROR
"#;
        let diags = parse_pytest(output, "cs-test-3");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].error_type, "TestFailed");
        assert!(diags[0].file.as_deref().unwrap().contains("test_foo.py"));
        assert_eq!(diags[1].error_type, "CollectionError");
    }

    #[test]
    fn attach_recovery_hints_fills_missing() {
        let mut diags = vec![ParsedDiagnostic {
            trace_id: "cs-test".to_string(),
            tool: "ruff".to_string(),
            severity: Severity::Warning,
            file: Some("src/main.py".to_string()),
            line: Some(10),
            column: Some(5),
            error_type: "E501".to_string(),
            message: "Line too long".to_string(),
            context_lines: vec![],
            suggestion: None,
            confidence: 0.5,
            is_retryable: true,
        }];
        attach_recovery_hints(&mut diags);
        assert!(diags[0].suggestion.is_some());
        assert!(
            diags[0]
                .suggestion
                .as_deref()
                .unwrap()
                .contains("ruff check --fix")
        );
    }
}
