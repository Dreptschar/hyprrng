use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub grid_size: Option<GridSize>,
    pub keybindings: Option<Keybindings>,
    pub grid_color: Option<GridColor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GridSize {
    pub rows: usize,
    pub cols: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Keybindings {
    pub cells: Option<Vec<Vec<String>>>,
    pub backspace: Option<String>,
    pub esc: Option<String>,
    pub enter: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GridColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grid_size: None,
            keybindings: None,
            grid_color: None,
        }
    }
}

pub fn load_config() -> Config {
    let path = config_path();
    if let Some(path) = path {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(cfg) = toml::from_str::<Config>(&contents) {
                return cfg;
            }
        }
    }
    Config::default()
}

fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HYPRRGN_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/hyprrgn/config.toml"))
}
