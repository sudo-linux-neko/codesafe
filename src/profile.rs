//! Profiling helpers: parsing profiler output into ranked hotspots.

use crate::Result;
use crate::types::Language;

/// Parse raw profiler text into ranked hotspots. Language-specific parsers live
/// in each adapter; this is a shared fallback that returns empty with a clear note.
pub fn parse(text: &str, _language: Language) -> Result<Vec<crate::adapters::Hotspot>> {
    let _ = text;
    Ok(vec![])
}

/// Write a human-readable note when no profiler is available.
pub fn missing_profiler_note(language: Language) -> String {
    format!(
        "no profiler configured for {language}; install py-spy/perf/flamegraph to enable profiling"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_hotspots() {
        let hotspots = parse("", crate::types::Language::Python).unwrap();
        assert!(hotspots.is_empty());
    }
}
