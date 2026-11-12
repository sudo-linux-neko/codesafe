//! Binary entry point for codeguard.

use clap::Parser;
use codeguard::{
    Result,
    adapters::{LanguageAdapter, cpp::Cpp, python::Python, rust::Rust},
    cli::{Cli, Command, RuntimeConfig},
    config, report, rule_engine, scan,
    types::{FindingStatus, Language, Report, Severity},
};
use rayon::prelude::*;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("codeguard: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let config = config::load(&cli.config)?;
    let rt = RuntimeConfig::from_cli(&cli, config)?;
    match cli.command {
        Command::Scan { path } => cmd_scan(&path, &rt),
        Command::Test { path } => cmd_test(&path, &rt),
        Command::Bench { path } => cmd_bench(&path, &rt),
        Command::Fix { path } => cmd_fix(&path, &rt),
        Command::Report { path } => cmd_report(&path, &rt),
        Command::All { path } => cmd_all(&path, &rt),
        Command::Init { path } => cmd_init(&path),
    }
}

fn adapters_for(path: &std::path::Path) -> Result<codeguard::adapters::AdapterList> {
    let _scan = scan::scan(path)?;
    // Instantiate adapters for every language we support; each adapter's detect()
    // confirms applicability at run time. We keep all adapters so a mixed repo
    // is handled in parallel.
    Ok(vec![
        Box::new(Python) as Box<dyn LanguageAdapter>,
        Box::new(Rust) as Box<dyn LanguageAdapter>,
        Box::new(Cpp) as Box<dyn LanguageAdapter>,
    ])
}

fn cmd_scan(path: &std::path::Path, _rt: &RuntimeConfig) -> Result<ExitCode> {
    let result = scan::scan(path)?;
    println!("{}", scan::render(&result));
    Ok(ExitCode::from(0))
}

