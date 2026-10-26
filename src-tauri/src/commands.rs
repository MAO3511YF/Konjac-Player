use std::sync::mpsc;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut};

use crate::browser::{self, TabInfo};
use crate::error::{AppError, CmdResult};
use crate::menu;
use crate::settings::{self, Settings, Site};
use crate::state::AppState;

pub const DEFAULT_BOSS_KEY: &str = "Ctrl+Shift+H";

/// Runs a window/webview call on the event-loop thread and waits for it.
///
/// Tauri executes synchronous commands on the main thread, and creating or
/// destroying a WebView2 inside such a handler deadlocks: the completion is
/// posted to the very loop the handler is blocking. Commands therefore run as
/// `async` (worker threads) and hand the real work over here.
fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(AppHandle) -> T + Send + 'static,
) -> CmdResult<T> {
    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(f(handle));
    })?;
    rx.recv()
        .map_err(|_| AppError::Message("界面线程没有响应，请重启程序".into()))
}

#[tauri::command]
pub async fn open_tab(app: AppHandle, url: String) -> CmdResult<String> {
    let url = normalize_url(&url).map_err(AppError::Message)?;
    on_main(&app, move |app| browser::open_tab(&app, &url).map_err(Into::into))?
}

#[tauri::command]
pub async fn close_tab(app: AppHandle, label: String) -> CmdResult<()> {
    on_main(&app, move |app| browser::close_tab(&app, &label).map_err(Into::into))?
}

#[tauri::command]
pub async fn activate_tab(app: AppHandle, label: String) -> CmdResult<()> {
    on_main(&app, move |app| browser::activate(&app, &label).map_err(Into::into))?
}

#[tauri::command]
pub fn list_tabs(app: AppHandle) -> CmdResult<Vec<TabInfo>> {
    Ok(browser::tab_infos(&app))
}

#[tauri::command]
pub async fn detach_mini(app: AppHandle, label: String, shape: Option<String>) -> CmdResult<()> {
    on_main(&app, move |app| {
        browser::detach_mini(&app, &label, shape.as_deref()).map_err(Into::into)
    })?
}

#[tauri::command]
pub async fn attach_back(app: AppHandle) -> CmdResult<()> {
    on_main(&app, move |app| browser::attach_back(&app).map_err(Into::into))?
}

/// Silences one tab by label, so the sidebar's button can reach a page that is
/// not in front. The page stays the source of truth: the playback poll reads
/// `video.muted` back out, which is what keeps the small window's speaker and
/// this button showing the same thing.
#[tauri::command]
pub async fn set_mute(app: AppHandle, label: String, on: bool) -> CmdResult<()> {
    on_main(&app, move |app| {
        browser::set_tab_mute(&app, &label, on).map_err(Into::into)
    })?
}

#[tauri::command]
pub async fn mini_size(app: AppHandle, width: f64, height: f64) -> CmdResult<()> {
    on_main(&app, move |app| {
        browser::set_mini_size(&app, width, height);
    })?;
    Ok(())
}

/// A button on the strip beside the small window. The strip is its own webview
/// and cannot reach into the page next to it, so every button arrives here as a
/// name and is handed to the bridge installed in that page.
#[tauri::command]
pub async fn mini_ctl(app: AppHandle, what: String) -> CmdResult<()> {
    on_main(&app, move |app| {
        browser::mini_ctl(&app, &what).map_err(Into::into)
    })?
}

#[tauri::command]
pub async fn mini_topmost(app: AppHandle, on: bool) -> CmdResult<()> {
    on_main(&app, move |app| {
        browser::toggle_mini_topmost(&app, on);
    })?;
    Ok(())
}

/// Steps the page in front through its own history.
#[tauri::command]
pub async fn page_nav(app: AppHandle, dir: String) -> CmdResult<()> {
    on_main(&app, move |app| browser::page_nav(&app, &dir).map_err(Into::into))?
}

#[tauri::command]
pub async fn pages_visible(app: AppHandle, on: bool) -> CmdResult<()> {
    on_main(&app, move |app| browser::set_pages_visible(&app, on).map_err(Into::into))?
}

/// Rebuilds the native 站点 submenu from the library the shell is showing.
#[tauri::command]
pub async fn set_site_menu(app: AppHandle, sites: Vec<Site>) -> CmdResult<()> {
    menu::remember(&app, &sites);
    Ok(())
}

