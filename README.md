# codeguard

**Test, benchmark, profile and safely auto-fix Python, Rust and C++ repositories.**

One static binary that orchestrates existing tools behind one trait per language.
Emits `report.md` (human-readable) and `report.json` (AI-actionable `ai_tips`).

---

## 📋 Table of Contents

- [Features](#features)
- [Quick Start](#quick-start)
- [Installation](#installation)
- [Usage](#usage)
- [Cross-Platform Support](#cross-platform-support)
- [Configuration](#configuration)
- [Rules](#rules)
- [Safe Fix Loop](#safe-fix-loop)
- [Reports](#reports)
- [CI / Exit Codes](#ci--exit-codes)
- [License](#license)

---

## ✨ Features

| Feature | Description |
|---------|-------------|
| `scan` | Detect language, build system, source files, and available tools |
| `test` | Build, run tests, lint, and fuzz across all detected languages |
| `bench` | Run benchmarks (median of N runs), cache results, flag regressions |
| `fix` | Safe rule-based fixes: checkpoint → apply → verify → commit/revert |
| `report` | Write `report.md` (human) + `report.json` (AI tips) |
| `all` | Run scan → test → bench → fix → report in one command |
| `init` | Scaffold a `codeguard.toml` + `rules/` for a new project |

**Design principles:**
- **Tool orchestration only** — never reimplements lint/test tools; skips missing tools with clear notes, never crashes
- **No `unwrap` in non-test code** — all errors use `Result<T>`
- **Safe by default** — only `safety = "safe"` rules are auto-applied; everything else is report-only
- **Verified fixes** — each fix is applied, then build+tests are rerun; if anything fails, the change is reverted automatically
- **One fix per commit** — git history stays clean and auditable

---

## 🚀 Quick Start

### Copy-paste install (one-liner)

**Linux / macOS:**

```bash
curl -fsSL https://github.com/sudo-linux-neko/codesafe/releases/latest/download/codeguard-$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m) -o /usr/local/bin/codeguard && chmod +x /usr/local/bin/codeguard
```

**Windows (PowerShell, Administrator):**

```powershell
Invoke-WebRequest -Uri "https://github.com/sudo-linux-neko/codesafe/releases/latest/download/codeguard-windows-x86_64.exe" -OutFile "$env:USERPROFILE\codeguard.exe" ; [Environment]::SetEnvironmentVariable("PATH", [Environment]::GetEnvironmentVariable("PATH", [EnvironmentVariableTarget]::User) + ";$env:USERPROFILE", [EnvironmentVariableTarget]::User)
```

**Termux (Android):**

```bash
pkg install rust cargo git
cargo install --git https://github.com/sudo-linux-neko/codesafe codeguard
# or use the prebuilt binary:
curl -fsSL https://github.com/sudo-linux-neko/codesafe/releases/latest/download/codeguard-linux-aarch64 -o $PREFIX/bin/codeguard && chmod +x $PREFIX/bin/codeguard
```

---

## 💾 Installation

### Option 1: Prebuilt binary (recommended)

Download the latest release for your platform from the [Releases page](https://github.com/sudo-linux-neko/codesafe/releases):

| Platform | Asset |
|----------|-------|
| Linux x86_64 | `codeguard-linux-x86_64` |
| Linux aarch64 (ARM64, incl. Raspberry Pi, Termux) | `codeguard-linux-aarch64` |
| macOS x86_64 (Intel) | `codeguard-macos-x86_64` |
| macOS aarch64 (Apple Silicon) | `codeguard-macos-aarch64` |
| Windows x86_64 | `codeguard-windows-x86_64.exe` |

Then place it on your `PATH`:

```bash
# Linux / macOS
install -m 0755 codeguard-<platform>-<arch> /usr/local/bin/codeguard

# Windows (PowerShell)
Copy-Item codeguard-windows-x86_64.exe $env:USERPROFILE\codeguard.exe
# Add $env:USERPROFILE to your PATH
```

### Option 2: Build from source

**Requirements:** Rust 1.70+, cargo

```bash
# Clone
git clone https://github.com/sudo-linux-neko/codesafe.git
cd codesafe

# Build (release for best performance)
cargo build --release

# The binary is at target/release/codeguard
cp target/release/codeguard /usr/local/bin/
```

**Termux-specific notes:**
- `cargo build` needs a writable `CARGO_TARGET_DIR` because `/data` may be noexec:
  ```bash
  export CARGO_TARGET_DIR=/data/data/com.termux/files/home/.cache/codeguard-target
  cargo build --release
  ```
- Disk space on Android shared storage can fill up quickly; run `df -h` before building.

---

## 🖥️ Cross-Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| Linux (x86_64, aarch64) | ✅ Fully supported | Tested on Ubuntu, Debian, Termux |
| macOS (Intel, Apple Silicon) | ✅ Fully supported | Tested on macOS 14+ |
| Windows (x86_64) | ✅ Fully supported | Tested on Windows 10/11, PowerShell + CMD |
| Termux (Android) | ✅ Fully supported | ARM64, use `CARGO_TARGET_DIR` cache dir |

**Platform-specific behavior:**

- **clang-format:** On some Termux builds, `--check` is unsupported. codeguard uses a per-file diff approach instead.
- **cargo target dir:** codeguard injects `CARGO_TARGET_DIR=<repo>/.codeguard/target` per repository to avoid noexec mount issues.
- **git:** Safe-directory configuration may be needed on Termux: `git config --global --add safe.directory '*'`

---

## 📖 Usage

```bash
# Scan a repository for languages, build systems, and tools
codeguard scan /path/to/repo

# Build, test, and lint all detected languages
codeguard test /path/to/repo

# Run benchmarks and detect regressions
codeguard bench /path/to/repo

# Apply safe fixes (requires git repo or --force)
codeguard fix /path/to/repo

# Generate report.md + report.json
codeguard report /path/to/repo

# Everything in one command
codeguard all /path/to/repo

# Scaffold config + rules for a new project
codeguard init /path/to/repo
```

### Global options

| Option | Short | Description |
|--------|-------|-------------|
| `--config <PATH>` | | Path to `codeguard.toml` (default: `codeguard.toml`) |
| `--jobs <N>` | `-j` | Number of parallel jobs (default: 1) |
| `--timeout <S>` | | Timeout in seconds per tool invocation |
| `--fail-on <LEVEL>` | | Fail if any finding has severity ≥ this level (info\|warning\|error\|critical) |
| `--dry-run` | | Report what would change without modifying files |
| `--force` | | Allow fix on a dirty git tree |

### Examples

```bash
# CI-friendly: fail on warnings or worse
codeguard test --fail-on warning ./my-project

# Dry run: see what fixes would be applied
codeguard fix --dry-run ./my-project

# Force fix even with uncommitted changes
codeguard fix --force ./my-project

# Parallel execution with 4 jobs, 300s timeout
codeguard all -j 4 --timeout 300 ./my-project
```

---

## ⚙️ Configuration

Create a `codeguard.toml` in your project root:

```toml
# codeguard.toml
jobs = 4
timeout_s = 300
bench_runs = 5
regression_threshold_pct = 5.0
fail_on = "error"
dry_run = false
force = false
rules_dir = "rules"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `jobs` | integer | 1 | Parallel jobs |
| `timeout_s` | integer | none | Timeout per tool (seconds) |
| `bench_runs` | integer | 3 | Benchmark repetitions |
| `regression_threshold_pct` | float | 5.0 | Regression flag threshold (%) |
| `fail_on` | string | none | CI gating severity |
| `dry_run` | boolean | false | Don't modify files |
| `force` | boolean | false | Allow fix on dirty tree |
| `rules_dir` | string | `rules` | Rules directory |

Or generate one automatically:

```bash
codeguard init ./my-project
```

---

## 📜 Rules

Rules live in `rules/<language>.toml` as `[[rules]]` tables:

```toml
[[rules]]
id = "cpp-clang-format"
language = "cpp"
trigger = "clang-format"
action = "clang-format -i src/**/*.cpp src/**/*.h"
safety = "safe"
explanation = "Format C++ sources with clang-format (output-preserving)."
```

| Field | Values | Description |
|-------|--------|-------------|
| `id` | string | Unique rule identifier |
| `language` | `python`, `rust`, `cpp` | Target language |
| `trigger` | string | Substring or `tool:code` to match against tool output |
| `action` | string | Shell command to apply the fix |
| `safety` | `safe` \| `review` | `safe` = auto-applied; `review` = report-only |
| `explanation` | string | Human-readable description |

Generate default rules for all three languages:

```bash
codeguard init ./my-project
```

This creates `rules/python.toml`, `rules/rust.toml`, and `rules/cpp.toml`.

---

## 🔒 Safe Fix Loop

When you run `codeguard fix`:

1. **Git checkpoint** — commits the current state (refuses dirty trees unless `--force`)
2. **Baseline** — captures build + test + lint output before any fix
3. **Apply** — runs each `safety = "safe"` rule's action
4. **Verify** — rebuilds, reruns tests, compares output to baseline
5. **Commit or revert** — keeps the fix as its own commit if verified; reverts automatically if anything fails
6. **Baseline refresh** — after each kept fix, the baseline is refreshed so subsequent fixes compare against the post-fix state

### Example output

```
[fix] cpp-clang-format [Fixed] applied and verified
codeguard fix: applied 1 verified fix(es)
```

---

## 📊 Reports

`codeguard report` (or `codeguard all`) writes two files:

### `report.md` — human-readable

Markdown with summary table, tool results, findings, benchmarks, and AI tips.

### `report.json` — AI-actionable

```json
{
  "summary": {
    "tests_passed": true,
    "findings_total": 1,
    "findings_fixed": 0,
    "benchmarks_total": 0
  },
  "findings": [
    {
      "id": "clang-format-failure",
      "severity": "warning",
      "language": "cpp",
      "status": "reported",
      "description": "files need formatting: ...",
      "fix": null,
      "evidence": "..."
    }
  ],
  "ai_tips": [
    {
      "file": "src/main.cpp",
      "line": null,
      "concrete_change": "Run clang-format -i on this file",
      "why_safe": "Formatting is output-preserving by construction",
      "how_to_verify": "Run clang-format -i, then `codeguard test <path>`"
    }
  ]
}
```

---

## 🔄 CI / Exit Codes

| Exit code | Meaning |
|-----------|---------|
| `0` | All checks passed |
| `1` | Test/fix/bench failure (or `--fail-on` threshold met) |
| `2` | Git error (dirty tree, commit failed) or fatal error |

### GitHub Actions example

```yaml
name: codeguard
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: |
          curl -fsSL https://github.com/sudo-linux-neko/codesafe/releases/latest/download/codeguard-linux-x86_64 -o /usr/local/bin/codeguard
          chmod +x /usr/local/bin/codeguard
      - run: codeguard test --fail-on warning .
```

---

## 👥 Credit

**Author:** vaibhav (legal name)  
**GitHub:** [@sudo-linux-neko](https://github.com/sudo-linux-neko)

This project is maintained by **vaibhav** under the GitHub account **sudo-linux-neko**.

---

## 📄 License

This project is licensed under the **GPL-3.0 license**. See the [LICENSE](LICENSE) file for details.

---

*Built with Rust, clap, rayon, serde, and thiserror. Orchestrates existing tools — never reimplements them.*