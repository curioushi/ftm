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
    /// Global history queue size (max total entries across all files).
    pub max_history: usize,
    pub max_file_size: u64,
    /// Max total size in bytes of referenced snapshots. Oldest history and snapshots are trimmed when exceeded.
    #[serde(default = "default_max_quota")]
    pub max_quota: u64,
    /// Interval in seconds between periodic full scans. Minimum 2.
    #[serde(default = "default_scan_interval")]
    pub scan_interval: u64,
    /// Interval in seconds between periodic clean (orphan snapshot removal). Minimum 2.
    #[serde(default = "default_clean_interval")]
    pub clean_interval: u64,
}

fn default_max_quota() -> u64 {
    1024 * 1024 * 1024 // 1GB
}

fn default_scan_interval() -> u64 {
    300
}

fn default_clean_interval() -> u64 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub watch: WatchConfig,
    pub settings: Settings,
    /// Compiled exclude patterns; not serialized, built from watch.exclude.
    #[serde(skip, default)]
    pub exclude_compiled: Vec<Pattern>,
}

impl Default for Config {
    fn default() -> Self {
        let watch = WatchConfig {
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
        };
        let exclude_compiled = watch
            .exclude
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();
        Self {
            watch,
            settings: Settings {
                max_history: 10_000,
                max_file_size: 30 * 1024 * 1024, // 30MB
                max_quota: default_max_quota(),
                scan_interval: default_scan_interval(),
                clean_interval: default_clean_interval(),
            },
            exclude_compiled,
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
        if config.settings.clean_interval < 2 {
            config.settings.clean_interval = 2;
        }
        config.build_exclude_compiled();
        Ok(config)
    }

    fn build_exclude_compiled(&mut self) {
        self.exclude_compiled = self
            .watch
            .exclude
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();
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

        if self.excluded_by_patterns(&path_str, None) {
            return false;
        }

        // Check include patterns
        if let Some(ext) = path.extension() {
            let ext_suffix = format!(".{}", ext.to_string_lossy());
            return self.watch.patterns.iter().any(|p| p.ends_with(&ext_suffix));
        }

        false
    }

    /// Returns true if path_str or (if provided) dir_str matches any compiled exclude pattern.
    pub(crate) fn excluded_by_patterns(&self, path_str: &str, dir_str: Option<&str>) -> bool {
        self.exclude_compiled
            .iter()
            .any(|p| p.matches(path_str) || dir_str.is_some_and(|d| p.matches(d)))
    }

    /// Get a config value by dot-notation key (e.g. "settings.max_history").
    pub fn get_value(&self, key: &str) -> Result<String> {
        match key {
            "settings.max_history" => Ok(self.settings.max_history.to_string()),
            "settings.max_file_size" => Ok(self.settings.max_file_size.to_string()),
            "settings.max_quota" => Ok(self.settings.max_quota.to_string()),
            "settings.scan_interval" => Ok(self.settings.scan_interval.to_string()),
            "settings.clean_interval" => Ok(self.settings.clean_interval.to_string()),
            "watch.patterns" => Ok(self.watch.patterns.join(",")),
            "watch.exclude" => Ok(self.watch.exclude.join(",")),
            _ => anyhow::bail!(
                "Unknown config key '{}'. Valid keys: settings.max_history, \
                 settings.max_file_size, settings.max_quota, settings.scan_interval, settings.clean_interval, \
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
            "settings.max_quota" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value for max_quota: {}", value))?;
                if v == 0 {
                    anyhow::bail!("max_quota must be > 0, got {}", v);
                }
                self.settings.max_quota = v;
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
            "settings.clean_interval" => {
                let v: u64 = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value for clean_interval: {}", value))?;
                if v < 2 {
                    anyhow::bail!("clean_interval must be >= 2, got {}", v);
                }
                self.settings.clean_interval = v;
            }
            "watch.patterns" => {
                self.watch.patterns = value.split(',').map(|s| s.trim().to_string()).collect();
            }
            "watch.exclude" => {
                self.watch.exclude = value.split(',').map(|s| s.trim().to_string()).collect();
                self.build_exclude_compiled();
            }
            _ => anyhow::bail!(
                "Unknown config key '{}'. Valid keys: settings.max_history, \
                 settings.max_file_size, settings.max_quota, settings.scan_interval, settings.clean_interval, \
                 watch.patterns, watch.exclude",
                key
            ),
        }
        Ok(())
    }
}
