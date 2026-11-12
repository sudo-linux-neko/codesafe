//! Loading of `codeguard.toml`.

use crate::Result;
use crate::types::Config;
use std::path::Path;

/// Load a config file if it exists; missing files yield an empty config (never an error).
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| crate::Error::Config(format!("reading {path:?}: {e}")))?;
    let parsed: Config = toml::from_str(&text)
        .map_err(|e| crate::Error::Config(format!("parsing {path:?}: {e}")))?;
    Ok(parsed)
}

/// Example config documentation.
pub fn example() -> &'static str {
    r#"
# codeguard.toml - edit to tune behavior

jobs = 4
timeout_s = 300
bench_runs = 5
regression_threshold_pct = 5.0
fail_on = "error"
dry_run = false
force = false
rules_dir = "rules"
"#
}
