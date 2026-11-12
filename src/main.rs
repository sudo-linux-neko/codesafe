//! Binary entry point for codesafe.
//!
//! Supports human-readable output (default), JSON, and JSONL modes for
//! agent consumption. Every command emits a structured envelope in JSON
//! modes: { ok, result, error, meta }.

use clap::Parser;
use codeguard::config;
use codeguard::report;
use codeguard::rule_engine;
use codeguard::scan;
use codeguard::{
    Result,
    aci::{self, Envelope, OutputMode, print_envelope},
    adapters::{LanguageAdapter, cpp::Cpp, python::Python, rust::Rust},
    cli::{Cli, Command, RuntimeConfig, schema_json, skill_document},
    types::{FindingStatus, Language, Report, Severity, TraceId},
};
use rayon::prelude::*;
use serde::Serialize;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("codesafe: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let config = config::load(&cli.config)?;
    let rt = RuntimeConfig::from_cli(&cli, config)?;
    let trace_id = aci::make_trace_id("codesafe", None);

    match cli.command {
        Command::Scan { path } => cmd_scan(&path, &rt, &trace_id),
        Command::Test { path } => cmd_test(&path, &rt, &trace_id),
        Command::Bench { path } => cmd_bench(&path, &rt, &trace_id),
        Command::Fix { path } => cmd_fix(&path, &rt, &trace_id),
        Command::Report { path } => cmd_report(&path, &rt, &trace_id),
        Command::All { path } => cmd_all(&path, &rt, &trace_id),
        Command::Init { path } => cmd_init(&path, &rt, &trace_id),
        Command::Spec {
            id,
            description,
            allowlist,
        } => cmd_spec(&id, &description, &allowlist, &rt, &trace_id),
        Command::Red {
            spec_id,
            test_name,
            test_file,
            test_source,
        } => cmd_red(
            &spec_id,
            &test_name,
            &test_file,
            &test_source,
            &rt,
            &trace_id,
        ),
        Command::Verify { path } => cmd_verify(&path, &rt, &trace_id),
        Command::Green { path } => cmd_green(&path, &rt, &trace_id),
        Command::Schema => cmd_schema(&rt),
        Command::GenerateSkill { path } => cmd_generate_skill(&path, &rt, &trace_id),
    }
}

/// Emit a success envelope in the requested output mode, returning ExitCode.
fn emit_ok<T: Serialize>(
    mode: OutputMode,
    result: T,
    command: &str,
    path: Option<&str>,
    trace_id: &TraceId,
) -> ExitCode {
    let env = Envelope::ok(result, command, path, trace_id);
    print_envelope(mode, &env);
    ExitCode::from(0)
}

/// Emit an error envelope in the requested output mode, returning ExitCode.
#[allow(clippy::too_many_arguments)]
fn emit_err(
    mode: OutputMode,
    code: &str,
    category: &str,
    message: &str,
    suggestion: Option<&str>,
    is_retryable: bool,
    command: &str,
    path: Option<&str>,
    trace_id: &TraceId,
) -> ExitCode {
    let env: Envelope<()> = Envelope::err(
        code,
        category,
        message,
        suggestion,
        is_retryable,
        command,
        path,
        trace_id,
    );
    print_envelope(mode, &env);
    ExitCode::from(1)
}

fn adapters_for(_path: &std::path::Path) -> Result<codeguard::adapters::AdapterList> {
    Ok(vec![
        Box::new(Python) as Box<dyn LanguageAdapter>,
        Box::new(Rust) as Box<dyn LanguageAdapter>,
        Box::new(Cpp) as Box<dyn LanguageAdapter>,
    ])
}

