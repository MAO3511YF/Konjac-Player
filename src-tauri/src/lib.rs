mod browser;
mod commands;
mod error;
mod menu;
mod settings;
mod state;
mod tray;

use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let state = app.state::<state::AppState>();
                    if state.boss_shortcut.lock().unwrap().as_ref() != Some(shortcut) {
                        return;
                    }
                    if *state.boss_hidden.lock().unwrap() {
                        browser::show_everything(app);
                    } else if let Err(e) = browser::hide_everything(app) {
                        state.record(&format!("boss hide refused: {e}"));
                    }
                })
                .build(),
        )
        .manage(state::AppState::default())
        // The title row's menus are system popups now, so their picks arrive
        // here rather than from a click on HTML. Spaced off the event loop: a
        // site entry builds a webview, and that cannot happen on this thread.
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            // The tray answers its own rows, and its handler already runs on the
            // event loop; forwarding them here would run each pick twice.
            if id.starts_with("tray.") {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::run_menu(app, id).await;
            });
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let saved = settings::load(&handle);
            // The stored sidebar width decides where the pages start, so it has
            // to be in place before the first layout runs.
            if let Some(width) = saved.sidebar_width {
                *handle
                    .state::<state::AppState>()
                    .sidebar_width
                    .lock()
                    .unwrap() =
                    width.clamp(browser::SIDEBAR_MIN, browser::SIDEBAR_MAX);
            }
            *handle
                .state::<state::AppState>()
                .close_to_tray
                .lock()
                .unwrap() = saved.close_to_tray.unwrap_or(true);
            browser::create_main_window(&handle)?;
            // A shell that refuses the icon must not cost the user the app: with
            // no tray in place the ✕ falls back to quitting on its own.
            if let Err(e) = tray::install(&handle) {
                handle
                    .state::<state::AppState>()
                    .record(&format!("tray refused: {e}"));
            }

            let state = handle.state::<state::AppState>();
            let combo = saved
                .boss_key
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| commands::DEFAULT_BOSS_KEY.into());
            // A combo another app grabbed in the meantime must not cost the user
            // the only way back from a hidden app.
            let applied = commands::apply_boss_key(&handle, &state, &combo)
                .or_else(|_| commands::apply_boss_key(&handle, &state, commands::DEFAULT_BOSS_KEY));
            state.record(match applied {
                Ok(text) => format!("boss shortcut {text} registered"),
                Err(_) => "boss shortcut NOT registered".into(),
            });
            drop(state);

            let main = handle.get_window(browser::MAIN).expect("main window");
            let handle2 = handle.clone();
            main.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Resized(_)) {
                    browser::main_resized(&handle2);
                }
            });

            browser::emit_tabs(&handle);
            browser::spawn_playback_poller(&handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_tab,
            commands::close_tab,
            commands::activate_tab,
            commands::list_tabs,
            commands::detach_mini,
            commands::attach_back,
            commands::set_mute,
            commands::mini_size,
            commands::mini_ctl,
            commands::mini_topmost,
            commands::page_nav,
            commands::pages_visible,
            commands::set_site_menu,
            commands::set_main_size,
            commands::set_boss_key,
            commands::get_settings,
            commands::save_settings,
            commands::close_all,
            commands::quit,
            commands::window_control,
            commands::popup_menu,
            commands::app_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
