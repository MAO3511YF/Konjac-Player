use std::sync::Mutex;

use tauri::Webview;

/// What the page itself reports about its media, sampled by `browser::PLAY_PROBE`.
#[derive(Clone, Copy, Default)]
pub struct Play {
    /// a probe has answered since the tab opened
    pub live: bool,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub muted: bool,
    pub videos: usize,
    /// the player's own width/height, 0 until the page reports one
    pub aspect: f64,
    /// a swipe feed (抖音/快手): one full-screen item the page flips through,
    /// rather than a player set into a document that scrolls
    pub feed: bool,
    /// the speed the player is actually running at
    pub rate: f64,
}

pub struct Tab {
    pub label: String,
    pub url: String,
    pub title: String,
    pub webview: Webview,
}

pub struct AppState {
    pub tabs: Mutex<Vec<Tab>>,
    pub active: Mutex<Option<String>>,
    /// tab label currently parked in the mini window
    pub mini_tab: Mutex<Option<String>>,
    /// How many times the page in the mini window has been right-clicked. The
    /// page counts them; a right click means "show the app again".
    pub mini_ctx: Mutex<i64>,
    /// Whether the pages are on screen, or hidden behind the settings page.
    pub pages_visible: Mutex<bool>,
    /// The small window's own frame — x, y, width, height in logical screen
    /// units, as of the last resize it was asked for. A page that goes 全屏
    /// hands this window to the whole monitor, and the frame is what comes back.
    pub mini_frame: Mutex<(f64, f64, f64, f64)>,
    /// The library the menu bar's 站点 submenu is showing, in menu order.
    pub menu_sites: Mutex<Vec<crate::settings::Site>>,
    /// last believable inner size per window label, in logical pixels
    pub sizes: Mutex<std::collections::HashMap<String, (f64, f64)>>,
    /// latest playback sample per tab label, written from the page-probe
    /// callback. Kept apart from `Tab` because that callback can be delivered
    /// while a command on the same thread already holds the tab list, and
    /// `std::sync::Mutex` is not reentrant.
    pub play: Mutex<std::collections::HashMap<String, Play>>,
    pub sidebar_width: Mutex<f64>,
    pub boss_hidden: Mutex<bool>,
    /// Whether the boss key actually reached the OS. Hiding without it would
    /// leave the user with an app that has no window and no way back.
    pub boss_key: Mutex<bool>,
    /// The boss key as the user set it, in the label form shown in the sidebar.
    pub boss_combo: Mutex<String>,
    /// Parsed `boss_combo`; the global-shortcut handler compares against it.
    pub boss_shortcut: Mutex<Option<tauri_plugin_global_shortcut::Shortcut>>,
    /// What the ✕ does: fold into the corner tray, or quit. Mirrors the setting
    /// of the same name and is read on the event loop, where no file IO belongs.
    pub close_to_tray: Mutex<bool>,
    pub log: Mutex<Vec<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tabs: Mutex::new(Vec::new()),
            active: Mutex::new(None),
            mini_tab: Mutex::new(None),
            mini_ctx: Mutex::new(0),
            pages_visible: Mutex::new(true),
            mini_frame: Mutex::new((0.0, 0.0, 0.0, 0.0)),
            menu_sites: Mutex::new(Vec::new()),
            sizes: Mutex::new(std::collections::HashMap::new()),
            play: Mutex::new(std::collections::HashMap::new()),
            sidebar_width: Mutex::new(232.0),
            boss_hidden: Mutex::new(false),
            boss_key: Mutex::new(false),
            boss_combo: Mutex::new(crate::commands::DEFAULT_BOSS_KEY.into()),
            boss_shortcut: Mutex::new(None),
            close_to_tray: Mutex::new(true),
            log: Mutex::new(Vec::new()),
        }
    }
}

impl AppState {
    pub fn record(&self, msg: impl Into<String>) {
        let line = format!("{} {}", clock(), msg.into());
        eprintln!("[vb] {line}");
        let mut log = self.log.lock().unwrap();
        log.push(line);
        if log.len() > 200 {
            log.remove(0);
        }
    }
}

/// HH:MM:SS, so log lines can be matched against screenshot file names.
fn clock() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() % 86400)
        .unwrap_or(0);
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}
