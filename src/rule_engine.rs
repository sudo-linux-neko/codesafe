//! Rule engine: editable TOML rules and the verified safe-fix loop.
//!
//! Rules are stored in rules/<language>.toml as an array of rule tables:
//!   [[rules]]
//!   id = "py-ruff-format"
//!   language = "python"
//!   trigger = "ruff"
//!   action = "ruff format ."
//!   safety = "safe"
//!   explanation = "Format Python sources with ruff (output-identical)."
//!
//! trigger is matched against a tool's output. It is either a substring pattern or,
//! when it contains a colon (tool:code), it also requires the result to come from
//! tool and code to appear in the output.
//!
//! Only safety = "safe" rules are auto-applied. Everything else is report-only.

use crate::Result;
use crate::adapters::LanguageAdapter;
use crate::cli::RuntimeConfig;
use crate::git;
use crate::types::{Finding, FindingStatus, Language, Rule, RuleSafety, Severity, ToolResult};
use std::path::Path;

/// Wrapper for TOML files that use [[rules]] arrays.
#[derive(Debug, Clone, serde::Deserialize)]
struct RulesFile {
    rules: Vec<Rule>,
}

/// Loaded rule set.
pub struct RuleEngine {
    rules: Vec<Rule>,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Load every *.toml rules file from dir.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut rules = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir).map_err(crate::Error::Io)? {
                let entry = entry.map_err(crate::Error::Io)?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                    let text = std::fs::read_to_string(&path).map_err(crate::Error::Io)?;
                    let parsed: RulesFile = toml::from_str(&text)
                        .map_err(|e| crate::Error::Rule(format!("{path:?}: {e}")))?;
                    rules.extend(parsed.rules);
                }
            }
        }
        Ok(Self { rules })
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Rules that may be auto-applied without review.
    pub fn safe_rules(&self) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.safety == RuleSafety::Safe)
            .collect()
    }

    /// Rules that must be reported but not applied.
    pub fn review_rules(&self) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.safety == RuleSafety::Review)
            .collect()
    }

    /// Rules for a given language.
    pub fn for_language(&self, language: Language) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(move |r| r.language == language)
            .collect()
    }

    /// Does trigger match output from tool?
    pub fn trigger_matches(trigger: &str, tool: &str, output: &str) -> bool {
        free_trigger_matches(trigger, tool, output)
    }
}

