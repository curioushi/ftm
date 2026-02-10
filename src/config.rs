use crate::path_util;
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
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    /// Interval in seconds between periodic full scans. Minimum 2.
    #[serde(default = "default_scan_interval")]
    pub scan_interval: u64,
}

fn default_web_port() -> u16 {
    13580
}

fn default_scan_interval() -> u64 {
    300
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
                    "*.vision".into(),
                    "*.task".into(),
                    "*.conf".into(),
                    "*.ini".into(),
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
                max_file_size: 30 * 1024 * 1024, // 30MB
                web_port: 13580,
                scan_interval: 300,
            },
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Config = serde_yaml::from_str(&content)?;
        if config.settings.scan_interval < 2 {
            config.settings.scan_interval = 2;
        }
        Ok(config)
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
        let path_str = path_util::normalize_rel_path(&rel_path.to_string_lossy());

        // Check exclude patterns (glob expects forward slashes)
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

    /// Get a config value by dot-notation key (e.g. "settings.max_history").
    pub fn get_value(&self, key: &str) -> Result<String> {
        match key {
            "settings.max_history" => Ok(self.settings.max_history.to_string()),
            "settings.max_file_size" => Ok(self.settings.max_file_size.to_string()),
            "settings.web_port" => Ok(self.settings.web_port.to_string()),
            "settings.scan_interval" => Ok(self.settings.scan_interval.to_string()),
            "watch.patterns" => Ok(self.watch.patterns.join(",")),
            "watch.exclude" => Ok(self.watch.exclude.join(",")),
            _ => anyhow::bail!(
                "Unknown config key '{}'. Valid keys: settings.max_history, \
                 settings.max_file_size, settings.web_port, settings.scan_interval, \
                 watch.patterns, watch.exclude",
                key
            ),
        }
    }

    /// Set a config value by dot-notation key (e.g. "settings.max_history").
    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "settings.max_history" => {
                self.settings.max_history = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value for max_history: {}", value))?;
            }
            "settings.max_file_size" => {
                self.settings.max_file_size = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value for max_file_size: {}", value))?;
            }
            "settings.web_port" => {
                self.settings.web_port = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value for web_port: {}", value))?;
            }
            "settings.scan_interval" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value for scan_interval: {}", value))?;
                if v < 2 {
                    anyhow::bail!("scan_interval must be >= 2, got {}", v);
                }
                self.settings.scan_interval = v;
            }
            "watch.patterns" => {
                self.watch.patterns = value.split(',').map(|s| s.trim().to_string()).collect();
            }
            "watch.exclude" => {
                self.watch.exclude = value.split(',').map(|s| s.trim().to_string()).collect();
            }
            _ => anyhow::bail!(
                "Unknown config key '{}'. Valid keys: settings.max_history, \
                 settings.max_file_size, settings.web_port, settings.scan_interval, \
                 watch.patterns, watch.exclude",
                key
            ),
        }
        Ok(())
    }
}
