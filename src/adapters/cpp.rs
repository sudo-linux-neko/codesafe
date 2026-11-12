//! C++ adapter: CMake/Make, clang-format, clang-tidy, cppcheck,
//! ASan/UBSan/TSan, libFuzzer, Google Benchmark, perf/valgrind.

use crate::Result;
use crate::adapters::{
    Context, FixResult, Hotspot, LanguageAdapter, require_tool, run_command, run_command_in_dir,
};
use crate::types::{BenchmarkResult, Language, Rule, RuleSafety, ToolResult};
use std::path::Path;
use walkdir::WalkDir;

pub struct Cpp;

impl LanguageAdapter for Cpp {
    fn language(&self) -> Language {
        Language::Cpp
    }

    fn detect(&self, path: &Path) -> Result<bool> {
        Ok(has_cpp_files(path)
            || path.join("CMakeLists.txt").is_file()
            || path.join("Makefile").is_file()
            || path.join("makefile").is_file())
    }

    fn build(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if path_join_exists(ctx.path, "CMakeLists.txt") {
            require_tool("cmake")?;
            let build_dir = ctx.path.join("build");
            let _ = std::fs::create_dir_all(&build_dir);
            let cfg = run_command(
                ctx.path,
                "cmake",
                &[
                    "-S",
                    ctx.path.to_str().unwrap_or("."),
                    "-B",
                    build_dir.to_str().unwrap_or("build"),
                ],
                ctx.timeout_s,
            )?;
            if !cfg.success() {
                return Ok(cfg);
            }
            run_command(
                ctx.path,
                "cmake",
                &[
                    "--build",
                    build_dir.to_str().unwrap_or("build"),
                    "-j",
                    &ctx.jobs.to_string(),
                ],
                ctx.timeout_s,
            )
        } else if path_join_exists(ctx.path, "Makefile") || path_join_exists(ctx.path, "makefile") {
            require_tool("make")?;
            run_command(
                ctx.path,
                "make",
                &["-j", &ctx.jobs.to_string()],
                ctx.timeout_s,
            )
        } else {
            Ok(skipped("cmake/make"))
        }
    }

    fn test(&self, ctx: Context<'_>) -> Result<ToolResult> {
        let build_dir = ctx.path.join("build");
        if build_dir.join("ctest").exists() || build_dir.is_dir() {
            if require_tool("ctest")?.skipped {
                return Ok(skipped("ctest"));
            }
            return run_command(
                ctx.path,
                "ctest",
                &[
                    "--test-dir",
                    build_dir.to_str().unwrap_or("build"),
                    "--output-on-failure",
                ],
                ctx.timeout_s,
            );
        }
        if path_join_exists(ctx.path, "Makefile") || path_join_exists(ctx.path, "makefile") {
            if require_tool("make")?.skipped {
                return Ok(skipped("make"));
            }
            return run_command(
                ctx.path,
                "make",
                &["test", "-j", &ctx.jobs.to_string()],
                ctx.timeout_s,
            );
        }
        Ok(skipped("ctest/make test"))
    }

    fn lint(&self, ctx: Context<'_>) -> Result<ToolResult> {
        if !require_tool("clang-format")?.skipped {
            return lint_clang_format(ctx);
        }
        if !require_tool("clang-tidy")?.skipped {
            return run_command(
                ctx.path,
                "clang-tidy",
                &[ctx.path.to_str().unwrap_or(".")],
                ctx.timeout_s,
            );
        }
        if !require_tool("cppcheck")?.skipped {
            return run_command(
                ctx.path,
                "cppcheck",
                &["--quiet", ctx.path.to_str().unwrap_or(".")],
                ctx.timeout_s,
            );
        }
        Ok(skipped("clang-format/clang-tidy/cppcheck"))
    }

    fn fuzz(&self, ctx: Context<'_>) -> Result<ToolResult> {
        // libFuzzer targets are built binaries named *fuzz* or fuzz_*; run any found for a short time.
        if let Some(target) = find_fuzz_target(ctx.path) {
            return run_command(
                ctx.path,
                target.to_str().unwrap_or(""),
                &["-max_total_time=30"],
                ctx.timeout_s,
            );
        }
        Ok(skipped("libFuzzer target"))
    }

    fn bench(&self, ctx: Context<'_>) -> Result<Vec<BenchmarkResult>> {
        bench_cpp(ctx)
    }

    fn profile(&self, _ctx: Context<'_>) -> Result<Vec<Hotspot>> {
        if require_tool("perf")?.skipped {
            return Ok(vec![]);
        }
        Ok(vec![])
    }

    fn fix(&self, ctx: Context<'_>, rules: &[Rule]) -> Result<Vec<FixResult>> {
        fix_cpp(ctx, rules)
    }
}