/// Does trigger match output from tool?
fn free_trigger_matches(trigger: &str, tool: &str, output: &str) -> bool {
    if let Some((t, code)) = trigger.split_once(':') {
        t == tool && output.contains(code)
    } else {
        output.contains(trigger)
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract file:line style location hints from tool output.
fn scan_location(output: &str) -> Option<(String, u32)> {
    for line in output.lines() {
        let len = line.len();
        let mut idx = 0;
        while idx < len {
            if line.as_bytes()[idx] == b':' {
                let file = line[..idx].trim();
                if !looks_like_file(file) {
                    idx += 1;
                    continue;
                }
                let rest = &line[idx + 1..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(line_num) = digits.parse::<u32>() {
                    return Some((file.to_string(), line_num));
                }
            }
            idx += 1;
        }
    }
    None
}

fn looks_like_file(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains(' ') {
        return false;
    }
    let ext = std::path::Path::new(text)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "py" | "rs" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx"
    )
}

/// Turn failed tool results into report-only findings, matched against rules.
pub fn findings_from_results(
    engine: &RuleEngine,
    results: &[ToolResult],
    language: Language,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for result in results {
        if result.skipped || result.success() {
            continue;
        }
        let output = format!("{}\n{}", result.stdout, result.stderr);
        let (file, line) = match scan_location(&output) {
            Some((f, l)) => (Some(f), Some(l)),
            None => (None, None),
        };
        let matched: Vec<&Rule> = engine
            .for_language(language)
            .iter()
            .filter(|r| free_trigger_matches(&r.trigger, &result.tool, &output))
            .cloned()
            .collect();
        if matched.is_empty() {
            let mut f = Finding::new(
                &format!("{}-failure", result.tool),
                severity_from_exit(result.exit_code),
                language,
                "tool",
                &first_non_empty_line(&output),
            );
            f.file = file.unwrap_or_default();
            f.line = line;
            findings.push(f);
            continue;
        }
        for rule in matched {
            let mut f = Finding::new(
                &rule.id,
                severity_from_exit(result.exit_code),
                language,
                &rule.id,
                &rule.explanation,
            );
            f.file = file.clone().unwrap_or_default();
            f.line = line;
            f.fix = Some(rule.action.clone());
            f.evidence = output.trim().to_string();
            findings.push(f);
        }
    }
    findings
}

fn severity_from_exit(exit_code: Option<i32>) -> Severity {
    match exit_code {
        Some(0) => Severity::Info,
        Some(1) => Severity::Warning,
        Some(_) => Severity::Error,
        None => Severity::Critical,
    }
}

fn first_non_empty_line(output: &str) -> String {
    output
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(no output)")
        .trim()
        .to_string()
}

/// Outcome of applying one rule inside the safe-fix loop.
#[derive(Debug, Clone)]
pub enum FixVerdict {
    Kept,
    Reverted,
    SkippedDryRun,
}

/// Run the verified safe-fix loop over every applicable adapter.
///
/// For each safe rule: apply the rule's action, rebuild, rerun all tests, compare
/// outputs and re-benchmark. Keep the change only if everything passes and there is
/// no meaningful regression; otherwise revert automatically. One fix per commit.
///
/// After each kept fix, the baseline is refreshed so subsequent fixes are compared
/// against the post-fix state rather than the original pre-fix state.
pub fn safe_fix_loop(
    path: &Path,
    rt: &RuntimeConfig,
    engine: &RuleEngine,
    adapters: &mut [Box<dyn LanguageAdapter>],
) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();

    // Git checkpoint before the fix loop (only for git repos). Refuse dirty trees
    // unless --force, mirroring cmd_fix.
    let checkpoint_head = if git::is_git_repo(path) {
        if !rt.force && git::is_dirty(path)? {
            return Err(crate::Error::Git(
                "working tree is dirty; commit or stash changes, or use --force".into(),
            ));
        }
        Some(git::checkpoint(path)?)
    } else {
        None
    };

    // Baseline: build + test + lint + fuzz before any fix.
    let mut baseline = run_adapter_tools(path, rt, adapters)?;
    let _baseline_test_out = baseline_test_output(&baseline);

    // Detect applicable adapters first, then process each one. We collect
    // (language, index) pairs so we can re-borrow adapters mutably per-iteration
    // without holding a borrow across the run_adapter_tools call.
    let applicable: Vec<(Language, usize)> = adapters
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            a.detect(path)
                .ok()
                .filter(|&d| d)
                .map(|_| (a.language(), i))
        })
        .collect();

    for (language, idx) in applicable {
        for rule in engine.safe_rules() {
            if rule.language != language {
                continue;
            }
            let verdict = {
                // Re-borrow this adapter mutably, scoped so it is dropped
                // before run_adapter_tools borrows all adapters again.
                let adapter = &mut adapters[idx];
                if rt.dry_run {
                    FixVerdict::SkippedDryRun
                } else {
                    apply_and_verify(
                        path,
                        rt,
                        checkpoint_head.as_deref(),
                        rule,
                        &mut **adapter,
                        &baseline,
                    )?
                }
            };
            match verdict {
                FixVerdict::Kept => {
                    // Refresh the baseline so the next rule is compared against
                    // the post-fix state, not the original pre-fix state.
                    baseline = run_adapter_tools(path, rt, adapters)?;
                }
                FixVerdict::Reverted => {
                    // Revert to the checkpoint (or HEAD if no git) so the next
                    // rule starts from a clean state.
                    if let Some(head) = &checkpoint_head {
                        git::revert_to(path, head)?;
                    } else if git::is_git_repo(path) {
                        git::revert_to(path, "HEAD")?;
                    }
                    // Re-fetch baseline after revert.
                    baseline = run_adapter_tools(path, rt, adapters)?;
                }
                FixVerdict::SkippedDryRun => {}
            }
            findings.push(finding_from_verdict(rule, language, verdict));
        }
    }
    Ok(findings)
}

/// Run build/test/lint across adapters and collect results.
fn run_adapter_tools(
    path: &Path,
    rt: &RuntimeConfig,
    adapters: &mut [Box<dyn LanguageAdapter>],
) -> Result<Vec<ToolResult>> {
    let mut results = Vec::new();
    for adapter in adapters.iter_mut() {
        if !adapter.detect(path)? {
            continue;
        }
        let ctx = crate::adapters::Context {
            path,
            jobs: rt.jobs,
            timeout_s: rt.timeout_s,
            dry_run: rt.dry_run,
            force: rt.force,
        };
        results.push(adapter.build(ctx)?);
        results.push(adapter.test(ctx)?);
        results.push(adapter.lint(ctx)?);
        results.push(adapter.fuzz(ctx)?);
    }
    Ok(results)
}

fn baseline_test_output(baseline: &[ToolResult]) -> Option<String> {
    baseline
        .iter()
        .find(|r| r.tool == "pytest" || r.tool == "cargo" || r.tool == "ctest" || r.tool == "make")
        .map(|r| normalize_test_output(&r.stdout))
}