/// Reads the settings file. Async so the read happens on the command pool: a
/// synchronous command body runs on the event-loop thread, and that thread is
/// the one that has to keep the pages moving.
#[tauri::command]
pub async fn get_settings(app: AppHandle) -> CmdResult<Settings> {
    Ok(settings::load(&app))
}

/// Saves whatever fields a settings row changed and applies the ones that have
/// an immediate effect, so the panel only needs this one call.
#[tauri::command]
pub async fn save_settings(app: AppHandle, patch: Settings) -> CmdResult<Settings> {
    let mut patch = patch;
    if let Some(width) = patch.sidebar_width {
        let handle = app.clone();
        on_main(&handle, move |app| {
            browser::set_sidebar_width(&app, width);
        })?;
        // The file keeps the width the pages actually got, not the one asked
        // for: the shell paints its own column from this value, and the two
        // disagreeing is what leaves a strip of shell beside every page.
        patch.sidebar_width = Some(*app.state::<AppState>().sidebar_width.lock().unwrap());
    }
    if let Some(to_tray) = patch.close_to_tray {
        // The ✕ reads this on the event loop, where loading the file would be
        // out of place, so the cached copy is refreshed alongside the save.
        *app.state::<AppState>().close_to_tray.lock().unwrap() = to_tray;
    }
    Ok(settings::update(&app, patch)?)
}

#[tauri::command]
pub async fn set_main_size(app: AppHandle, width: f64, height: f64) -> CmdResult<()> {
    on_main(&app, move |app| {
        browser::set_main_size(&app, width, height);
    })?;
    Ok(())
}

#[tauri::command]
pub async fn close_all(app: AppHandle) -> CmdResult<()> {
    on_main(&app, move |app| browser::close_all(&app).map_err(Into::into))?
}

#[tauri::command]
pub async fn quit(app: AppHandle) -> CmdResult<()> {
    let _ = on_main(&app, move |app| browser::quit(&app));
    Ok(())
}

/// The title row is HTML, so its three window buttons need a way in. They go
/// through Rust rather than `@tauri-apps/api` to keep the shell webview's
/// capability list free of window-control grants.
#[tauri::command]
pub async fn window_control(app: AppHandle, what: String) -> CmdResult<()> {
    on_main(&app, move |app| {
        // 收进托盘 is the one that is not about this window alone: it has to leave
        // the small window floating, so `browser` decides whether it is allowed.
        if what == "tuck" {
            let on = !browser::main_tucked(&app);
            return browser::tuck_main(&app, on);
        }
        let Some(win) = app.get_window(browser::MAIN) else {
            return Ok(());
        };
        match what.as_str() {
            "min" => win.minimize()?,
            "max" => {
                if win.is_maximized().unwrap_or(false) {
                    win.unmaximize()?
                } else {
                    win.maximize()?
                }
            }
            "close" => win.close()?,
            other => return Err(AppError::Message(format!("未知窗口操作 {other}"))),
        }
        Ok(())
    })?
}

/// Whether the main window is folded into the corner right now, so the small
/// window's rail and the title row light up the same button for the same state.
#[tauri::command]
pub fn main_tucked(app: AppHandle) -> CmdResult<bool> {
    Ok(browser::main_tucked(&app))
}

/// A row of a title-row menu, as the shell describes it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupItem {
    id: String,
    label: String,
    /// Group name drawn above this row. Only the combined 菜单 button uses it,
    /// because that one shows all five menus at once.
    heading: Option<String>,
}

/// Draws one of the title-row menus with the system, under the button that was
/// clicked.
///
/// The site pages are separate windows stacked over the shell, so an HTML
/// dropdown is either buried under them or has to take them out of the way —
/// both read as a bug. A system menu is drawn above every window, so the video
/// stays exactly where it is.
#[tauri::command]
pub async fn popup_menu(app: AppHandle, items: Vec<PopupItem>, x: f64, y: f64) -> CmdResult<()> {
    use tauri::menu::{ContextMenu, IsMenuItem, Menu, MenuItemBuilder, MenuItemKind, PredefinedMenuItem};

    let mut rows: Vec<MenuItemKind<_>> = Vec::new();
    for (n, item) in items.iter().enumerate() {
        if let Some(heading) = item.heading.as_ref() {
            if !rows.is_empty() {
                rows.push(PredefinedMenuItem::separator(&app)?.kind());
            }
            rows.push(
                MenuItemBuilder::with_id(format!("heading.{n}"), heading)
                    .enabled(false)
                    .build(&app)?
                    .kind(),
            );
        }
        rows.push(
            MenuItemBuilder::with_id(item.id.clone(), &item.label)
                .build(&app)?
                .kind(),
        );
    }
    let rows: Vec<&dyn IsMenuItem<_>> = rows.iter().map(|r| r as &dyn IsMenuItem<_>).collect();
    let menu = Menu::with_items(&app, &rows)?;
    let window = app
        .get_window(browser::MAIN)
        .ok_or_else(|| AppError::Message("主窗口还没打开".into()))?;
    // The shell webview fills the window's client area from its top-left, so the
    // button's own rectangle is already in the coordinates the menu wants.
    menu.popup_at(window, tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))?;
    Ok(())
}