/// Run clang-format without relying on `--check` (some builds, e.g. Termux's,
/// do not support it). Formats each source file to stdout and reports any file
/// whose formatted output differs from the on-disk content.
fn lint_clang_format(ctx: Context<'_>) -> Result<ToolResult> {
    let files = cpp_source_files(ctx.path);
    if files.is_empty() {
        return Ok(skipped("clang-format"));
    }
    let mut unformatted = Vec::new();
    for f in &files {
        let res = run_command(
            ctx.path,
            "clang-format",
            &[f.to_str().unwrap_or("")],
            ctx.timeout_s,
        )?;
        let original = match std::fs::read(f) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if res.stdout.as_bytes() != original.as_slice() {
            unformatted.push(f.to_string_lossy().to_string());
        }
    }
    if unformatted.is_empty() {
        Ok(ToolResult {
            tool: "clang-format".into(),
            command: "clang-format <files>".into(),
            exit_code: Some(0),
            stdout: "all files formatted".into(),
            stderr: String::new(),
            duration_ms: 0,
            skipped: false,
            skip_reason: None,
        })
    } else {
        Ok(ToolResult {
            tool: "clang-format".into(),
            command: "clang-format <files>".into(),
            exit_code: Some(1),
            stdout: String::new(),
            stderr: format!("files need formatting: {}", unformatted.join(", ")),
            duration_ms: 0,
            skipped: false,
            skip_reason: None,
        })
    }
}

/// Collect C/C++ source files under `path`, ignoring common build/cache dirs.
fn cpp_source_files(path: &Path) -> Vec<std::path::PathBuf> {
    let exts = ["cpp", "cc", "cxx", "h", "hpp", "hxx"];
    WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !crate::scan::is_ignored_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| exts.contains(&ext))
        })
        .map(|e| e.into_path())
        .collect()
}

fn skipped(tool: &str) -> ToolResult {
    ToolResult {
        tool: tool.to_string(),
        command: format!("which {tool}"),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 0,
        skipped: true,
        skip_reason: Some(format!("{tool} not found on PATH")),
    }
}

fn path_join_exists(path: &Path, name: &str) -> bool {
    path.join(name).is_file()
}

fn has_cpp_files(path: &Path) -> bool {
    !find_files_by_ext(path, &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"]).is_empty()
}

fn find_files_by_ext(path: &Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !crate::scan::is_ignored_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let ext = e.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            if exts.contains(&ext) {
                Some(e.path().to_path_buf())
            } else {
                None
            }
        })
        .collect()
}

/// Find a libFuzzer-style fuzz binary under build/.
fn find_fuzz_target(path: &Path) -> Option<std::path::PathBuf> {
    let build = path.join("build");
    let root = if build.is_dir() { &build } else { path };
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !crate::scan::is_ignored_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if (name.contains("fuzz") || name.contains("fuzzer")) && is_executable(entry.path()) {
            return Some(entry.path().to_path_buf());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

/// Google Benchmark binaries emit JSON with `--benchmark_format=json`.
fn bench_cpp(ctx: Context<'_>) -> Result<Vec<BenchmarkResult>> {
    let targets = find_benchmark_targets(ctx.path);
    let mut out = Vec::new();
    for target in targets {
        let result = run_command(
            ctx.path,
            target.to_str().unwrap_or(""),
            &["--benchmark_format=json"],
            ctx.timeout_s,
        )?;
        if let Ok(mut benchmarks) = parse_google_benchmark_json(&result.stdout) {
            out.append(&mut benchmarks);
        }
    }
    Ok(out)
}

fn find_benchmark_targets(path: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let roots = [path.join("build"), path.to_path_buf()];
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !crate::scan::is_ignored_dir(e))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if (name.contains("bench") || name.contains("benchmark")) && is_executable(entry.path())
            {
                out.push(entry.path().to_path_buf());
            }
        }
    }
    out
}

fn parse_google_benchmark_json(text: &str) -> Result<Vec<BenchmarkResult>> {
    let data: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
    let benchmarks: Vec<serde_json::Value> = data
        .get("benchmarks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<BenchmarkResult> = Vec::new();
    for b in benchmarks {
        let name = b
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let stats: serde_json::Map<String, serde_json::Value> = b
            .get("stats")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let real_time = stats.get("real_time").and_then(|v| v.as_f64());
        let unit = b
            .get("time_unit")
            .and_then(|v| v.as_str())
            .unwrap_or("ns")
            .to_string();
        out.push(BenchmarkResult {
            name,
            before: None,
            after: real_time,
            unit,
            change_pct: None,
        });
    }
    Ok(out)
}

/// Safe fixes: apply each safe C++ rule's action (e.g. clang-format -i).
fn fix_cpp(ctx: Context<'_>, rules: &[Rule]) -> Result<Vec<FixResult>> {
    let mut results = Vec::new();
    if require_tool("clang-format")?.skipped {
        return Ok(results);
    }
    if find_files_by_ext(ctx.path, &["cpp", "cc", "cxx", "h", "hpp", "hxx"]).is_empty() {
        return Ok(results);
    }
    for rule in rules {
        if rule.language != Language::Cpp || rule.safety != RuleSafety::Safe {
            continue;
        }
        let action = rule.action.trim();
        if action.is_empty() {
            continue;
        }
        if ctx.dry_run {
            results.push(FixResult {
                rule_id: rule.id.clone(),
                file: "repository".into(),
                applied: false,
                reverted: false,
                reason: "dry run, no changes applied".into(),
            });
            continue;
        }
        let res = run_command_in_dir(ctx.path, action, ctx.timeout_s)?;
        results.push(FixResult {
            rule_id: rule.id.clone(),
            file: "repository".into(),
            applied: res.success(),
            reverted: false,
            reason: if res.success() {
                "rule action applied".into()
            } else {
                format!("action failed: {}", res.stderr.trim())
            },
        });
    }
    Ok(results)
}