/// Run build/test/lint/fuzz across applicable adapters and collect tool results,
/// grouped by language.
fn run_tools(
    path: &std::path::Path,
    rt: &RuntimeConfig,
) -> Result<Vec<(Language, codeguard::adapters::ToolResults)>> {
    // Work with an absolute path so subprocesses (cargo, cmake, ...) invoked from
    // this directory never see a doubled/relative path.
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let adapters = adapters_for(&path)?;

    // Run each language's tool suite in parallel — they are independent processes.
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
            Some(_) if r.exit_code == Some(2) => Severity::Error,
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

fn cmd_test(path: &std::path::Path, rt: &RuntimeConfig) -> Result<ExitCode> {
    let results: Vec<codeguard::types::ToolResult> = run_tools(path, rt)?
        .into_iter()
        .flat_map(|(_, r)| r)
        .collect();
    print_results(&results);
    let failed = any_failed(&results);
    let worst = worst_severity(&results);
    let gated = worst.is_some_and(|s| rt.fail_on.is_some_and(|f| s >= f));
    // When --fail-on is set, only fail if the worst severity meets the threshold.
    // Otherwise, any non-zero exit code is a failure.
    let fail = match rt.fail_on {
        None => failed,
        Some(_) => gated,
    };
    if fail {
        println!("\ncodeguard test: FAIL (fail_on={:?})", rt.fail_on);
        Ok(ExitCode::from(1))
    } else {
        println!("\ncodeguard test: pass");
        Ok(ExitCode::from(0))
    }
}

fn cmd_bench(path: &std::path::Path, rt: &RuntimeConfig) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    let scan_result = match scan::scan(&path) {
        Ok(s) => s,
        Err(_) => {
            println!("codeguard bench: {} (no language detected)", path.display());
            return Ok(ExitCode::from(0));
        }
    };
    let source_hash = codeguard::types::hash_files(&scan_result.source_files)?;
    let mut bench_cache = codeguard::bench::BenchmarkCache::new(&path)?;
    let mut adapters = adapters_for(&path)?;
    let mut regressions = 0u64;
    let mut any_benchmarks = false;
    let mut all_hotspots: Vec<codeguard::adapters::Hotspot> = Vec::new();
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
        // Repeat the benchmark run and keep the median per benchmark name.
        let mut series: Vec<(String, String, Vec<f64>)> = Vec::new();
        for _ in 0..rt.bench_runs {
            for b in adapter.bench(ctx)? {
                if let Some(value) = b.after {
                    match series.iter_mut().find(|(n, _, _)| n == &b.name) {
                        Some((_, _, values)) => values.push(value),
                        None => series.push((b.name.clone(), b.unit.clone(), vec![value])),
                    }
                }
            }
        }
        for (name, unit, mut values) in series {
            let after = match codeguard::bench::median(&mut values) {
                Some(v) => v,
                None => continue,
            };
            any_benchmarks = true;
            let before = bench_cache.previous(language, &source_hash, &name);
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
            let _ = bench_cache.store(language, &source_hash, &name, after);
            let before_str = before.map_or("n/a".into(), |v| format!("{v:?}"));
            let change = change_pct.map_or("n/a".into(), |c| format!("{c:+.2}%"));
            let flag = if regressed { "  REGRESSION" } else { "" };
            println!(
                "  bench {}: before={} after={:?} {} change={}{}",
                name, before_str, after, unit, change, flag
            );
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
    println!("codeguard bench: {}", path.display());
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
    if regressions > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

fn cmd_fix(path: &std::path::Path, rt: &RuntimeConfig) -> Result<ExitCode> {
    let path = path
        .canonicalize()
        .map_err(|e| codeguard::Error::Config(format!("cannot canonicalize {path:?}: {e}")))?;
    if codeguard::git::is_git_repo(&path) && !rt.force && codeguard::git::is_dirty(&path)? {
        return Err(codeguard::Error::Git(
            "working tree is dirty; commit or stash changes, or use --force".into(),
        ));
    }
    let rules_dir = rules_dir_for(&path, rt);
    let engine = rule_engine::RuleEngine::load_from_dir(&rules_dir)?;
    let mut adapters = adapters_for(&path)?;
    let findings = rule_engine::safe_fix_loop(&path, rt, &engine, &mut adapters)?;
    for f in &findings {
        println!("[fix] {} [{:?}] {}", f.id, f.status, f.description);
    }
    if rt.dry_run {
        println!("codeguard fix: dry run, no changes applied");
    } else {
        let kept = findings
            .iter()
            .filter(|f| f.status == FindingStatus::Fixed)
            .count();
        println!("codeguard fix: applied {kept} verified fix(es)");
    }
    Ok(ExitCode::from(0))
}

/// Locate the rules directory: explicit `--config`-adjacent setting, then `<path>/rules`,
/// then `./rules`.
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

fn cmd_report(path: &std::path::Path, rt: &RuntimeConfig) -> Result<ExitCode> {
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
    report::write(path, &report)?;
    println!(
        "wrote report.md and report.json to {} ({} findings, {} ai tips)",
        path.display(),
        report.summary.findings_total,
        ai_tips.len()
    );
    Ok(ExitCode::from(0))
}

fn cmd_all(path: &std::path::Path, rt: &RuntimeConfig) -> Result<ExitCode> {
    let _ = cmd_scan(path, rt)?;
    let test_code = cmd_test(path, rt)?;
    let _ = cmd_bench(path, rt)?;
    let fix_code = cmd_fix(path, rt)?;
    let _ = cmd_report(path, rt)?;
    if test_code == ExitCode::from(0) && fix_code == ExitCode::from(0) {
        Ok(ExitCode::from(0))
    } else {
        Ok(ExitCode::from(1))
    }
}

/// Initialize a `codeguard.toml` config and `rules/` directory for a project.
fn cmd_init(path: &std::path::Path) -> Result<ExitCode> {
    let config_path = path.join("codeguard.toml");
    if config_path.exists() {
        return Err(codeguard::Error::Config(format!(
            "{config_path:?} already exists; refusing to overwrite"
        )));
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

    println!(
        "Initialized codeguard:\n  config: {}\n  rules:  {}/\n\nEdit codeguard.toml and rules/*.toml to tune behavior.\nRun `codeguard test <path>` to verify.",
        config_path.display(),
        rules_dir.display()
    );
    Ok(ExitCode::from(0))
}
