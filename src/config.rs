use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    pub patterns: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub max_history: usize,
    pub max_file_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub watch: WatchConfig,
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch: WatchConfig {
                patterns: vec![
                    "*.rs".into(),
                    "*.py".into(),
                    "*.md".into(),
                    "*.txt".into(),
                    "*.json".into(),
                    "*.yml".into(),
                    "*.yaml".into(),
                    "*.toml".into(),
                    "*.js".into(),
                    "*.ts".into(),
                ],
                exclude: vec![
                    "**/target/**".into(),
                    "**/node_modules/**".into(),
                    "**/.git/**".into(),
                    "**/.ftm/**".into(),
                ],
            },
            settings: Settings {
                max_history: 100,
                max_file_size: 10 * 1024 * 1024, // 10MB
            },
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&content)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Check if a file path matches the watch patterns (include/exclude).
    /// `path` should be an absolute path, `root_dir` is the project root.
    pub fn matches_path(&self, path: &Path, root_dir: &Path) -> bool {
        let rel_path = path.strip_prefix(root_dir).unwrap_or(path);
        let path_str = rel_path.to_string_lossy();

        // Check exclude patterns
        for pattern in &self.watch.exclude {
            if let Ok(p) = Pattern::new(pattern) {
                if p.matches(&path_str) {
                    return false;
                }
            }
        }

        // Check include patterns
        if let Some(ext) = path.extension() {
            let ext_pattern = format!("*.{}", ext.to_string_lossy());
            for pattern in &self.watch.patterns {
                if pattern == &ext_pattern
                    || pattern.ends_with(&format!(".{}", ext.to_string_lossy()))
                {
                    return true;
                }
            }
        }

        false
    }
}