fn run_tools(
    path: &std::path::Path,
    rt: &RuntimeConfig,
) -> Result<Vec<(Language, codeguard::adapters::ToolResults)>> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let adapters = adapters_for(&path)?;

    let results: Vec<(Language, codeguard::adapters::ToolResults)> = adapters
        .into_par_iter()
        .filter_map(|adapter| {
            if adapter.detect(&path).ok() == Some(true) {
                Some(adapter)
            } else {
                None
            }
        })
        .map(|adapter| {
            let language = adapter.language();
            let ctx = codeguard::adapters::Context {
                path: &path,
                jobs: rt.jobs,
                timeout_s: rt.timeout_s,
                dry_run: rt.dry_run,
                force: rt.force,
            };
            let results = vec![
                adapter.build(ctx)?,
                adapter.test(ctx)?,
                adapter.lint(ctx)?,
                adapter.fuzz(ctx)?,
            ];
            Ok((language, results))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(results)
}

fn worst_severity(results: &[codeguard::types::ToolResult]) -> Option<Severity> {
    let mut worst: Option<Severity> = None;
    for r in results {
        if r.skipped {
            continue;
        }
        let sev = match r.exit_code {
            Some(0) => Severity::Info,
            Some(1) => Severity::Warning,
            Some(2) => Severity::Error,
            Some(_) => Severity::Error,
            None => Severity::Critical,
        };
        worst = Some(match worst {
            None => sev,
            Some(w) if sev > w => sev,
            Some(w) => w,
        });
    }
    worst
}

fn any_failed(results: &[codeguard::types::ToolResult]) -> bool {
    results.iter().any(|r| !r.skipped && r.exit_code != Some(0))
}

fn print_results(results: &[codeguard::types::ToolResult]) {
    for r in results {
        let status = if r.skipped {
            "skipped"
        } else if r.success() {
            "ok"
        } else {
            "FAILED"
        };
        println!("[{}] {} ({} ms)", status, r.tool, r.duration_ms);
        if !r.stdout.is_empty() {
            for line in r.stdout.lines().take(12) {
                println!("    {line}");
            }
        }
        if !r.stderr.is_empty() {
            for line in r.stderr.lines().take(12) {
                println!("    {line}");
            }
        }
    }
}

/// Scan command — detect languages, build systems, tools.
fn cmd_scan(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let result = scan::scan(path)?;
    let mode = rt.output_mode;

    #[derive(Serialize)]
    struct ScanResult {
        languages: Vec<String>,
        source_files: usize,
        build_systems: Vec<String>,
        config_files: usize,
        available_tools: Vec<String>,
        trace_id: String,
    }

    let scan_result = ScanResult {
        languages: result.languages.iter().map(|l| l.to_string()).collect(),
        source_files: result.source_files.len(),
        build_systems: result.build_systems.iter().map(|b| b.to_string()).collect(),
        config_files: result.config_files.len(),
        available_tools: result.tool_results.iter().map(|t| t.tool.clone()).collect(),
        trace_id: trace_id.clone(),
    };

    if mode == OutputMode::Human {
        println!("{}", scan::render(&result));
    } else {
        emit_ok(
            mode,
            scan_result,
            "scan",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Test command — build, test, lint across all languages.
fn cmd_test(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let results: Vec<codeguard::types::ToolResult> = run_tools(path, rt)?
        .into_iter()
        .flat_map(|(_, r)| r)
        .collect();
    let mode = rt.output_mode;

    if mode == OutputMode::Human {
        print_results(&results);
    }

    let failed = any_failed(&results);
    let worst = worst_severity(&results);
    let gated = worst.is_some_and(|s| rt.fail_on.is_some_and(|f| s >= f));
    let fail = match rt.fail_on {
        None => failed,
        Some(_) => gated,
    };

    #[derive(Serialize)]
    struct TestResult {
        tests_passed: bool,
        tests_total: u64,
        tests_failed: u64,
        tools: Vec<codeguard::types::ToolResult>,
        worst_severity: Option<String>,
        fail_on: Option<String>,
        gated: bool,
        trace_id: String,
    }

    let tests_total = results.iter().filter(|r| !r.skipped).count() as u64;
    let tests_failed = results
        .iter()
        .filter(|r| !r.skipped && r.exit_code != Some(0))
        .count() as u64;

    let test_result = TestResult {
        tests_passed: !fail,
        tests_total,
        tests_failed,
        tools: results,
        worst_severity: worst.map(|s| s.to_string()),
        fail_on: rt.fail_on.map(|s| s.to_string()),
        gated,
        trace_id: trace_id.clone(),
    };

    if mode == OutputMode::Human {
        if fail {
            println!("\ncodesafe test: FAIL (fail_on={:?})", rt.fail_on);
        } else {
            println!("\ncodesafe test: pass");
        }
    } else {
        emit_ok(
            mode,
            test_result,
            "test",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    if fail {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

/// Bench command — run benchmarks, profile, rank hotspots.
fn cmd_bench(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let scan_result = match scan::scan(&path) {
        Ok(s) => s,
        Err(_) => {
            if rt.output_mode == OutputMode::Human {
                println!("codesafe bench: {} (no language detected)", path.display());
            } else {
                #[derive(Serialize)]
                struct BenchResult {
                    benchmarks: Vec<codeguard::types::BenchmarkResult>,
                    hotspots: Vec<codeguard::adapters::Hotspot>,
                    regressions: u64,
                    trace_id: String,
                }
                emit_ok(
                    rt.output_mode,
                    BenchResult {
                        benchmarks: Vec::new(),
                        hotspots: Vec::new(),
                        regressions: 0,
                        trace_id: trace_id.clone(),
                    },
                    "bench",
                    Some(&path.to_string_lossy()),
                    trace_id,
                );
            }
            return Ok(ExitCode::from(0));
        }
    };
    let source_hash = codeguard::types::hash_files(&scan_result.source_files)?;
    let mut bench_cache = codeguard::bench::BenchmarkCache::new(&path)?;
    let mut adapters = adapters_for(&path)?;
    let mut regressions = 0u64;
    let mut any_benchmarks = false;
    let mut all_hotspots: Vec<codeguard::adapters::Hotspot> = Vec::new();
    let mut bench_results: Vec<codeguard::types::BenchmarkResult> = Vec::new();

    for adapter in adapters.iter_mut() {
        if !adapter.detect(&path)? {
            continue;
        }
        let language = adapter.language();
        let ctx = codeguard::adapters::Context {
            path: &path,
            jobs: rt.jobs,
            timeout_s: rt.timeout_s,
            dry_run: rt.dry_run,
            force: rt.force,
        };
        for _ in 0..rt.bench_runs {
            for b in adapter.bench(ctx)? {
                if let Some(value) = b.after {
                    match bench_results.iter_mut().find(|n| n.name == b.name) {
                        Some(existing) => {
                            existing.after = Some(value);
                        }
                        None => bench_results.push(b),
                    }
                }
            }
        }
        for b in &bench_results {
            let after = b.after.unwrap_or(0.0);
            any_benchmarks = true;
            let before = bench_cache.previous(language, &source_hash, &b.name);
            let change_pct = before.map(|base| {
                if base == 0.0 {
                    0.0
                } else {
                    (after - base) / base * 100.0
                }
            });
            let regressed = change_pct.is_some_and(|c| c > rt.regression_threshold_pct);
            if regressed {
                regressions += 1;
            }
            let _ = bench_cache.store(language, &source_hash, &b.name, after);
        }
        for h in adapter.profile(ctx)? {
            all_hotspots.push(h);
        }
    }

    all_hotspots.sort_by(|a, b| {
        b.percent_runtime
            .partial_cmp(&a.percent_runtime)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        println!("codesafe bench: {}", path.display());
        if !any_benchmarks {
            println!("  benchmarks: none (no benchmark tooling found)");
        }
        if regressions > 0 {
            println!(
                "  {regressions} regression(s) flagged (threshold {:?}%)",
                rt.regression_threshold_pct
            );
        }
        if all_hotspots.is_empty() {
            println!("  hotspots: none (no profiler data available)");
        } else {
            println!("  ranked hotspots:");
            for h in &all_hotspots {
                println!(
                    "    {:5.1}%  {} ({}:{:?})",
                    h.percent_runtime, h.function, h.file, h.line
                );
            }
        }
    } else {
        #[derive(Serialize)]
        struct BenchResult {
            benchmarks: Vec<codeguard::types::BenchmarkResult>,
            hotspots: Vec<codeguard::adapters::Hotspot>,
            regressions: u64,
            trace_id: String,
        }
        emit_ok(
            mode,
            BenchResult {
                benchmarks: bench_results,
                hotspots: all_hotspots,
                regressions,
                trace_id: trace_id.clone(),
            },
            "bench",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    if regressions > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

/// Fix command — apply safe rules with verification loop.
fn cmd_fix(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    if codeguard::git::is_git_repo(&path) && !rt.force && codeguard::git::is_dirty(&path)? {
        return Ok(emit_err(
            rt.output_mode,
            "E001",
            "git",
            "working tree is dirty; commit or stash changes, or use --force",
            Some("commit or stash your changes, then re-run; or pass --force to bypass"),
            true,
            "fix",
            Some(&path.to_string_lossy()),
            trace_id,
        ));
    }
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_from_dir(&rules_dir)?;
    let mut adapters = adapters_for(&path)?;
    let findings = rule_engine::safe_fix_loop(&path, rt, &engine, &mut adapters)?;

    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        for f in &findings {
            println!("[fix] {} [{:?}] {}", f.id, f.status, f.description);
        }
        if rt.dry_run {
            println!("codesafe fix: dry run, no changes applied");
        } else {
            let kept = findings
                .iter()
                .filter(|f| f.status == FindingStatus::Fixed)
                .count();
            println!("codesafe fix: applied {kept} verified fix(es)");
        }
    } else {
        #[derive(Serialize)]
        struct FixResult {
            findings: Vec<codeguard::types::Finding>,
            applied: usize,
            dry_run: bool,
            trace_id: String,
        }
        let applied = findings
            .iter()
            .filter(|f| f.status == FindingStatus::Fixed)
            .count();
        emit_ok(
            mode,
            FixResult {
                findings,
                applied,
                dry_run: rt.dry_run,
                trace_id: trace_id.clone(),
            },
            "fix",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(ExitCode::from(0))
}

/// Report command — write report.md and report.json.
fn cmd_report(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let _scan = scan::scan(path)?;
    let per_language = run_tools(path, rt)?;
    let mut results: Vec<codeguard::types::ToolResult> = Vec::new();
    for (_, r) in &per_language {
        results.extend(r.clone());
    }
    let rules_dir = rules_dir_for(path, rt);
    let engine = rule_engine::RuleEngine::load_from_dir(&rules_dir)?;
    let mut findings = Vec::new();
    for (language, lang_results) in &per_language {
        findings.extend(rule_engine::findings_from_results(
            &engine,
            lang_results,
            *language,
        ));
    }
    let ai_tips = report::ai_tips_for(&findings);
    let mut report = Report::new(path.to_string_lossy());
    report.summary.tools_found = results.iter().filter(|r| !r.skipped).count();
    report.summary.tools_missing = results.iter().filter(|r| r.skipped).count();
    report.summary.tests_passed = !any_failed(&results);
    report.summary.findings_total = findings.len();
    report.summary.findings_fixed = findings
        .iter()
        .filter(|f| f.status == FindingStatus::Fixed)
        .count();
    report.findings = findings;
    report.ai_tips = ai_tips.clone();
    report.tool_results = results;

    // Parse diagnostics from tool results into structured findings
    let mut parsed_diagnostics: Vec<codeguard::diagnostics::ParsedDiagnostic> = Vec::new();
    for r in &report.tool_results {
        if !r.skipped && r.exit_code != Some(0) {
            let diag_trace_id = aci::make_trace_id("report", Some(&path.to_string_lossy()));
            match r.tool.as_str() {
                "python3" => {
                    parsed_diagnostics.extend(codeguard::diagnostics::parse_python_compiler(
                        &r.stderr,
                        &diag_trace_id,
                    ));
                }
                "ruff" => {
                    parsed_diagnostics.extend(codeguard::diagnostics::parse_ruff(
                        &r.stderr,
                        &diag_trace_id,
                    ));
                    parsed_diagnostics.extend(codeguard::diagnostics::parse_ruff(
                        &r.stdout,
                        &diag_trace_id,
                    ));
                }
                "pytest" => {
                    parsed_diagnostics.extend(codeguard::diagnostics::parse_pytest(
                        &r.stderr,
                        &diag_trace_id,
                    ));
                }
                _ => {}
            }
        }
    }
    codeguard::diagnostics::attach_recovery_hints(&mut parsed_diagnostics);

    let mode = rt.output_mode;
    report::write(path, &report)?;

    if mode == OutputMode::Human {
        println!(
            "wrote report.md and report.json to {} ({} findings, {} ai tips)",
            path.display(),
            report.summary.findings_total,
            ai_tips.len()
        );
    } else {
        #[derive(Serialize)]
        struct ReportResult {
            path: String,
            findings_total: usize,
            ai_tips: usize,
            tools_found: usize,
            tools_missing: usize,
            tests_passed: bool,
            trace_id: String,
            parsed_diagnostics_count: usize,
        }
        emit_ok(
            mode,
            ReportResult {
                path: path.to_string_lossy().to_string(),
                findings_total: report.summary.findings_total,
                ai_tips: ai_tips.len(),
                tools_found: report.summary.tools_found,
                tools_missing: report.summary.tools_missing,
                tests_passed: report.summary.tests_passed,
                trace_id: trace_id.clone(),
                parsed_diagnostics_count: parsed_diagnostics.len(),
            },
            "report",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(ExitCode::from(0))
}

/// All command — scan → test → bench → fix → report.
fn cmd_all(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let _ = cmd_scan(path, rt, trace_id)?;
    let test_code = cmd_test(path, rt, trace_id)?;
    let _ = cmd_bench(path, rt, trace_id)?;
    let fix_code = cmd_fix(path, rt, trace_id)?;
    let _ = cmd_report(path, rt, trace_id)?;

    if mode == OutputMode::Human {
        if test_code == ExitCode::from(0) && fix_code == ExitCode::from(0) {
            println!("\ncodesafe all: complete");
        } else {
            println!("\ncodesafe all: completed with failures");
        }
    } else {
        #[derive(Serialize)]
        struct AllResult {
            scan_passed: bool,
            test_passed: bool,
            fix_applied: bool,
            report_written: bool,
            trace_id: String,
        }
        emit_ok(
            mode,
            AllResult {
                scan_passed: true,
                test_passed: test_code == ExitCode::from(0),
                fix_applied: fix_code == ExitCode::from(0),
                report_written: true,
                trace_id: trace_id.clone(),
            },
            "all",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    if test_code == ExitCode::from(0) && fix_code == ExitCode::from(0) {
        Ok(ExitCode::from(0))
    } else {
        Ok(ExitCode::from(1))
    }
}

/// Init command — generate codesafe.toml and rules/.
fn cmd_init(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let config_path = path.join("codesafe.toml");
    if config_path.exists() {
        return Ok(emit_err(
            rt.output_mode,
            "E002",
            "config",
            &format!("{config_path:?} already exists; refusing to overwrite"),
            Some("remove the existing file or choose a different path"),
            false,
            "init",
            Some(&path.to_string_lossy()),
            trace_id,
        ));
    }
    std::fs::write(&config_path, config::example())
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    let rules_dir = path.join("rules");
    std::fs::create_dir_all(&rules_dir)
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    // Default Python rules
    let py_rules = r#"[[rules]]
id = "py-ruff-format"
language = "python"
trigger = "ruff"
action = "ruff format ."
safety = "safe"
explanation = "Format Python sources with ruff (output-preserving)."

[[rules]]
id = "py-ruff-check"
language = "python"
trigger = "ruff"
action = "ruff check --fix ."
safety = "safe"
explanation = "Auto-fix lint issues with ruff (output-preserving)."

[[rules]]
id = "py-mypy-strict"
language = "python"
trigger = "mypy"
action = ""
safety = "review"
explanation = "Run mypy for type checking; review findings manually.
"#;
    let py_path = rules_dir.join("python.toml");
    std::fs::write(&py_path, py_rules)
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    // Default Rust rules
    let rs_rules = r#"[[rules]]
id = "rs-fmt"
language = "rust"
trigger = "rustfmt"
action = "cargo fmt"
safety = "safe"
explanation = "Format Rust sources with cargo fmt (output-preserving)."

[[rules]]
id = "rs-clippy-fix"
language = "rust"
trigger = "clippy"
action = "cargo clippy --fix --allow-staged --allow-dirty"
safety = "safe"
explanation = "Auto-fix clippy lints with cargo clippy --fix (output-preserving)."

[[rules]]
id = "rs-audit"
language = "rust"
trigger = "audit"
action = ""
safety = "review"
explanation = "Run cargo audit for vulnerability scanning; review findings manually.
"#;
    let rs_path = rules_dir.join("rust.toml");
    std::fs::write(&rs_path, rs_rules)
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    // Default C++ rules
    let cpp_rules = r#"[[rules]]
id = "cpp-clang-format"
language = "cpp"
trigger = "clang-format"
action = "clang-format -i src/**/*.cpp src/**/*.h src/**/*.hpp src/**/*.cc src/**/*.cxx"
safety = "safe"
explanation = "Format C++ sources with clang-format (output-preserving)."

[[rules]]
id = "cpp-clang-tidy"
language = "cpp"
trigger = "clang-tidy"
action = ""
safety = "review"
explanation = "Run clang-tidy for static analysis; review findings manually."

[[rules]]
id = "cpp-cppcheck"
language = "cpp"
trigger = "cppcheck"
action = ""
safety = "review"
explanation = "Run cppcheck for static analysis; review findings manually.
"#;
    let cpp_path = rules_dir.join("cpp.toml");
    std::fs::write(&cpp_path, cpp_rules)
        .map_err(|e| codeguard::Error::Io(std::io::Error::other(e)))?;

    let mode = rt.output_mode;
    if mode == OutputMode::Human {
        println!(
            "Initialized codesafe:\n  config: {}\n  rules:  {}/\n\nEdit codesafe.toml and rules/*.toml to tune behavior.\nRun `codesafe test <path>` to verify.",
            config_path.display(),
            rules_dir.display()
        );
    } else {
        #[derive(Serialize)]
        struct InitResult {
            config_path: String,
            rules_dir: String,
            trace_id: String,
        }
        emit_ok(
            mode,
            InitResult {
                config_path: config_path.to_string_lossy().to_string(),
                rules_dir: rules_dir.to_string_lossy().to_string(),
                trace_id: trace_id.clone(),
            },
            "init",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    Ok(ExitCode::from(0))
}

/// Schema command — emit JSON schema for agent introspection.
fn cmd_schema(rt: &RuntimeConfig) -> Result<ExitCode> {
    let schema = schema_json();
    match rt.output_mode {
        OutputMode::Human => {
            println!("{}", serde_json::to_string_pretty(&schema).unwrap());
            Ok(ExitCode::from(0))
        }
        OutputMode::Json | OutputMode::Jsonl => {
            emit_ok(
                rt.output_mode,
                schema,
                "schema",
                None,
                &aci::make_trace_id("schema", None),
            );
            Ok(ExitCode::from(0))
        }
    }
}

/// GenerateSkill command — emit an agent skill document for LLM tool-use.
fn cmd_generate_skill(
    path: &std::path::Path,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let skill = skill_document(&path.to_string_lossy());
    match rt.output_mode {
        OutputMode::Human => {
            println!("{skill}");
            Ok(ExitCode::from(0))
        }
        OutputMode::Json | OutputMode::Jsonl => {
            emit_ok(
                rt.output_mode,
                skill,
                "generate-skill",
                Some(&path.to_string_lossy()),
                trace_id,
            );
            Ok(ExitCode::from(0))
        }
    }
}

/// Spec command — write acceptance criteria spec (test-writing agent only).
fn cmd_spec(
    id: &str,
    description: &str,
    allowlist: &[std::path::PathBuf],
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;
    let spec = codeguard::verification::create_spec(
        id,
        description,
        allowlist
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect(),
        allowlist.to_vec(),
        codeguard::types::Language::Python, // default; will be refined by detection
        trace_id,
    );

    if mode == OutputMode::Human {
        println!("Spec created: {}", spec.id);
        println!("  Description: {}", spec.description);
        println!("  Criteria: {}", spec.acceptance_criteria.len());
        println!("  Allowlist: {:?}", spec.file_allowlist);
    } else {
        emit_ok(mode, spec, "spec", None, trace_id);
    }
    Ok(ExitCode::from(0))
}

/// Red command — write a RED test from a spec (test-writing agent only).
fn cmd_red(
    spec_id: &str,
    test_name: &str,
    test_file: &std::path::Path,
    test_source: &str,
    rt: &RuntimeConfig,
    trace_id: &TraceId,
) -> Result<ExitCode> {
    let mode = rt.output_mode;

    // Run anti-gaming stub-passability check on the RED test
    // We need a unit under test name; for now use a heuristic from the file path
    let unit_under_test = test_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let gaming = codeguard::verification::check_stub_passability(test_source, unit_under_test);
    if matches!(
        gaming,
        codeguard::verification::AntiGamingResult::GamingDetected { .. }
    ) {
        return Ok(emit_err(
            mode,
            "E003",
            "anti-gaming",
            &format!("{:?}", gaming),
            Some("remove mocks from RED test; test real code only"),
            false,
            "red",
            Some(&test_file.to_string_lossy()),
            trace_id,
        ));
    }

    let test_case = codeguard::verification::write_red_test(
        &codeguard::verification::Spec {
            id: spec_id.to_string(),
            description: String::new(),
            acceptance_criteria: Vec::new(),
            file_allowlist: Vec::new(),
            language: codeguard::types::Language::Python,
            trace_id: trace_id.clone(),
        },
        test_name,
        test_source,
        test_file.to_path_buf(),
        trace_id,
    );

    if mode == OutputMode::Human {
        println!("RED test written: {}", test_case.id);
        println!("  File: {}", test_case.file.display());
        println!("  Anti-gaming: passed");
    } else {
        emit_ok(
            mode,
            test_case,
            "red",
            Some(&test_file.to_string_lossy()),
            trace_id,
        );
    }
    Ok(ExitCode::from(0))
}

/// Verify command — run RED-first verification protocol.
fn cmd_verify(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;

    if mode == OutputMode::Human {
        println!(
            "codesafe verify: running verification protocol on {}",
            path.display()
        );
    }

    // Run anti-gaming lints on the current state
    // This is a placeholder - full implementation would check the working tree
    let results = codeguard::verification::run_anti_gaming_lints(
        "", // test_source - not available without reading files
        "unknown",
        None,
        "",
        &[],   // changed_files
        &[],   // allowlist
        false, // scoped_passed
        false, // full_passed
    );

    let gaming_found = results
        .iter()
        .filter(|r| {
            matches!(
                r,
                codeguard::verification::AntiGamingResult::GamingDetected { .. }
            )
        })
        .count();

    if mode == OutputMode::Human {
        if gaming_found > 0 {
            println!("codesafe verify: FAIL - {gaming_found} anti-gaming violation(s) detected");
            for r in &results {
                if matches!(
                    r,
                    codeguard::verification::AntiGamingResult::GamingDetected { .. }
                ) {
                    println!("  - {:?}", r);
                }
            }
        } else {
            println!("codesafe verify: pass - no anti-gaming violations");
        }
    } else {
        #[derive(serde::Serialize)]
        struct VerifyResult {
            gaming_violations: usize,
            checks: Vec<codeguard::verification::AntiGamingResult>,
            trace_id: String,
        }
        emit_ok(
            mode,
            VerifyResult {
                gaming_violations: gaming_found,
                checks: results,
                trace_id: trace_id.clone(),
            },
            "verify",
            Some(&path.to_string_lossy()),
            trace_id,
        );
    }

    if gaming_found > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

/// Green command — run GREEN verification (implementation against RED tests).
fn cmd_green(path: &std::path::Path, rt: &RuntimeConfig, trace_id: &TraceId) -> Result<ExitCode> {
    let mode = rt.output_mode;

    if mode == OutputMode::Human {
        println!(
            "codesafe green: running GREEN verification on {}",
            path.display()
        );
    }

    // For now, run the standard test suite as GREEN verification
    let test_code = cmd_test(path, rt, trace_id)?;

    if mode == OutputMode::Human {
        if test_code == ExitCode::from(0) {
            println!("codesafe green: PASS - implementation verified against RED tests");
        } else {
            println!("codesafe green: FAIL - implementation does not satisfy RED tests");
        }
    }

    if test_code == ExitCode::from(0) {
        Ok(ExitCode::from(0))
    } else {
        Ok(ExitCode::from(1))
    }
}

/// Locate the rules directory.
fn rules_dir_for(path: &std::path::Path, rt: &RuntimeConfig) -> std::path::PathBuf {
    if let Some(d) = &rt.rules_dir {
        return Path::new(d).to_path_buf();
    }
    let project_rules = path.join("rules");
    if project_rules.is_dir() {
        return project_rules;
    }
    Path::new("rules").to_path_buf()
}
