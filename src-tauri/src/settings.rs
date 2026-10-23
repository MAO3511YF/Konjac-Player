use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::AppError;

/// One entry of the user's site library.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Site {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Global shortcut that hides the whole app, e.g. `Ctrl+Shift+H`.
    #[serde(alias = "boss_key", skip_serializing_if = "Option::is_none")]
    pub boss_key: Option<String>,
    /// The 常用站点 library; an empty list falls back to the built-in defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sites: Option<Vec<Site>>,
    /// Expanded sidebar width in logical pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f64>,
    /// Whether the portrait button folds the sidebar away for the narrow frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portrait_folds: Option<bool>,
    /// Shape a page takes when it is moved into the mini window: `port` or `land`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_ratio: Option<String>,
    /// Strip the page down to its player inside the mini window. Off leaves the
    /// site's own layout intact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_cinema: Option<bool>,
    /// Shell text size as a multiplier; the pages keep their own zoom.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f64>,
    /// Site hosts whose page group is folded in the sidebar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folded_groups: Option<Vec<String>>,
    /// The ✕ puts the app in the bottom-right corner instead of quitting, so a
    /// video can keep playing in the background. Off keeps the old quit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_to_tray: Option<bool>,
}

fn file(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    Ok(app.path().app_config_dir()?.join("settings.json"))
}

/// A corrupt or unreadable settings file must not stop the app from starting.
pub fn load(app: &AppHandle) -> Settings {
    file(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), AppError> {
    let path = file(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

/// Writes only the fields the caller sent, so a partial save from one settings
/// row cannot erase what another row stored.
pub fn update(app: &AppHandle, patch: Settings) -> Result<Settings, AppError> {
    let mut current = load(app);
    if patch.boss_key.is_some() {
        current.boss_key = patch.boss_key;
    }
    if patch.sites.is_some() {
        current.sites = patch.sites;
    }
    if patch.sidebar_width.is_some() {
        current.sidebar_width = patch.sidebar_width;
    }
    if patch.portrait_folds.is_some() {
        current.portrait_folds = patch.portrait_folds;
    }
    if patch.mini_ratio.is_some() {
        current.mini_ratio = patch.mini_ratio;
    }
    if patch.mini_cinema.is_some() {
        current.mini_cinema = patch.mini_cinema;
    }
    if patch.font_scale.is_some() {
        current.font_scale = patch.font_scale;
    }
    if patch.folded_groups.is_some() {
        current.folded_groups = patch.folded_groups;
    }
    if patch.close_to_tray.is_some() {
        current.close_to_tray = patch.close_to_tray;
    }
    save(app, &current)?;
    Ok(current)
}
