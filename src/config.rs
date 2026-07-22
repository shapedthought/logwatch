use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub watch: Watch,

    #[serde(default)]
    pub display: Display,

    #[serde(default)]
    pub filters: Filters,

    #[serde(default)]
    pub history: HistoryConfig,

    #[serde(default)]
    pub output: OutputConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Watch {
    #[serde(default)]
    pub directories: Vec<PathBuf>,

    #[serde(default = "default_file_pattern")]
    pub file_pattern: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Display {
    #[serde(default = "default_path_style")]
    pub path_style: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Filters {
    #[serde(default)]
    pub include: Vec<String>,

    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Lines retained in memory for `/` search. `None` leaves the built-in default
/// in place, which lets the CLI tell "unset" apart from an explicit `0`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Optional export of displayed lines to a file.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OutputConfig {
    #[serde(default)]
    pub file: Option<PathBuf>,

    #[serde(default)]
    pub append: bool,
}

fn default_file_pattern() -> String {
    "*.log".to_string()
}

fn default_path_style() -> String {
    "relative".to_string()
}

impl Default for Watch {
    fn default() -> Self {
        Self {
            directories: Vec::new(),
            file_pattern: default_file_pattern(),
        }
    }
}

impl Default for Display {
    fn default() -> Self {
        Self {
            path_style: default_path_style(),
        }
    }
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn watch_directories(&self) -> Vec<PathBuf> {
        self.watch.directories.clone()
    }

    pub fn file_pattern(&self) -> Option<String> {
        if self.watch.file_pattern != default_file_pattern() {
            Some(self.watch.file_pattern.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.watch.file_pattern, "*.log");
        assert_eq!(config.display.path_style, "relative");
        assert!(config.watch.directories.is_empty());
    }
}
