//! Per-plugin, per-pattern scene + baseline persistence on disk.

use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct ScenesStore {
    root: PathBuf,
}

impl ScenesStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn load(&self, plugin: &str, pattern: &str) -> Result<Option<serde_json::Value>> {
        let path = self.path(plugin, pattern)?;
        if !path.is_file() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("read scenes {}", path.display()))?;
        serde_json::from_str(&data).context("parse scenes JSON")
            .map(Some)
    }

    pub fn save(&self, plugin: &str, pattern: &str, data: &serde_json::Value) -> Result<()> {
        let path = self.path(plugin, pattern)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create scenes dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(data).context("serialize scenes JSON")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("write scenes {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("commit scenes {}", path.display()))?;
        Ok(())
    }

    /// Last pattern selected on the full scenes UI (for the remote slider page).
    pub fn load_active_pattern(&self, plugin: &str) -> Result<Option<String>> {
        let path = self.active_path(plugin);
        if !path.is_file() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("read active pattern {}", path.display()))?;
        let v: serde_json::Value = serde_json::from_str(&data).context("parse active pattern JSON")?;
        Ok(v.get("pattern").and_then(|p| p.as_str()).map(str::to_string))
    }

    pub fn save_active_pattern(&self, plugin: &str, pattern: &str) -> Result<()> {
        let path = self.active_path(plugin);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create scenes dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&serde_json::json!({ "pattern": pattern }))
            .context("serialize active pattern JSON")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).with_context(|| format!("write active pattern {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("commit active pattern {}", path.display()))?;
        Ok(())
    }

    fn active_path(&self, plugin: &str) -> PathBuf {
        self.root
            .join(sanitize_segment(plugin))
            .join("_active.json")
    }

    fn path(&self, plugin: &str, pattern: &str) -> Result<PathBuf> {
        Ok(self
            .root
            .join(sanitize_segment(plugin))
            .join(format!("{}.json", sanitize_segment(pattern))))
    }
}

fn sanitize_segment(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return "default".to_string();
    }
    trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize_segment("Analog Heat"), "Analog_Heat");
        assert_eq!(sanitize_segment("A01"), "A01");
        assert_eq!(sanitize_segment(""), "default");
    }

    #[test]
    fn active_pattern_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ob-scenes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = ScenesStore::new(dir.clone());
        store.save_active_pattern("Digitakt", "B05").unwrap();
        assert_eq!(
            store.load_active_pattern("Digitakt").unwrap().as_deref(),
            Some("B05")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