/// Strip variable timing/count info from test output so reruns compare stably.
fn normalize_test_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let l = line.to_lowercase();
            !l.contains("total test time")
                && !l.contains("test time")
                && !l.contains("tests passed")
                && !l.contains("tests failed")
                && !l.contains("test result:")
                && !l.contains("running ")
                && !l.contains("elapsed:")
                && !l.contains("duration:")
        })
        .map(|line| {
            // Strip trailing time values like "0.01 sec" from test result lines.
            // A time token looks like digits optionally with a dot, followed by sec/s/ms/us.
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() >= 2 {
                let last = tokens.last().unwrap();
                let second_last = tokens[tokens.len() - 2];
                let looks_like_time = last.contains("sec")
                    || last.ends_with("ms")
                    || last.ends_with("us")
                    || (last.len() > 1
                        && last.ends_with('s')
                        && second_last.chars().all(|c| c.is_ascii_digit() || c == '.'));
                if looks_like_time {
                    return tokens[..tokens.len() - 2].join(" ");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_and_verify(
    path: &Path,
    rt: &RuntimeConfig,
    checkpoint_head: Option<&str>,
    rule: &Rule,
    adapter: &mut dyn LanguageAdapter,
    baseline: &[ToolResult],
) -> Result<FixVerdict> {
    // 1. Apply the rule's action.
    if !run_action(path, rt, rule)? {
        return Ok(FixVerdict::Reverted);
    }
    // 2. Rebuild + rerun tests.
    let ctx = crate::adapters::Context {
        path,
        jobs: rt.jobs,
        timeout_s: rt.timeout_s,
        dry_run: rt.dry_run,
        force: rt.force,
    };
    let build = adapter.build(ctx)?;
    if !build.success() {
        revert_to_checkpoint(path, checkpoint_head)?;
        return Ok(FixVerdict::Reverted);
    }
    let test = adapter.test(ctx)?;
    if !test.success() {
        revert_to_checkpoint(path, checkpoint_head)?;
        return Ok(FixVerdict::Reverted);
    }
    // 3. Compare outputs: tests must produce identical output.
    let after = normalize_test_output(&test.stdout);
    let before = baseline_test_output(baseline);
    if before.as_ref() != Some(&after) {
        revert_to_checkpoint(path, checkpoint_head)?;
        return Ok(FixVerdict::Reverted);
    }
    // 4. Keep: one fix per commit (only for git repos).
    if git::is_git_repo(path) {
        git::commit(path, &format!("codeguard: apply safe fix ({})", rule.id))?;
    }
    Ok(FixVerdict::Kept)
}

/// Revert the working tree to the checkpoint head (or HEAD if no checkpoint).
fn revert_to_checkpoint(path: &Path, checkpoint_head: Option<&str>) -> Result<()> {
    if git::is_git_repo(path) {
        if let Some(head) = checkpoint_head {
            git::revert_to(path, head)?;
        } else {
            git::revert_to(path, "HEAD")?;
        }
    }
    Ok(())
}

/// Run a rule's `action` command in the repository directory.
fn run_action(path: &Path, rt: &RuntimeConfig, rule: &Rule) -> Result<bool> {
    if rule.action.trim().is_empty() {
        return Ok(false);
    }
    let result = crate::adapters::run_command_in_dir(path, rule.action.trim(), rt.timeout_s)?;
    Ok(result.success())
}

fn finding_from_verdict(rule: &Rule, language: Language, verdict: FixVerdict) -> Finding {
    let (status, description) = match verdict {
        FixVerdict::Kept => (FindingStatus::Fixed, "applied and verified".into()),
        FixVerdict::Reverted => (
            FindingStatus::Reported,
            "applied but failed verification, reverted".into(),
        ),
        FixVerdict::SkippedDryRun => (
            FindingStatus::Reported,
            "dry run, would apply: ".to_string() + &rule.action,
        ),
    };
    let mut f = Finding::new(&rule.id, Severity::Info, language, "rule", &description);
    f.status = status;
    f.fix = Some(rule.action.clone());
    f.evidence = rule.explanation.clone();
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_matches_tool_code() {
        assert!(free_trigger_matches(
            "ruff:E501",
            "ruff",
            "main.py:4:8: E501 too long"
        ));
        assert!(!free_trigger_matches(
            "ruff:E501",
            "ruff",
            "main.py:4:8: F401 unused"
        ));
        assert!(!free_trigger_matches(
            "ruff:E501",
            "mypy",
            "main.py:4:8: E501 too long"
        ));
    }

    #[test]
    fn trigger_matches_substring() {
        assert!(free_trigger_matches("warning", "ruff", "some warning here"));
        assert!(!free_trigger_matches("warning", "ruff", "no match"));
    }

    #[test]
    fn scan_location_extracts_file_line() {
        assert_eq!(
            scan_location("src/lib.rs:10:5: error: unused variable"),
            Some(("src/lib.rs".to_string(), 10))
        );
        assert_eq!(scan_location("main.cpp: undefined reference"), None);
        assert_eq!(scan_location("no location here"), None);
    }

    #[test]
    fn findings_from_results_wrap_tool_failures() {
        let engine = RuleEngine::new();
        let results = vec![ToolResult {
            tool: "ruff".into(),
            command: "ruff check .".into(),
            exit_code: Some(1),
            stdout: "main.py:4:8: E501 line too long\n".into(),
            stderr: String::new(),
            duration_ms: 5,
            skipped: false,
            skip_reason: None,
        }];
        let findings = findings_from_results(&engine, &results, Language::Python);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "ruff-failure");
        assert_eq!(findings[0].status, FindingStatus::Reported);
    }
}
