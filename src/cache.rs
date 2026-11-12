//! Result cache keyed by file content hash.

use crate::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Simple on-disk cache: one file per key under a directory.
pub struct Cache {
    dir: PathBuf,
    map: HashMap<String, String>,
}

impl Cache {
    pub fn new(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| crate::Error::Cache(format!("{dir:?}: {e}")))?;
        let mut map = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let key = entry.file_name().to_string_lossy().to_string();
                if let Ok(value) = std::fs::read_to_string(entry.path()) {
                    map.insert(key, value);
                }
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            map,
        })
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }

    pub fn put(&mut self, key: &str, value: &str) -> Result<()> {
        self.map.insert(key.to_string(), value.to_string());
        let path = self.dir.join(key);
        std::fs::write(&path, value).map_err(|e| crate::Error::Cache(format!("{path:?}: {e}")))?;
        Ok(())
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            dir: PathBuf::new(),
            map: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trip() {
        let dir = std::env::temp_dir().join(format!("codeguard-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Cache::new(&dir).unwrap();
        c.put("a", "hello").unwrap();
        assert_eq!(c.get("a"), Some("hello".into()));
        assert_eq!(c.get("missing"), None);
    }
}