/// A title-row menu pick.
///
/// A site entry builds a webview, so it goes through `on_main` like every other
/// page call: a synchronous command runs on the event-loop thread, and a
/// WebView2 created there waits for a completion that thread cannot deliver.
/// Called from the menu handler in `lib.rs`, never from the shell.
pub async fn run_menu(app: AppHandle, id: String) -> CmdResult<()> {
    if let Some(site) = menu::site_for(&app, &id) {
        // A hand-edited library row can arrive without a scheme, and `Url::parse`
        // would refuse it with nowhere for the error to surface: the menu row
        // would simply look dead.
        if let Ok(url) = normalize_url(&site.url) {
            let _ = on_main(&app, move |app| browser::open_tab(&app, &url).map(|_| ()))?;
        }
    }
    // Every pick is reported back, so the row can bring a freshly opened page
    // in front of the settings sheet.
    menu::send(&app, &id);
    Ok(())
}

/// The shell's own running-log tail, shown under 设置 → 运行日志.
#[tauri::command]
pub fn app_log(app: AppHandle) -> CmdResult<Vec<String>> {
    Ok(app.state::<crate::state::AppState>().log.lock().unwrap().clone())
}

/// Registers `combo` as the boss key and remembers it.
///
/// The new combo is registered before the old one is dropped, so a combo another
/// program already owns leaves the working shortcut in place. Never call this
/// from the event-loop thread: the plugin itself round-trips to that thread.
pub fn apply_boss_key(app: &AppHandle, state: &AppState, combo: &str) -> Result<String, AppError> {
    let parsed: Shortcut = combo
        .trim()
        .parse()
        .map_err(|_| AppError::Message("无法识别的组合键，请用类似 Ctrl+Shift+H 的写法".into()))?;
    let text = pretty(&parsed);
    let shortcuts = app.global_shortcut();
    if state.boss_shortcut.lock().unwrap().as_ref() != Some(&parsed) {
        shortcuts
            .register(parsed)
            .map_err(|_| AppError::Message("这个组合键已被其他程序占用，换一个试试".into()))?;
        if let Some(old) = state.boss_shortcut.lock().unwrap().replace(parsed) {
            let _ = shortcuts.unregister(old);
        }
    }
    *state.boss_combo.lock().unwrap() = text.clone();
    *state.boss_key.lock().unwrap() = true;
    // A patch, not a whole write: the site library and the panel width live in
    // the same file and must survive a shortcut change.
    settings::update(
        app,
        Settings {
            boss_key: Some(text.clone()),
            ..Settings::default()
        },
    )?;
    state.record(&format!("boss shortcut {text}"));
    Ok(text)
}

/// Renders a parsed shortcut the way the parser wants to read it back, so the
/// label in the sidebar is also a valid definition.
fn pretty(shortcut: &Shortcut) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if shortcut.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        parts.push("Alt");
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift");
    }
    if shortcut.mods.contains(Modifiers::SUPER) {
        parts.push("Super");
    }
    let key = format!("{:?}", shortcut.key);
    parts.push(
        key.strip_prefix("Key")
            .or_else(|| key.strip_prefix("Digit"))
            .unwrap_or(&key),
    );
    parts.join("+")
}

#[tauri::command]
pub async fn set_boss_key(
    app: AppHandle,
    state: State<'_, AppState>,
    combo: String,
) -> CmdResult<String> {
    Ok(apply_boss_key(&app, state.inner(), &combo)?)
}

pub fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("地址为空".into());
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(raw.to_string());
    }
    if raw.contains('.') && !raw.contains(' ') {
        return Ok(format!("https://{raw}"));
    }
    Ok(format!(
        "https://www.bing.com/search?q={}",
        url::form_urlencoded::byte_serialize(raw.as_bytes()).collect::<String>()
    ))
}
