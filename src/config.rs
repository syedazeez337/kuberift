use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CostDisplayPeriod {
    Hourly,
    #[default]
    Daily,
    Monthly,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CostConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub opencost_endpoint: String,
    #[serde(default)]
    pub display_period: CostDisplayPeriod,
    #[serde(default = "default_highlight_threshold")]
    pub highlight_threshold: f64,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            opencost_endpoint: String::new(),
            display_period: CostDisplayPeriod::Daily,
            highlight_threshold: default_highlight_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub cost: CostConfig,
}

fn default_highlight_threshold() -> f64 {
    10.0
}

pub fn config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kuberift").join("config.toml"))
}

pub fn load_config() -> AppConfig {
    config_file_path()
        .as_deref()
        .and_then(load_config_from_path)
        .unwrap_or_default()
}

pub fn load_config_from_path(path: &Path) -> Option<AppConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    Some(parse_config(&raw, path))
}

pub fn parse_config(raw: &str, source: &Path) -> AppConfig {
    toml::from_str(raw).unwrap_or_else(|err| {
        eprintln!(
            "[kuberift] warning: cannot parse config '{}': {err}",
            source.display()
        );
        AppConfig::default()
    })
}
