//! Report generation: report.md for humans, report.json for tools/AI.

use crate::{
    Result,
    aci::make_trace_id,
    diagnostics,
    types::{AiTip, Finding, FindingStatus, Language, Report, Severity},
};
use std::path::Path;

/// Write report.md and report.json into `path`.
pub fn write(path: &Path, report: &Report) -> Result<()> {
    let md = render_md(report);
    let json = serde_json::to_string_pretty(report).map_err(crate::Error::Json)?;
    std::fs::write(path.join("report.md"), md)
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    std::fs::write(path.join("report.json"), json)
        .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
    Ok(())
}

pub fn render_md(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# codeguard report\n\n");
    out.push_str(&format!("Path: {}\n\n", report.path));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- tests passed: {}\n- findings: {}\n- benchmarks: {}\n- ai tips: {}\n\n",
        report.summary.tests_passed,
        report.summary.findings_total,
        report.summary.benchmarks_total,
        report.ai_tips.len()
    ));
    out.push_str("## Tool results\n\n");
    if report.tool_results.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str("| tool | status | duration_ms | skip_reason |\n");
        out.push_str("|------|--------|-------------|-------------|\n");
        for r in &report.tool_results {
            let status = if r.skipped {
                "skipped"
            } else if r.success() {
                "ok"
            } else {
                "failed"
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                r.tool,
                status,
                r.duration_ms,
                r.skip_reason.as_deref().unwrap_or("-")
            ));
        }
        out.push('\n');
    }
    out.push_str("## Findings\n\n");
    if report.findings.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str(
            "| id | severity | language | file | line | category | status | description |\n",
        );
        out.push_str(
            "|----|----------|----------|------|------|----------|--------|-------------|\n",
        );
        for f in &report.findings {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {:?} | {} | {} | {} |\n",
                f.id, f.severity, f.language, f.file, f.line, f.category, f.status, f.description
            ));
        }
        out.push('\n');
    }
    out.push_str("## Benchmarks\n\n");
    if report.benchmarks.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str("| name | before | after | unit | change_pct |\n");
        out.push_str("|------|--------|-------|------|------------|\n");
        for b in &report.benchmarks {
            out.push_str(&format!(
                "| {} | {:?} | {:?} | {} | {:?} |\n",
                b.name, b.before, b.after, b.unit, b.change_pct
            ));
        }
        out.push('\n');
    }
    out.push_str("## AI tips\n\n");
    if report.ai_tips.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        for (i, tip) in report.ai_tips.iter().enumerate() {
            out.push_str(&format!("### Tip {}\n\n", i + 1));
            out.push_str(&format!("- file: {}\n", tip.file));
            out.push_str(&format!("- line: {:?}\n", tip.line));
            out.push_str(&format!("- change: {}\n", tip.concrete_change));
            out.push_str(&format!("- why safe: {}\n", tip.why_safe));
            out.push_str(&format!("- verify: {}\n\n", tip.how_to_verify));
        }
    }
    out
}

/// Convert a tool result into a reported finding.
/// Attempts to parse structured diagnostics from tool output first;
/// falls back to raw output if parsing fails.
pub fn finding_from_result(
    id: &str,
    language: Language,
    result: &crate::types::ToolResult,
    category: &str,
) -> Finding {
    let severity = match result.exit_code {
        Some(0) => Severity::Info,
        Some(1) => Severity::Warning,
        Some(_) => Severity::Error,
        None => Severity::Critical,
    };
    let trace_id = make_trace_id("finding", None);

    // Try to parse structured diagnostics from tool output
    let diagnostic_text = if !result.stderr.trim().is_empty() {
        result.stderr.trim().to_string()
    } else if !result.stdout.trim().is_empty() {
        result.stdout.trim().to_string()
    } else {
        "(no diagnostic output)".to_string()
    };

    // Try tool-specific parsing first
    let parsed = match result.tool.as_str() {
        "python3" => {
            let diags = diagnostics::parse_python_compiler(&result.stderr, &trace_id);
            if !diags.is_empty() {
                Some(diags[0].to_finding())
            } else {
                None
            }
        }
        "ruff" => {
            let mut diags = diagnostics::parse_ruff(&result.stderr, &trace_id);
            diags.extend(diagnostics::parse_ruff(&result.stdout, &trace_id));
            if !diags.is_empty() {
                Some(diags[0].to_finding())
            } else {
                None
            }
        }
        "pytest" => {
            let diags = diagnostics::parse_pytest(&result.stderr, &trace_id);
            if !diags.is_empty() {
                Some(diags[0].to_finding())
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(mut f) = parsed {
        f.id = id.to_string();
        f.language = language;
        f.category = category.to_string();
        return f;
    }

    // Fallback to raw output
    Finding {
        id: id.to_string(),
        severity,
        language,
        file: String::new(),
        line: None,
        column: None,
        category: category.to_string(),
        error_type: None,
        status: crate::types::FindingStatus::Reported,
        description: diagnostic_text,
        fix: None,
        evidence: if !result.stdout.trim().is_empty() {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        },
        context_lines: Vec::new(),
        trace_id: Some(trace_id),
        recovery_hint: Some(crate::types::RecoveryHint {
            action: "review the diagnostic and apply a manual fix".to_string(),
            fix: None,
            confidence: 0.3,
        }),
    }
}

/// Build AI-actionable tips from reported findings. Each tip carries enough context
/// for another AI to apply the change without extra information.
pub fn ai_tips_for(findings: &[Finding]) -> Vec<AiTip> {
    findings
        .iter()
        .filter(|f| f.status != FindingStatus::Fixed)
        .map(|f| {
            let concrete_change = f
                .fix
                .clone()
                .unwrap_or_else(|| "review the diagnostic and apply a manual fix".to_string());
            let why_safe = if f.fix.is_some() {
                "the suggested action is the project's own lint/formatter fix; \
                 it is output-preserving by construction"
                    .to_string()
            } else {
                "no automatic fix is available for this finding; manual review required".to_string()
            };
            let how_to_verify = if f.file.is_empty() {
                "rerun the failing tool and `codeguard test <path>`; the finding \
                 should disappear or move to `fixed`"
                    .to_string()
            } else {
                format!(
                    "apply the change to {}:{:?}, then run `codeguard test <path>` \
                     and confirm tests still pass",
                    f.file, f.line
                )
            };
            AiTip {
                file: f.file.clone(),
                line: f.line,
                concrete_change,
                why_safe,
                how_to_verify,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_contains_sections() {
        let report = Report::new(".");
        let md = render_md(&report);
        assert!(md.contains("# codeguard report"));
        assert!(md.contains("## Summary"));
    }
}
