//! The corner of the screen: an icon in the notification area that lets the app
//! keep running with no window on screen at all.
//!
//! Two different hides live here and in `browser`, and they must stay apart. The
//! boss key pauses and mutes because it is pretending nobody is watching a
//! video; folding into the tray keeps the sound going, because that is the whole
//! reason to leave the app running with its windows closed.

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::browser;
use crate::error::AppError;

pub const TRAY: &str = "vb-tray";

/// Puts the icon in the notification area. Called once from setup; a machine
/// whose shell refuses the icon must still start the app, so the caller only
/// logs a failure here.
pub fn install(app: &AppHandle) -> Result<(), AppError> {
    let m_show = MenuItemBuilder::with_id("tray.show", "显示魔芋播放器").build(app)?;
    let m_hide = MenuItemBuilder::with_id("tray.hide", "缩到右下角（声音继续）").build(app)?;
    let m_pause = MenuItemBuilder::with_id("tray.pause", "全部暂停并静音").build(app)?;
    let m_quit = MenuItemBuilder::with_id("tray.quit", "退出魔芋播放器").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&m_show, &m_hide, &m_pause])
        .separator()
        .item(&m_quit)
        .build()?;

    let mut builder = TrayIconBuilder::with_id(TRAY)
        .menu(&menu)
        // Windows shows the menu on right click by itself; a left click is the
        // gesture people use to bring the app back.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            // Menu events are already delivered on the event-loop thread, so the
            // window calls go straight through — routing them back through
            // `on_main` would block this thread waiting for itself.
            run(app, id.strip_prefix("tray.").unwrap_or(""));
        })
        .on_tray_icon_event(|icon, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                show(icon.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).tooltip("魔芋播放器");
    }
    builder.build(app)?;
    app.state::<crate::state::AppState>()
        .record("tray icon installed");
    Ok(())
}

/// One tray action, on the event-loop thread.
pub fn run(app: &AppHandle, action: &str) {
    match action {
        "show" => show(app),
        "hide" => browser::hide_to_tray(app),
        "pause" => browser::silence_all(app),
        "quit" => browser::quit(app),
        _ => {}
    }
}

/// Brings the windows back, undoing a boss-key hide as well as a trip to the
/// tray. `show_everything` already focuses the main window and re-lays-out the
/// pages, so there is nothing to add here.
fn show(app: &AppHandle) {
    browser::show_everything(app);
}
