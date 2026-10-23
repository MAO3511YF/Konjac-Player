use tauri::{AppHandle, Emitter, Manager};

use crate::browser;
use crate::settings::Site;
use crate::state::AppState;

/// Event the shell listens to; the payload is the action's id.
pub const EVENT: &str = "browser://menu";

/// The shell's title row is HTML now, so the menus are drawn by the app itself
/// and every pick comes back through here. `site.N` still resolves against the
/// library the shell last published, so a menu row and the settings page cannot
/// disagree about which site is number three.
pub fn remember(app: &AppHandle, sites: &[Site]) {
    *app.state::<AppState>().menu_sites.lock().unwrap() = sites.to_vec();
}

/// Resolves a `site.N` id against the library the shell last published, so a
/// menu row and the settings page cannot disagree about which site is number
/// three. The caller opens it — never from this thread.
pub fn site_for(app: &AppHandle, id: &str) -> Option<Site> {
    let state = app.state::<AppState>();
    let i = id.strip_prefix("site.")?.parse::<usize>().ok()?;
    state.menu_sites.lock().unwrap().get(i).cloned()
}

/// Hand a pick the shell has to act on back to the title row. Site entries are
/// answered here by the caller, because only they may build a webview, and not
/// on the thread a synchronous command runs on.
pub fn send(app: &AppHandle, id: &str) {
    app.state::<AppState>().record(&format!("menu {id}"));
    let _ = app.emit_to(browser::SHELL, EVENT, id.to_string());
}
