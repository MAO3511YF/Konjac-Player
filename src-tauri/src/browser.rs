use std::time::Duration;

use tauri::webview::WebviewBuilder;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl, Window,
};

use crate::error::AppError;
use crate::state::{AppState, Play, Tab};

pub const MAIN: &str = "main";
pub const SHELL: &str = "shell";
pub const MINI: &str = "mini";
pub const MINI_BAR: &str = "mini-bar";
/// The small window's own control column, standing to the *right* of the video.
/// It is our webview, not an overlay: the page owns everything to the left of the
/// rail, so nothing drawn here can ever cover 点赞 or the site's own progress bar.
/// Narrow on purpose — the viewer asked for the video, not for a toolbar.
pub const MINI_RAIL_W: f64 = 43.0;

/// The shell webview covers the whole window and paints its own chrome: a menu
/// bar along the top and a narrow rail down the right. Pages are laid out inside
/// the frame those leave, so the two numbers have to match the CSS.
pub const APP_TITLE: &str = "魔芋播放器";
pub const TOP_CHROME: f64 = 40.0;
pub const RIGHT_RAIL: f64 = 54.0;

/// The span the sidebar slider may set. `save_settings` clamps a stored width
/// into it before writing, so the number in the settings file is the number the
/// pages are laid out at and the number the shell paints its column with — one
/// width, not three that can drift apart.
pub const SIDEBAR_MIN: f64 = 52.0;
pub const SIDEBAR_MAX: f64 = 420.0;

/// Chromium args shared by every site webview. They must stay identical across
/// webviews, because webviews that differ here cannot share a WebView2 profile.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--disable-backgrounding-occluded-windows \
--disable-renderer-backgrounding --disable-background-timer-throttling";

/// Dev-only: Chromium's debug port is claimed per WebView2 process, and the
/// process holding the site pages is not the one holding the shell (different
/// data directories), so each gets its own port. Release builds expose neither.
fn debug_port(port: &str) -> String {
    if cfg!(debug_assertions) {
        // Repeats wry's default argument set: replacing additional args would
        // otherwise silently drop it.
        format!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --remote-debugging-port={port} --remote-allow-origins=*"
        )
    } else {
        String::new()
    }
}

/// Site webviews share one WebView2 profile, so they must also share one set of
/// Chromium args — including their debug port, separate from the shell's.
fn browser_args() -> String {
    let mut args = BROWSER_ARGS.to_string();
    let extra = debug_port("9224");
    if !extra.is_empty() {
        args.push(' ');
        args.push_str(&extra);
    }
    args
}

/// Debug builds only: gives a shell webview its WebView2 process's debug port.
/// Release builds keep Tauri's own argument set.
#[cfg_attr(not(debug_assertions), allow(unused_variables))] // the port is a debug-only concept
fn with_debug_port(
    builder: WebviewBuilder<tauri::Wry>,
    port: &str,
) -> WebviewBuilder<tauri::Wry> {
    #[cfg(debug_assertions)]
    return builder.additional_browser_args(&debug_port(port));
    #[cfg(not(debug_assertions))]
    return builder;
}

fn logical_size(app: &AppHandle, label: &str) -> (f64, f64) {
    let read = match app.get_window(label) {
        Some(w) => {
            let scale = w.scale_factor().unwrap_or(1.0);
            w.inner_size()
                .map(|s| (s.width as f64 / scale, s.height as f64 / scale))
                .unwrap_or((0.0, 0.0))
        }
        None => (0.0, 0.0),
    };
    // A hidden or minimised window reports 0x0. Laying out against that shrinks
    // the webviews to a sliver that never grows back on its own, so the last
    // believable size is remembered per window.
    let fallback: (f64, f64) = if label == MINI {
        (460.0, 280.0)
    } else {
        (1180.0, 740.0)
    };
    let state = app.state::<AppState>();
    let mut sizes = state.sizes.lock().unwrap();
    let value = if read.0 >= 1.0 && read.1 >= 1.0 {
        read
    } else {
        sizes.get(label).copied().unwrap_or(fallback)
    };
    sizes.insert(label.to_string(), value);
    value
}

fn place(webview: &Webview, x: f64, y: f64, w: f64, h: f64) {
    // One atomic bounds write: separate position/size calls have been observed
    // to leave WebView2 half-applied after a reparent.
    let _ = webview.set_bounds(tauri::Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.0), h.max(1.0)).into(),
    });
}

// Note on geometry: `Webview::bounds` answers in the coordinates of the window
// a webview was *created* in and keeps doing so after `reparent`, so a page
// moved into the small window reads back as though it were still offset by the
// distance between the two windows. `set_bounds` is not like that — it places
// against the current parent — so "correcting" the difference that is measured
// turns a correct layout into one that pushes the page off screen. That
// correction, not the site, was the "小窗识别不到视频位置" report.

/// Content area inside the shell's menu bar, sidebar and right rail.
fn content_rect(app: &AppHandle) -> (f64, f64, f64, f64) {
    let side = *app.state::<AppState>().sidebar_width.lock().unwrap();
    let (w, h) = logical_size(app, MAIN);
    (
        side,
        TOP_CHROME,
        (w - side - RIGHT_RAIL).max(200.0),
        (h - TOP_CHROME).max(160.0),
    )
}

/// Usable desktop area in the logical units of `win`.
///
/// A window built at a fixed logical size spills off high-DPI screens, which
/// pushes the sidebar footer and the mini window's controls out of reach. The
/// divisor is the window's own scale factor rather than the monitor's: the two
/// disagree here (monitor 1.0, window 1.5), and dividing by the monitor's value
/// turns the clamp into a no-op.
fn desktop(win: &Window<tauri::Wry>) -> (f64, f64, f64, f64) {
    let scale = win.scale_factor().unwrap_or(1.0).max(0.5);
    let (x, y, w, h) = match win.primary_monitor().ok().flatten() {
        Some(m) => {
            let a = m.work_area();
            (
                a.position.x as f64,
                a.position.y as f64,
                a.size.width as f64,
                a.size.height as f64,
            )
        }
        None => (0.0, 0.0, 1280.0, 800.0),
    };
    (x / scale, y / scale, w / scale, h / scale)
}

/// Pulls a window back inside the desktop. A top-left anchored window grows off
/// the right and bottom edges when it is resized, which is how the mini
/// window's control bar ends up unreachable.
fn keep_on_screen(win: &Window<tauri::Wry>) {
    let scale = win.scale_factor().unwrap_or(1.0).max(0.5);
    let (ax, ay, aw, ah) = desktop(win);
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
        return;
    };
    let (x, y) = (pos.x as f64 / scale, pos.y as f64 / scale);
    let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
    let nx = x.clamp(ax + 8.0, (ax + aw - w - 8.0).max(ax + 8.0));
    let ny = y.clamp(ay + 8.0, (ay + ah - h - 8.0).max(ay + 8.0));
    if (nx - x).abs() > 1.0 || (ny - y).abs() > 1.0 {
        let _ = win.set_position(LogicalPosition::new(nx, ny));
    }
}

/// The small window's own frame: where it stood and how big its content was the
/// last time it was a size it chose for itself, in logical screen units.
///
/// The size is the *inner* one because that is what the picture and the rail are
/// laid out to, and a window that has been through a page's 全屏 comes back with
/// its invisible resize border folded into the client area — so outer and inner
/// are no longer the same numbers they were before.
fn remember_mini_frame(app: &AppHandle, win: &Window<tauri::Wry>) {
    let scale = win.scale_factor().unwrap_or(1.0).max(0.5);
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) else {
        return;
    };
    let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
    if w < 1.0 || h < 1.0 {
        return;
    }
    let state = app.state::<AppState>();
    let mut frame = state.mini_frame.lock().unwrap();
    *frame = (pos.x as f64 / scale, pos.y as f64 / scale, w, h);
}

/// Keeps the small window small.
///
/// A page that asks to go 全屏 — B 站's own button, or a double click on its
/// picture — is answered by WebView2 with a resize of the *window* hosting it to
/// the whole monitor, moved to its top-left corner. Inside the small window that
/// turns a corner player into a desktop-covering one that other windows cannot
/// come in front of, which is how the picture and its control bar end up on
/// either side of a file browser. The frame it stood in goes back; the page keeps
/// its fullscreen element, so the video now fills the small window instead of
/// the screen.
fn clamp_mini_frame(app: &AppHandle, win: &Window<tauri::Wry>) {
    let (_, _, aw, ah) = desktop(win);
    let scale = win.scale_factor().unwrap_or(1.0).max(0.5);
    let Ok(size) = win.inner_size() else {
        return;
    };
    let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
    if w < aw * 0.95 || h < ah * 0.95 {
        remember_mini_frame(app, win);
        return;
    }
    let state = app.state::<AppState>();
    let (px, py, pw, ph) = *state.mini_frame.lock().unwrap();
    if pw < 1.0 || (w - pw).abs() < 8.0 {
        return;
    }
    let _ = win.set_size(LogicalSize::new(
        pw.max(MINI_RAIL_W + 200.0) as u32,
        ph.max(170.0) as u32,
    ));
    let _ = win.set_position(LogicalPosition::new(px, py));
    // WebView2 settles the new bounds a beat after the call, and the layout
    // above ran against the size it was leaving, which left the picture 16 wide
    // too far under the rail. Same trick `detach_mini` uses.
    for delay in [180u64, 600] {
        let app = app.clone();
        schedule(delay, move || size_mini_content(&app));
    }
    state.record(&format!(
        "mini: the page's own 全屏 grew it to {w:.0}x{h:.0}; the {pw:.0}x{ph:.0} frame went back"
    ));
}

pub fn create_main_window(app: &AppHandle) -> Result<Window<tauri::Wry>, AppError> {
    let win = Window::builder(app, MAIN)
        .title(APP_TITLE)
        .inner_size(1180.0, 740.0)
        // low enough to allow the tall, narrow short-video frame
        .min_inner_size(420.0, 420.0)
        .position(40.0, 24.0)
        // The shell paints its own one-row title bar: brand, menus and window
        // buttons share the app's colours instead of sitting on an OS strip
        // above everything. Resizing survives because tao keeps the thick frame.
        .decorations(false)
        .build()?;

    let (ax, ay, aw, ah) = desktop(&win);
    let size = LogicalSize::new(1180.0_f64.min(aw * 0.9), 740.0_f64.min(ah * 0.9));
    app.state::<AppState>().record(&format!(
        "desktop {aw:.0}x{ah:.0} scale={:.2} -> main {:.0}x{:.0}",
        win.scale_factor().unwrap_or(1.0),
        size.width,
        size.height
    ));
    let _ = win.set_size(size);
    let _ = win.set_position(LogicalPosition::new(ax + 40.0, ay + 24.0));

    let handle = app.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            // The ✕ is "out of sight, still playing" while the corner icon is
            // there to bring it back; without one, closing means quitting, or
            // the user would be left with an app they cannot reach.
            let to_tray = {
                let state = handle.state::<AppState>();
                *state.close_to_tray.lock().unwrap()
            } && handle.tray_by_id(crate::tray::TRAY).is_some();
            if to_tray {
                api.prevent_close();
                hide_to_tray(&handle);
            } else {
                quit(&handle);
            }
        }
    });

    win.add_child(
        with_debug_port(
            WebviewBuilder::new(SHELL, WebviewUrl::App("index.html".parse().unwrap()))
                .devtools(cfg!(debug_assertions)),
            "9223",
        ),
        LogicalPosition::new(0.0, 0.0),
        // The shell is the window's whole client area: it paints the menu bar,
        // the sidebar and the right rail, and the pages sit on top of its centre.
        LogicalSize::new(size.width, size.height),
    )?;
    app.state::<AppState>().record("main window + shell created");
    Ok(win)
}

/// A minimised window reports a 0x0 client area, so laying out in response to
/// that resize would collapse every page into an invisible sliver.
pub fn main_resized(app: &AppHandle) {
    let minimized = app
        .get_window(MAIN)
        .map(|w| w.is_minimized().unwrap_or(false))
        .unwrap_or(false);
    if minimized {
        app.state::<AppState>().record("main minimised: layout skipped");
        return;
    }
    layout(app);
}

pub fn layout(app: &AppHandle) {
    let (w, h) = logical_size(app, MAIN);
    if let Some(shell) = app.get_webview(SHELL) {
        place(&shell, 0.0, 0.0, w, h);
    }
    let (x, y, w, hh) = content_rect(app);
    let state = app.state::<AppState>();
    let active = state.active.lock().unwrap().clone();
    let mini = state.mini_tab.lock().unwrap().clone();
    for tab in state.tabs.lock().unwrap().iter() {
        if Some(&tab.label) == mini.as_ref() {
            continue;
        }
        if Some(&tab.label) == active.as_ref() {
            place(&tab.webview, x, y, w, hh);
        }
    }
    size_mini_content(app);
}

pub fn open_tab(app: &AppHandle, url: &str) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let label = format!("tab-{}", next_id());
    let win = app.get_window(MAIN).ok_or_else(|| AppError::Message("主窗口缺失".into()))?;
    let profile = profile_dir(app, "shared")?;
    let (x, y, w, h) = content_rect(app);

    let builder = WebviewBuilder::new(&label, WebviewUrl::External(Url::parse(url)?))
        .additional_browser_args(&browser_args())
        .data_directory(profile)
        .devtools(cfg!(debug_assertions))
        .initialization_script(PAUSE_BRIDGE)
        .on_document_title_changed({
            let app = app.clone();
            let label = label.clone();
            move |_webview, title| {
                if let Some(tab) = app
                    .state::<AppState>()
                    .tabs
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|t| t.label == label)
                {
                    tab.title = title;
                }
                emit_tabs(&app);
            }
        })
        .on_page_load({
            let app = app.clone();
            let label = label.clone();
            move |webview, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    let l = webview.label().to_string();
                    app.state::<AppState>().record(&format!("finished load {l}"));
                    sync_url(&app, &label, &webview);
                    emit_tabs(&app);
                }
            }
        })
        .on_new_window({
            let app = app.clone();
            let label = label.clone();
            move |url, _features| {
                // The page is mid-click when this fires; building a webview inside
                // the callback re-enters the navigation that asked for it, so the
                // work is queued onto the next turn of the event loop.
                let app = app.clone();
                let from = label.clone();
                let target = url.clone();
                let _ = app
                    .clone()
                    .run_on_main_thread(move || follow_link(&app, &from, target.as_str()));
                tauri::webview::NewWindowResponse::Deny
            }
        });

    let webview = win.add_child(builder, LogicalPosition::new(x, y), LogicalSize::new(w, h))?;
    state.tabs.lock().unwrap().push(Tab {
        label: label.clone(),
        url: url.to_string(),
        title: String::new(),
        webview: webview.clone(),
    });
    state.record(&format!("opened {label} -> {url}"));
    watch_navigation(app, &webview, &label, url);
    activate(app, &label)?;
    Ok(label)
}

/// Re-navigates a page that was built but never started loading.
///
/// A webview created while the browser process is still releasing an earlier
/// controller can end up parked on `about:blank`: the tab exists and is
/// activated, yet shows nothing, which is how a video link looks like it will
/// not open. Asking the page for its own address is the only way to tell that
/// apart from a slow load, so it is asked twice and re-navigated if it is still
/// nowhere.
fn watch_navigation(app: &AppHandle, webview: &Webview, label: &str, url: &str) {
    for delay in [1500u64, 4000] {
        let app = app.clone();
        let label = label.to_string();
        let target = url.to_string();
        let webview = webview.clone();
        schedule(delay, move || {
            let probe = webview.clone();
            let name = label.clone();
            let again = webview.clone();
            let wanted = target.clone();
            let _ = probe.eval_with_callback("window.location.href", move |raw| {
                let here = raw.trim_matches('"').to_string();
                if !here.is_empty() && !here.starts_with("about:") {
                    return;
                }
                app.state::<AppState>()
                    .record(&format!("watchdog {name} stuck at {here:?}, re-navigating"));
                if let Ok(parsed) = Url::parse(&wanted) {
                    if let Err(e) = again.navigate(parsed) {
                        app.state::<AppState>()
                            .record(&format!("watchdog {name} navigate failed: {e}"));
                    }
                }
            });
        });
    }
}

/// A page asked to open `url` in a new window — a `target="_blank"` link, which
/// is how the video sites link from a feed to a player.
///
/// This is a player, not a browser: a click on a video means "play it here", so
/// the page navigates itself instead of being handed to a fresh tab. That also
/// sidesteps the race that made left clicks look dead — a webview built while
/// the page is still inside its own click handler can park on `about:blank`,
/// which reads as "the link opened nothing", while the same link taken from the
/// context menu a second later works because nothing else is in flight.
fn follow_link(app: &AppHandle, from: &str, url: &str) {
    let state = app.state::<AppState>();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        state.record(&format!("link from {from} ignored: {url}"));
        return;
    }
    let webview = {
        let mut tabs = state.tabs.lock().unwrap();
        match tabs.iter_mut().find(|t| t.label == from) {
            Some(tab) => {
                tab.url = url.to_string();
                tab.title.clear();
                tab.webview.clone()
            }
            None => {
                drop(tabs);
                let _ = open_tab(app, url);
                return;
            }
        }
    };
    state.record(&format!("link from {from} -> {url}"));
    navigate_until(app, &webview, from, url);
    emit_tabs(app);
}

/// Navigates a page and checks that it really moved.
///
/// A click on a feed card is handled by the site's own script before the host
/// ever sees it, and the navigation that follows is sometimes dropped: the tab
/// takes the new address while the window keeps showing the old page, which is
/// the "the list knows which video I clicked, nothing is playing" report. The
/// page is asked where it is a moment later and told again until it agrees, but
/// only while it stays on the site the link pointed at — a page that has moved
/// to another host was redirected on purpose.
fn navigate_until(app: &AppHandle, webview: &Webview, from: &str, url: &str) {
    let Some(wanted) = page_key(url) else { return };
    go_in_page(webview, url);
    // Once, after the page has had time to move. Asking twice in quick
    // succession is what turns one refused request into a pattern a risk-control
    // system notices, and a page that has landed on another host was redirected
    // on purpose.
    let delay = 2600u64;
    let app = app.clone();
    let label = from.to_string();
    let probe = webview.clone();
    let again = webview.clone();
    let target = url.to_string();
    let want = wanted.clone();
    schedule(delay, move || {
        let app = app.clone();
        let label = label.clone();
        let wanted = want.clone();
        let _ = probe.eval_with_callback("window.location.href", move |raw| {
            let here = raw.trim_matches('"').to_string();
            let key = page_key(&here);
            if key.as_ref() == Some(&wanted) {
                return;
            }
            if key.as_ref().map(|k| k.0.as_str()) != Some(wanted.0.as_str()) {
                return;
            }
            app.state::<AppState>()
                .record(&format!("link {label} still at {here:?}, asking the page once more"));
            go_in_page(&again, &target);
        });
    });
}

/// Moves a page by asking the page itself, not by telling the webview.
///
/// `Webview::navigate` is the address bar: the request goes out with no
/// `Referer`, and a top-level hit on a video page that arrives from nowhere is
/// exactly what B站's risk control answers with the 412 page. Script inside the
/// page carries the referrer and the cookie context the click would have had.
fn go_in_page(webview: &Webview, url: &str) {
    let Ok(literal) = serde_json::to_string(url) else {
        return;
    };
    let _ = webview.eval(&format!("window.location.href = {literal};"));
}

/// What identifies a page: its host and path, without the tracking query the
/// feeds tack onto every link.
fn page_key(url: &str) -> Option<(String, String)> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.trim_start_matches("www.").to_string();
    let path = u.path().trim_end_matches('/').to_string();
    if host.is_empty() {
        return None;
    }
    Some((host, path))
}

/// Keeps the address a tab shows in step with the page behind it.
///
/// These sites route inside the page: clicking a related video changes what is
/// playing without telling the host, so the address the tab was handed drifts
/// away from the one on screen and the list ends up naming another video.
fn sync_url(app: &AppHandle, label: &str, webview: &Webview) {
    let app = app.clone();
    let label = label.to_string();
    let _ = webview.eval_with_callback("window.location.href", move |raw| {
        let here = raw.trim_matches('"').to_string();
        if here.is_empty() || here.starts_with("about:") {
            return;
        }
        let changed = {
            let state = app.state::<AppState>();
            let mut tabs = state.tabs.lock().unwrap();
            match tabs.iter_mut().find(|t| t.label == label) {
                Some(tab) if tab.url != here => {
                    tab.url = here;
                    true
                }
                _ => false,
            }
        };
        if changed {
            emit_tabs(&app);
        }
    });
}

pub fn activate(app: &AppHandle, label: &str) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let (x, y, w, h) = content_rect(app);
    let mini = state.mini_tab.lock().unwrap().clone();
    {
        let tabs = state.tabs.lock().unwrap();
        for tab in tabs.iter() {
            // A tab parked in the mini window owns that window's layout; hiding
            // or repositioning it here would blank the small window.
            if Some(&tab.label) == mini.as_ref() {
                continue;
            }
            if tab.label == label {
                place(&tab.webview, x, y, w, h);
                tab.webview.show()?;
            } else {
                tab.webview.hide()?;
            }
        }
    }
    *state.active.lock().unwrap() = Some(label.to_string());
    *state.pages_visible.lock().unwrap() = true;
    layout(app);
    state.record(&format!("activate {label}"));
    emit_tabs(app);
    Ok(())
}

pub fn close_tab(app: &AppHandle, label: &str) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let victim = {
        let mut tabs = state.tabs.lock().unwrap();
        match tabs.iter().position(|t| t.label == label) {
            Some(i) => tabs.remove(i),
            None => return Ok(()),
        }
    };
    let was_mini = state.mini_tab.lock().unwrap().as_deref() == Some(label);
    if was_mini {
        *state.mini_tab.lock().unwrap() = None;
    }
    state.record(&format!("closed {label}"));
    state.play.lock().unwrap().remove(label);
    teardown(app, victim.webview);
    if was_mini {
        hide_mini(app);
    }
    let next = state.tabs.lock().unwrap().first().map(|t| t.label.clone());
    match next {
        Some(n) => activate(app, &n)?,
        None => {
            *state.active.lock().unwrap() = None;
            emit_tabs(app);
        }
    }
    Ok(())
}

/// Held while a controller is being released. WebView2 tears a webview down
/// over several turns of its own loop, and a create that overlaps it has been
/// seen to leave the new page on `about:blank`.
static TEARDOWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes a page down for good.
///
/// Two things make this need its own thread. Hiding first matters because wry
/// never shuts the WebView2 controller down, so a surface dropped while visible
/// keeps painting over the content area and eats the clicks of whichever tab
/// opens next. Running off the event-loop thread matters because a command body
/// already executes there, and `send_user_message` re-enters the handler from
/// that position while the window store is borrowed — the webview is then
/// resolved against a stale snapshot and the call fails.
fn teardown(app: &AppHandle, webview: Webview) {
    let app = app.clone();
    let label = webview.label().to_string();
    std::thread::spawn(move || {
        let _gate = TEARDOWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = webview.hide();
        std::thread::sleep(Duration::from_millis(120));
        match webview.close() {
            Ok(()) => app.state::<AppState>().record(&format!("tore down {label}")),
            Err(e) => app
                .state::<AppState>()
                .record(&format!("teardown {label} failed: {e}")),
        }
        // Let the process finish releasing before the next one is allowed in.
        std::thread::sleep(Duration::from_millis(250));
    });
}

fn hide_mini(app: &AppHandle) {
    if let Some(mini) = app.get_window(MINI) {
        let _ = mini.hide();
    }
}

/// Closes every page and exits. Deferred so the window-event callback that
/// triggers it returns before the loop is asked to stop.
pub fn quit(app: &AppHandle) {
    app.state::<AppState>().record("quit requested");
    // Teardown is serial — one page every ~370ms — so the exit has to wait for
    // as many pages as there are. Leaving at a fixed 120ms cut a six-tab shutdown
    // in the middle of it, which is the same thing as losing this run's logins.
    let pages = app.state::<AppState>().tabs.lock().unwrap().len() as u64;
    // The logins are cookies, and Chromium only commits that database on a
    // clean shutdown. Closing the pages first is what keeps a site signed in
    // across a restart; killing the process mid-teardown loses whatever was
    // signed in during this run.
    let _ = close_all(app);
    for label in [MAIN, MINI] {
        if let Some(w) = app.get_window(label) {
            let _ = w.hide();
        }
    }
    let grace = 400 + 370 * pages;
    let app = app.clone();
    schedule(grace, move || app.exit(0));
    // Backstop: if the graceful exit ever stalls on a WebView2 child process the
    // user is left with a hidden app they cannot close. Long enough that it
    // never lands in the middle of a shutdown that is going fine.
    schedule(grace + 6000, || std::process::exit(0));
}

/// Drops every page without exiting, so the sidebar can reset while the app
/// keeps running.
pub fn close_all(app: &AppHandle) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let victims = {
        let mut tabs = state.tabs.lock().unwrap();
        tabs.drain(..).collect::<Vec<Tab>>()
    };
    *state.mini_tab.lock().unwrap() = None;
    *state.active.lock().unwrap() = None;
    state.play.lock().unwrap().clear();
    state.record(&format!("closing {} tabs", victims.len()));
    for tab in victims {
        teardown(app, tab.webview);
    }
    hide_mini(app);
    emit_tabs(app);
    Ok(())
}

/// Tells a page it is in the small window, and hides everything but its player.
fn dress_for_mini(webview: &Webview, cinema: bool) {
    let _ = webview.eval("window.__vb && (window.__vb.inMini = true)");
    if cinema {
        let _ = webview.eval("window.__vb && window.__vb.cinema()");
    }
}

/// Tells a page it is back in the main window. It comes home paused: a video
/// still running while the feed is back on screen reads as two players fighting
/// over one tab.
fn undress_mini(webview: &Webview) {
    let _ = webview.eval(
        "window.__vb && (window.__vb.inMini = false, window.__vb.pause(), window.__vb.uncinema())",
    );
}

/// Move a page into the small always-on-top window. `shape` forces a portrait
/// or landscape frame; otherwise the player's own aspect ratio decides.
/// The page is not reloaded, so playback position, volume and login state all
/// survive.
pub fn detach_mini(app: &AppHandle, label: &str, shape: Option<&str>) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let webview = {
        let tabs = state.tabs.lock().unwrap();
        tabs.iter()
            .find(|t| t.label == label)
            .map(|t| t.webview.clone())
            .ok_or_else(|| AppError::Message(format!("标签 {label} 不存在")))?
    };
    let aspect = match shape {
        Some("port") => 0.5,
        Some("land") => 16.0 / 9.0,
        _ => state
            .play
            .lock()
            .unwrap()
            .get(label)
            .map(|p| p.aspect)
            .unwrap_or(0.0),
    };
    let mini = ensure_mini_window(app, aspect)?;
    // The frame is always re-applied: a page sent in with a shape the window is
    // not already using would otherwise stay letterboxed.
    let (w, h) = mini_shape(app, aspect);
    set_mini_size(app, w, h);
    // Order matters: park the webview hidden, size it, move it, then show —
    // showing first leaves a blank surface on WebView2.
    webview.hide()?;
    // Sized *before* the move, not after. WebView2 lets the new bounds land a
    // beat after `SetParent`, so a page placed only once was shown at its old
    // width for ~250ms and then snapped narrower — the "打开小窗时画面先大后小
    // 闪一下" report. It also means the site measures its viewport once instead
    // of twice, which is what stops 抖音 re-laying-out the item on screen.
    place(&webview, 0.0, 0.0, w, h);
    webview.reparent(&mini)?;
    *state.mini_tab.lock().unwrap() = Some(label.to_string());
    *state.mini_ctx.lock().unwrap() = 0;
    size_mini_content(app);
    let cinema = crate::settings::load(app).mini_cinema != Some(false);
    // The page keeps playing where it was, so the only thing to change is what
    // of it is visible. It may not have a player yet, so the offer is repeated.
    dress_for_mini(&webview, cinema);
    for delay in [700u64, 2000, 4500] {
        let app = app.clone();
        let label = label.to_string();
        schedule(delay, move || {
            let state = app.state::<AppState>();
            // Only while this page is still the one in the small window. Brought
            // back inside these 4.5s, a late dress would strip the feed again
            // in the main window and leave the page looking frozen.
            let still_mini = state
                .mini_tab
                .lock()
                .unwrap()
                .as_deref()
                .map(|m| m == label.as_str())
                .unwrap_or(false);
            if !still_mini {
                return;
            }
            let tabs = state.tabs.lock().unwrap();
            if let Some(tab) = tabs.iter().find(|t| t.label == label) {
                dress_for_mini(&tab.webview, cinema);
            }
        });
    }
    webview.show()?;
    mini.show()?;
    mini.set_focus()?;
    // The window being focused is not the same as the *page* being focused: the
    // small window holds two webviews and the keyboard lands on the rail, which
    // leaves the picture on screen but deaf — ← → no longer seek, ↑ ↓ no longer
    // move. Hand the keys to the page, which is what the window is for.
    let _ = webview.set_focus();
    // A window that has never been drawn counts its own invisible resize border
    // as client area: the 405 asked for above arrived as 434, and the page was
    // laid out to that for as long as it stayed in the small window. Once the
    // frame is on screen the same numbers mean what they say, so they are applied
    // again — and WebView2 needs a beat after `SetParent` for them to stick.
    {
        let app = app.clone();
        schedule(160, move || set_mini_size(&app, w, h));
    }
    relayout_soon(app);
    state.record(&format!("detached {label} into mini window"));
    emit_tabs(app);
    Ok(())
}

/// WebView2 needs a beat after SetParent before new bounds stick, so the layout
/// is applied twice more after a reparent.
fn relayout_soon(app: &AppHandle) {
    for delay in [250u64, 800] {
        let app = app.clone();
        schedule(delay, move || layout(&app));
    }
}

/// Runs `f` on a helper thread after a delay. Every window and webview call we
/// make just posts a message to the event loop, so this is thread-safe.
///
/// The guard only catches anything in a debug build: `profile.release.panic` is
/// `abort`, so a panic in a deferred task ends the app rather than being
/// reported here. That is deliberate — a layout task that panics has already
/// left the window geometry wrong, and a silent half-working shell is worse to
/// close than a crash the user can see and restart from.
fn schedule<F>(delay_ms: u64, f: F)
where
    F: FnOnce() + Send + 'static,
{
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
            eprintln!("[vb] deferred task at {delay_ms}ms panicked");
        }
    });
}

pub fn attach_back(app: &AppHandle) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let label = match state.mini_tab.lock().unwrap().take() {
        Some(l) => l,
        None => return Ok(()),
    };
    let webview = state
        .tabs
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.label == label)
        .map(|t| t.webview.clone());
    if let (Some(webview), Some(main)) = (webview, app.get_window(MAIN)) {
        undress_mini(&webview);
        webview.reparent(&main)?;
        state.record(&format!("reattached {label} to main window"));
    }
    if let Some(mini) = app.get_window(MINI) {
        mini.hide()?;
    }
    if let Some(main) = app.get_window(MAIN) {
        main.show()?;
        let _ = main.unminimize();
        main.set_focus()?;
    }
    // The page has a window to live in again, so a trip to the corner is over.
    *state.main_hidden.lock().unwrap() = false;
    activate(app, &label)?;
    relayout_soon(app);
    Ok(())
}

/// The frame the *picture* gets in the small window: the player's own shape
/// first, the shape the user last picked second, a neutral box when neither is
/// known. The rail is not counted here — `set_mini_size` adds it, so these
/// numbers stay "how big is the video" for every caller.
fn mini_shape(app: &AppHandle, aspect: f64) -> (f64, f64) {
    match aspect {
        a if a >= 1.05 => (720.0, 405.0),
        a if a > 0.0 => (400.0, 760.0),
        _ => match crate::settings::load(app).mini_ratio.as_deref() {
            Some("port") => (400.0, 760.0),
            Some("land") => (720.0, 405.0),
            _ => (460.0, 280.0),
        },
    }
}

fn ensure_mini_window(app: &AppHandle, aspect: f64) -> Result<Window<tauri::Wry>, AppError> {
    if let Some(existing) = app.get_window(MINI) {
        return Ok(existing);
    }
    let (vw, vh) = mini_shape(app, aspect);
    let (mw, mh) = (vw + MINI_RAIL_W, vh);
    let mini = Window::builder(app, MINI)
        .title("小屏")
        .inner_size(mw, mh)
        .min_inner_size(MINI_RAIL_W + 200.0, 170.0)
        .position(600.0, 90.0)
        .decorations(false)
        .always_on_top(true)
        .resizable(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;

    // Top-right corner of the desktop, so the rail's buttons never sit off edge.
    let (ax, ay, aw, _) = desktop(&mini);
    let _ = mini.set_position(LogicalPosition::new(
        (ax + aw - mw - 24.0).max(ax + 8.0),
        ay + 90.0,
    ));
    remember_mini_frame(app, &mini);

    mini.add_child(
        with_debug_port(
            WebviewBuilder::new(
                MINI_BAR,
                WebviewUrl::App("index.html?bar=1".parse().unwrap()),
            )
            .devtools(cfg!(debug_assertions)),
            "9223",
        ),
        LogicalPosition::new(vw, 0.0),
        LogicalSize::new(MINI_RAIL_W, 420.0),
    )?;

    let handle = app.clone();
    mini.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(_) => {
            // Before the webviews are laid out to whatever size arrived: a page
            // that asked for 全屏 hands this window the whole monitor, and
            // sizing the picture to that is what makes the rail and the video
            // disagree about where the window is.
            if let Some(mini) = handle.get_window(MINI) {
                clamp_mini_frame(&handle, &mini);
            }
            size_mini_content(&handle);
            // The frameless window is resizable from its edges, and a top-left
            // anchored grow pushes the control bar off the desktop.
            if let Some(mini) = handle.get_window(MINI) {
                keep_on_screen(&mini);
            }
        }
        tauri::WindowEvent::Moved(_) => {
            // A drag is the user choosing where the small window stands, and a
            // later 全屏 hijack should send it back here rather than to whatever
            // corner it happened to be born in.
            if let Some(mini) = handle.get_window(MINI) {
                remember_mini_frame(&handle, &mini);
            }
        }
        tauri::WindowEvent::CloseRequested { api, .. } => {
            // A frameless window still answers Alt+F4, and destroying it would
            // take the reparented page with it. Pull it back into the tabs
            // instead — deferred, because reparent waits on the event loop.
            api.prevent_close();
            let app = handle.clone();
            schedule(0, move || {
                let _ = attach_back(&app);
            });
        }
        _ => {}
    });
    app.state::<AppState>().record("mini window created");
    Ok(mini)
}

pub fn size_mini_content(app: &AppHandle) {
    if app.get_window(MINI).is_none() {
        return;
    }
    let (w, h) = logical_size(app, MINI);
    if let Some(rail) = app.get_webview(MINI_BAR) {
        place(&rail, (w - MINI_RAIL_W).max(0.0), 0.0, MINI_RAIL_W, h);
        log_place(app, &rail, "mini-rail", (w - MINI_RAIL_W).max(0.0), 0.0, MINI_RAIL_W, h);
    }
    let state = app.state::<AppState>();
    let webview = state
        .mini_tab
        .lock()
        .unwrap()
        .clone()
        .and_then(|l| {
            state
                .tabs
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.label == l)
                .map(|t| t.webview.clone())
        });
    if let Some(webview) = webview {
        // Everything to the left of the rail belongs to the page. The site's own
        // controls sit at its right edge, and they are never under anything of ours.
        let w = (w - MINI_RAIL_W).max(60.0);
        place(&webview, 0.0, 0.0, w, h);
        log_place(app, &webview, "mini-content", 0.0, 0.0, w, h);
    }
}

/// Records the frame a page was given. What the webview reports back is left
/// out on purpose: `Webview::bounds` measures from the window a page was born
/// in, so a page that has been moved into the small window looks offset by the
/// distance between the two windows even when it sits exactly where it was put.
fn log_place(app: &AppHandle, webview: &Webview, tag: &str, x: f64, y: f64, w: f64, h: f64) {
    app.state::<AppState>().record(&format!(
        "place[{tag}] {} in {} at ({x:.0},{y:.0}) {w:.0}x{h:.0}",
        webview.label(),
        webview.window().label()
    ));
}

/// Gives the small window its frame. `w`/`h` are the size of the *picture*; the
/// rail is added on the right here, so the numbers callers pass mean the same
/// thing they always meant — how big the video is.
pub fn set_mini_size(app: &AppHandle, w: f64, h: f64) {
    if let Some(mini) = app.get_window(MINI) {
        let _ = mini.set_size(LogicalSize::new((w + MINI_RAIL_W) as u32, h as u32));
        keep_on_screen(&mini);
    }
    size_mini_content(app);
    refocus_mini_page(app);
}

/// Collapses or restores the sidebar; the pages immediately take the freed width.
pub fn set_sidebar_width(app: &AppHandle, width: f64) {
    let state = app.state::<AppState>();
    *state.sidebar_width.lock().unwrap() = width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    layout(app);
    relayout_soon(app);
}

/// Hides every page so the shell can take the whole window for its own screens —
/// the settings page and the "no video yet" panel. Pages keep loading underneath.
pub fn set_pages_visible(app: &AppHandle, on: bool) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    *state.pages_visible.lock().unwrap() = on;
    let mini = state.mini_tab.lock().unwrap().clone();
    let (x, y, w, h) = content_rect(app);
    let tabs = state.tabs.lock().unwrap();
    // A hidden app that never comes back is worse than a page shown one frame
    // late, so bringing pages back does not depend on `active` still being set:
    // the first tab that is not parked in the small window stands in for it.
    let active = state
        .active
        .lock()
        .unwrap()
        .clone()
        .filter(|a| tabs.iter().any(|t| &t.label == a && Some(&t.label) != mini.as_ref()))
        .or_else(|| {
            tabs.iter()
                .find(|t| Some(&t.label) != mini.as_ref())
                .map(|t| t.label.clone())
        });
    for tab in tabs.iter() {
        if Some(&tab.label) == mini.as_ref() {
            continue;
        }
        if Some(&tab.label) == active.as_ref() {
            if on {
                place(&tab.webview, x, y, w, h);
                tab.webview.show()?;
            } else {
                tab.webview.hide()?;
            }
        } else if !on {
            tab.webview.hide()?;
        }
    }
    state.record(&format!("pages {}", if on { "shown" } else { "hidden" }));
    Ok(())
}

/// Steps the page in front through its own history, like a browser's toolbar.
pub fn page_nav(app: &AppHandle, dir: &str) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let label = state
        .active
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| AppError::Message("没有打开的页面".into()))?;
    let script = match dir {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "pause" => "document.querySelectorAll('video').forEach(v => v.pause())",
        "mute" => "document.querySelectorAll('video').forEach(v => { v.muted = true })",
        _ => "location.reload()",
    };
    eval_in(app, &label, script)
}

/// Silences one tab, whichever page is in front. The page keeps the state — the
/// playback poll reads `video.muted` back out of it — so the small window's
/// speaker and the sidebar's button can never disagree about what is muted.
pub fn set_tab_mute(app: &AppHandle, label: &str, on: bool) -> Result<(), AppError> {
    // Only the page sitting in the small window gets its keys back: a mute
    // clicked from the main window's tab row should not drag focus sideways.
    let in_mini = app
        .state::<AppState>()
        .mini_tab
        .lock()
        .unwrap()
        .as_deref()
        == Some(label);
    eval_in(
        app,
        label,
        &format!(
            "window.__vb && window.__vb.setMute({})",
            if on { "true" } else { "false" }
        ),
    )?;
    if in_mini {
        refocus_mini_page(app);
    }
    Ok(())
}

/// The rail is a webview of its own, so a click on one of its buttons takes the
/// small window's keyboard focus away from the page and the site's shortcuts stop
/// reaching it. `window.focus()` inside the page cannot cross back over from the
/// other webview, so every command the rail sends hands the keys back here.
pub fn refocus_mini_page(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(label) = state.mini_tab.lock().unwrap().clone() else {
        return;
    };
    let tabs = state.tabs.lock().unwrap();
    if let Some(tab) = tabs.iter().find(|t| t.label == label) {
        let _ = tab.webview.set_focus();
    }
}

/// One of the small window's rail buttons, aimed at the page parked in it.
/// The rail is our own webview and has no reach into the player's document, so
/// each button names one method of the bridge that is already installed there.
/// Nothing is decided here: the page answers with its own state, which the
/// playback poll reads back out.
pub fn mini_ctl(app: &AppHandle, what: &str) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let label = state
        .mini_tab
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| AppError::Message("小窗里现在没有页面".into()))?;
    let script = match what {
        "toggle" => "window.__vb && window.__vb.toggle()",
        "rate" => "window.__vb && window.__vb.cycleRate()",
        "up" => "window.__vb && window.__vb.flip(-1)",
        "down" => "window.__vb && window.__vb.flip(1)",
        "next" => "window.__vb && window.__vb.next()",
        "prev" => "window.__vb && window.__vb.prev()",
        "louder" => "window.__vb && window.__vb.nudgeVol(0.1)",
        "softer" => "window.__vb && window.__vb.nudgeVol(-0.1)",
        other => return Err(AppError::Message(format!("未知的小窗按钮 {other}"))),
    };
    eval_in(app, &label, script)?;
    // The click that brought the command here landed on the rail, so the page has
    // to be handed the keyboard back or the next arrow key goes nowhere.
    refocus_mini_page(app);
    Ok(())
}

/// Resizes the main window between a landscape frame and a tall one for
/// vertical feeds. The window keeps its corner and is pulled back on screen.
pub fn set_main_size(app: &AppHandle, w: f64, h: f64) {
    if let Some(win) = app.get_window(MAIN) {
        let (_, _, aw, ah) = desktop(&win);
        let _ = win.set_size(LogicalSize::new(w.min(aw - 16.0), h.min(ah - 16.0)));
        // growing from a tall frame can push the corner off the desktop
        keep_on_screen(&win);
    }
    layout(app);
    relayout_soon(app);
}

pub fn toggle_mini_topmost(app: &AppHandle, on: bool) {
    if let Some(mini) = app.get_window(MINI) {
        let _ = mini.set_always_on_top(on);
    }
    refocus_mini_page(app);
}

fn profile_dir(app: &AppHandle, name: &str) -> Result<std::path::PathBuf, AppError> {
    let base = app
        .path()
        .app_local_data_dir()?
        .join("webview-profiles")
        .join(name);
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

static ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn next_id() -> usize {
    ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

#[derive(serde::Serialize, Clone)]
pub struct TabInfo {
    pub label: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub mini: bool,
    pub live: bool,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub muted: bool,
    pub videos: usize,
    pub feed: bool,
    pub rate: f64,
}

pub fn tab_infos(app: &AppHandle) -> Vec<TabInfo> {
    let state = app.state::<AppState>();
    let active = state.active.lock().unwrap().clone();
    let mini = state.mini_tab.lock().unwrap().clone();
    let play = state.play.lock().unwrap().clone();
    state
        .tabs
        .lock()
        .unwrap()
        .iter()
        .map(|t| {
            let p = play.get(&t.label).copied().unwrap_or_default();
            TabInfo {
                label: t.label.clone(),
                url: t.url.clone(),
                title: t.title.clone(),
                active: active.as_deref() == Some(t.label.as_str()),
                mini: mini.as_deref() == Some(t.label.as_str()),
                live: p.live,
                playing: p.playing,
                position: p.position,
                duration: p.duration,
                muted: p.muted,
                videos: p.videos,
                feed: p.feed,
                rate: p.rate,
            }
        })
        .collect()
}

/// Asked of every site page every couple of seconds. The sidebar's playback
/// rails are drawn from these figures, so they are the page's own state rather
/// than anything the shell infers. Returns an object (never `undefined`) so a
/// half-loaded page still answers.
const PLAY_PROBE: &str = r#"(function () {
  try {
    // A page that came back to the main window still dressed for the small one
    // shows nothing but a pinned video, and the only way out was a reload;
    // undo it the moment the probe notices.
    var vb = window.__vb;
    if (vb && vb.cinemaOn && !vb.inMini) vb.uncinema();
    var vs = [].slice.call(document.querySelectorAll('video'));
    var best = null;
    for (var i = 0; i < vs.length; i++) {
      var r = vs[i].getBoundingClientRect();
      var s = Math.max(1, r.width * r.height) * (vs[i].paused || vs[i].ended ? 1 : 2);
      if (!best || s > best.s) best = { v: vs[i], s: s };
    }
    if (!best) return { n: vs.length, rs: document.readyState };
    var v = best.v;
    return {
      n: vs.length,
      p: !v.paused && !v.ended,
      t: Math.round(v.currentTime || 0),
      d: isFinite(v.duration) ? Math.round(v.duration) : 0,
      m: !!v.muted || v.volume === 0,
      a: v.videoWidth && v.videoHeight ? v.videoWidth / v.videoHeight : 0,
      /// The strip beside the small window offers 上一条/下一条 on a swipe feed and
      /// 下一集 on everything else, so the page has to say which one it is and
      /// what speed it is actually running at.
      f: vb && vb.isFeed ? (vb.isFeed() ? 1 : 0) : 0,
      r: v.playbackRate || 1,
      rs: document.readyState
    };
  } catch (e) { return { n: 0, rs: 'error' }; }
})()"#;

pub fn spawn_playback_poller(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut tick: u32 = 0;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            tick += 1;
            let state = app.state::<AppState>();
            if *state.boss_hidden.lock().unwrap() {
                continue;
            }
            // A right click in the small window means "bring the app back", and
            // the page only reports its count, so that one page is asked far more
            // often than the rest.
            if let Some(label) = state.mini_tab.lock().unwrap().clone() {
                let webview = state
                    .tabs
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t.label == label)
                    .map(|t| t.webview.clone());
                if let Some(webview) = webview {
                    let app = app.clone();
                    if let Err(e) = webview
                        .eval_with_callback("window.__vb ? window.__vb.ctx : -1", move |raw| {
                            let seen = raw.trim().parse::<i64>().unwrap_or(-1);
                            let stored = *app.state::<AppState>().mini_ctx.lock().unwrap();
                            if seen <= stored {
                                if seen >= 0 && seen != stored {
                                    *app.state::<AppState>().mini_ctx.lock().unwrap() = seen;
                                }
                                return;
                            }
                            *app.state::<AppState>().mini_ctx.lock().unwrap() = seen;
                            app.state::<AppState>()
                                .record(&format!("mini page right-clicked, showing {label}"));
                            let handle = app.clone();
                            schedule(0, move || bring_main_front(&handle));
                        })
                    {
                        state.record(&format!("mini probe failed: {e}"));
                    }
                }
            }
            // Every page is sampled about twice a second, but the one sitting in
            // the small window is sampled every cycle: its rail is the only
            // playback control the viewer can see while the app is out of the
            // way, and a pause that takes over a second to show up reads as a
            // dead button.
            let every = tick % 4 == 0;
            let mini = state.mini_tab.lock().unwrap().clone();
            let tabs = {
                let state = app.state::<AppState>();
                state
                    .tabs
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|t| every || Some(t.label.as_str()) == mini.as_deref())
                    .map(|t| (t.label.clone(), t.webview.clone()))
                    .collect::<Vec<_>>()
            };
            for (label, webview) in tabs {
                let handle = app.clone();
                let tagged = label.clone();
                if let Err(e) =
                    webview.eval_with_callback(PLAY_PROBE, move |raw| record_play(&handle, &label, &raw))
                {
                    app.state::<AppState>()
                        .record(&format!("probe {tagged} failed: {e}"));
                }
            }
        }
    });
}

/// Brings the main window forward without touching playback.
fn bring_main_front(app: &AppHandle) {
    if let Some(w) = app.get_window(MAIN) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    *app.state::<AppState>().main_hidden.lock().unwrap() = false;
    layout(app);
}

fn record_play(app: &AppHandle, label: &str, raw: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let number = |key: &str| value.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let play = Play {
        live: true,
        playing: value.get("p").and_then(serde_json::Value::as_bool).unwrap_or(false),
        position: number("t"),
        duration: number("d"),
        muted: value.get("m").and_then(serde_json::Value::as_bool).unwrap_or(false),
        videos: value.get("n").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize,
        aspect: number("a"),
        feed: value.get("f").and_then(serde_json::Value::as_f64).unwrap_or(0.0) > 0.5,
        rate: number("r"),
    };
    app.state::<AppState>()
        .play
        .lock()
        .unwrap()
        .insert(label.to_string(), play);
}


pub fn emit_tabs(app: &AppHandle) {
    let tabs = tab_infos(app);
    for target in [SHELL, MINI_BAR] {
        let _ = app.emit_to(target, "browser://tabs", tabs.clone());
    }
}

/// Installed into every site page.
///
/// It answers the three things the shell needs from a page it has no
/// capabilities on: silence/restore for the boss key, an isolate-the-player mode
/// for the mini window, and a click/right-click channel back out (`ctx` is read
/// by the poller, which is the only reply path that works on an external origin).
pub const PAUSE_BRIDGE: &str = r#"
(function () {
  var vb = window.__vb = {
    pausedByUs: false,
    volume: null,
    inMini: false,
    cinemaOn: false,
    feedOn: null,
    /// The site's own control bar survived the dress, so ours stays out of the way.
    ownUi: false,
    rec: [],
    ctx: 0,
    hudEl: null,
    hudTimer: 0,
    hudInt: 0,
    noteTimer: 0,
    rate: 1,
    rates: [1, 1.25, 1.5, 2, 0.75],
    boosted: false,
    holdTimer: 0,
    dressInt: 0,
    badgeEl: null,

    silence: function () {
      document.querySelectorAll('video').forEach(function (v) {
        if (vb.volume === null) vb.volume = v.volume;
        if (!v.paused) { vb.pausedByUs = true; v.pause(); }
        v.muted = true;
        v.volume = 0;
      });
      document.querySelectorAll('audio').forEach(function (a) { a.muted = true; });
    },
    restore: function () {
      document.querySelectorAll('video').forEach(function (v) {
        v.muted = false;
        if (vb.volume !== null) v.volume = vb.volume;
        if (vb.pausedByUs) { v.play().catch(function () {}); vb.pausedByUs = false; }
      });
      document.querySelectorAll('audio').forEach(function (a) { a.muted = false; });
    },
    pause: function () {
      document.querySelectorAll('video').forEach(function (v) {
        if (!v.paused) { vb.pausedByUs = true; v.pause(); }
      });
    },

    top: function () {
      var vs = document.querySelectorAll('video');
      var best = null, score = 0;
      var vw = innerWidth || 1, vh = innerHeight || 1;
      for (var i = 0; i < vs.length; i++) {
        var r = vs[i].getBoundingClientRect();
        /// A vertical feed keeps its neighbours mounted for preloading, and they
        /// are the same size as the one you are watching — only the box that is
        /// actually on screen counts as the player.
        var on = Math.max(0, Math.min(r.bottom, vh) - Math.max(r.top, 0)) *
                 Math.max(0, Math.min(r.right, vw) - Math.max(r.left, 0));
        var s = Math.max(1, r.width * r.height) * (vs[i].paused || vs[i].ended ? 1 : 2) *
                (on > vh * vw * 0.2 ? 4 : 1);
        if (s > score || !best) { best = vs[i]; score = s; }
      }
      return best;
    },

    fmt: function (sec) {
      sec = Math.max(0, Math.round(sec || 0));
      var m = Math.floor(sec / 60), s = sec % 60;
      return m + ':' + (s < 10 ? '0' + s : s);
    },

    /// Swipe feeds (抖音/快手) are one full-viewport picture per item and the
    /// page moves between them; long-video sites (B站/ACFun/YouTube) have a
    /// player embedded in a document that scrolls. The two need opposite handling
    /// in the small window, so the shape is decided before anything is restyled.
    isFeed: function () {
      if (vb.feedOn) return true;
      var h = location.hostname;
      /// Fixed players keep a page of text around them; never second-guess them.
      if (/(^|\.)(bilibili|acfun|youtube|youtu\.be|dailymotion|iqiyi|youku|tencent|mgtv)\./.test(h)) return false;
      if (/(^|\.)(douyin|iesdouyin|kuaishou|kwai|kwai-pro|gifshow|videovk|chenzhongtech)\./.test(h)) {
        vb.feedOn = true;
        return true;
      }
      var st = vb.stacked();
      /// A page with no player yet cannot be judged: say "not a feed" for now and
      /// decide properly once there is something on screen to measure.
      if (st === null) return false;
      vb.feedOn = st;
      return st;
    },
    /// The signature of a swipe feed, for a site nobody listed: the player is
    /// carried past its neighbours on a rail that the page slides with a
    /// transform, so one of its ancestors sits a whole screen off.
    stacked: function () {
      var v = vb.top();
      if (!v) return null;
      var vh = innerHeight || 1;
      for (var n = v; n && n.nodeType === 1 && n.tagName !== 'HTML'; n = n.parentElement) {
        var m = (getComputedStyle(n).transform || '').match(/matrix\(([^)]*)\)/);
        if (!m) continue;
        var ty = parseFloat(m[1].split(',')[5]);
        if (isFinite(ty) && Math.abs(ty) > vh * 0.4) return true;
      }
      return false;
    },
    /// Removes everything but the player and pins it to the small window's own
    /// viewport. Reversible: every property written is recorded first.
    ///
    /// Pinning beats stretching. Growing the ancestor chain to 100% leaves the
    /// frame at the mercy of whatever the site already did to those boxes —
    /// a `transform` or `contain` anywhere up the tree quietly moves where
    /// `position: fixed` lands, which is what looks like the video being
    /// mis-positioned in the small window. Clearing those properties and
    /// fastening the <video> itself to the viewport has no such dependency.
    ///
    /// That whole approach is wrong for a swipe feed, which is why the feed takes
    /// the lighter branch below.
    cinema: function () {
      if (vb.cinemaOn) return 'already';
      var v = vb.top();
      if (!v) return 'no video';
      if (vb.isFeed()) return vb.cinemaFeed(v);
      var rec = [];
      var set = function (el, prop, value) {
        rec.push([el, prop, el.style.getPropertyValue(prop), el.style.getPropertyPriority(prop)]);
        el.style.setProperty(prop, value, 'important');
      };
      var breaks = ['transform', 'perspective', 'filter', 'backdrop-filter',
                    'contain', 'will-change', 'container-type', 'clip-path'];
      var chain = [];
      for (var node = v; node && node.nodeType === 1 && node.tagName !== 'HTML'; node = node.parentElement) {
        chain.push(node);
      }
      /// Where the player stops and the page begins. A site's own progress bar,
      /// 倍速 and 弹幕开关 are hung inside the box the picture stands in, so the
      /// fold starts one level *above* that box and leaves them where they are.
      /// Without it the small window loses the bar the viewer was already using.
      var frame = vb.playerFrame(v);
      vb.ownUi = !!frame;
      var stop = frame ? chain.indexOf(frame) : 0;
      /// From the frame upward only. Inside it the site's own stacking order is
      /// what puts its control bar *over* its picture; lifting the chain of boxes
      /// down to the video — which is what this used to do — lifts the box the
      /// picture sits on above the bar instead, and the bar is there in the
      /// document and simply never paints.
      for (var c = stop; c < chain.length; c++) {
        var el = chain[c], parent = el.parentElement;
        for (var b = 0; b < breaks.length; b++) set(el, breaks[b], 'none');
        set(el, 'opacity', '1');
        set(el, 'z-index', '2147483000');
        if (c < stop || !parent) continue;
        for (var i = 0; i < parent.children.length; i++) {
          var sib = parent.children[i];
          if (sib === el || (sib.id && sib.id.indexOf('vb-') === 0)) continue;
          set(sib, 'display', 'none');
        }
      }
      set(document.documentElement, 'width', '100%');
      set(document.documentElement, 'height', '100%');
      set(document.body, 'width', '100%');
      set(document.body, 'height', '100%');
      set(document.body, 'margin', '0');
      set(document.body, 'overflow', 'hidden');
      set(document.body, 'background', '#0c1614');
      /// The box found above is what gets pinned to the small window's viewport,
      /// so the controls fastened inside it land on the edges the site built them
      /// for. With no such box the picture is pinned on its own, as before.
      var anchor = frame || v;
      set(anchor, 'position', 'fixed');
      set(anchor, 'inset', '0');
      set(anchor, 'width', '100vw');
      set(anchor, 'height', '100vh');
      set(anchor, 'max-width', '100vw');
      set(anchor, 'max-height', '100vh');
      // The bars a contained video leaves are the element's own background, and
      // the player boxes behind it are white — without this the small window
      // shows a bright band above the picture.
      set(anchor, 'background', '#0c1614');
      /// Everything between the picture and that box has to hand over its whole
      /// area, or the video stops at the size the page gave it and the control
      /// bar floats in the middle of a black frame.
      for (var m = 0; m < stop; m++) {
        set(chain[m], 'width', '100%');
        set(chain[m], 'height', '100%');
        set(chain[m], 'max-width', 'none');
        set(chain[m], 'max-height', 'none');
      }
      set(v, 'object-fit', 'contain');
      set(v, 'background', frame ? 'transparent' : '#0c1614');
      vb.keepUi(frame, v);
      vb.rec = rec;
      vb.cinemaOn = true;
      vb.hud();
      return 'on';
    },
    /// The player's own box: the lowest ancestor of the picture that still holds
    /// a control which is not the picture. B 站, ACFun and YouTube all hang their
    /// bar — 进度、倍速、清晰度、弹幕开关 — inside one container of about the
    /// video's own size, and that container is where the fold has to stop.
    ///
    /// Recognised by shape and contents, never by class name: the sites re-hash
    /// those, and a name found by hand folds their controls away again on the
    /// next rebuild. An ancestor meaningfully bigger than the picture is the
    /// page rather than the player, so the climb gives up there and the caller
    /// falls back to pinning the video alone.
    playerFrame: function (v) {
      var b = v.getBoundingClientRect();
      if (!b.width || !b.height) return null;
      for (var n = v.parentElement, up = 0;
           n && n.nodeType === 1 && n.tagName !== 'BODY' && n.tagName !== 'HTML' && up < 8;
           n = n.parentElement, up++) {
        var r = n.getBoundingClientRect();
        if (r.width > b.width * 1.25 || r.height > b.height * 1.4) return null;
        if (vb.hasUi(n)) return n;
      }
      return null;
    },
    /// A control the site still shows. Faded counts: a bar that hides itself
    /// until the pointer arrives is `opacity:0`, not gone, and folding it is the
    /// difference between a player and a picture.
    hasUi: function (box) {
      var v = vb.top();
      var els = box.querySelectorAll(
        'button,[role=button],[class*=ctrl],[class*=control],[class*=setting],[class*=progress],[class*=slider],[class*=volume]');
      for (var i = 0; i < els.length; i++) {
        var e = els[i];
        /// The chain down to the picture is the player's own skeleton, not a bar
        /// the viewer clicks.
        if (v && (e === v || (e.contains && e.contains(v)))) continue;
        var cs = getComputedStyle(e);
        if (cs.display === 'none' || cs.visibility === 'hidden') continue;
        var r = e.getBoundingClientRect();
        if (r.width < 6 || r.height < 6) continue;
        return true;
      }
      return false;
    },
    /// The fade a site puts over its own bar while the pointer is elsewhere.
    /// B 站 keeps 播放、进度、清晰度、音量 laid out and clickable the whole time
    /// and simply paints them at `opacity: 0`, so the small window looks like it
    /// threw the bar away when it is in fact sitting there transparent.
    ///
    /// Only a *fade* is woken. A box the site keeps at full opacity and
    /// `visibility: hidden` is not a sleeping bar, it is a panel the viewer
    /// closed: that is exactly how B 站 parks its 选集 drawer, and forcing those
    /// visible opened the drawer in the small window and left no way to shut it
    /// again — the site's own close button was underneath our `!important`.
    /// `display: none` boxes are already skipped: they have no size to mark.
    ///
    /// The mark is a class rather than an inline write because the bar has to be
    /// able to put itself away again: left up around the clock it sits on top of
    /// the subtitles.
    keepUi: function (frame, v) {
      if (!frame) return;
      var box = frame.getBoundingClientRect();
      var els = frame.querySelectorAll(
        'button,[role=button],[class*=ctrl],[class*=control],[class*=setting],[class*=progress],[class*=slider],[class*=volume]');
      var marked = [];
      for (var i = 0; i < els.length && i < 400; i++) {
        var e = els[i];
        if (e === v || (e.contains && e.contains(v))) continue;
        var r = e.getBoundingClientRect();
        if (r.width < 6 || r.height < 6) continue;
        if (r.bottom < box.top || r.top > box.bottom) continue;
        for (var n = e; n && n !== frame; n = n.parentElement) {
          var cs = getComputedStyle(n);
          if (parseFloat(cs.opacity) >= 0.99) continue;
          if (n.classList.contains('vb-ui')) break;
          n.classList.add('vb-ui');
          marked.push(n);
        }
      }
      vb.uiEls = marked;
      /// The bar a site never fades on its own. B 站 paints 播放、进度、清晰度 at
      /// full opacity and waits for its own hover logic to put them away, and a
      /// small window's pointer rarely visits the page the way it visits a tab —
      /// so the bar sits over the picture for the whole video. We fold it when
      /// the hand goes idle and bring it back on the next move. A feed's column
      /// of likes and comments is deliberately not in this set: that one stays.
      var bars = [];
      if (!vb.isFeed()) {
        var cand = frame.querySelectorAll('[class*=control],[class*=ctrl],[class*=progress]');
        for (var j = 0; j < cand.length && j < 200; j++) {
          var b = cand[j];
          if (b === v || (b.contains && b.contains(v))) continue;
          var br = b.getBoundingClientRect();
          if (br.width < 24 || br.height < 8) continue;
          if (br.top < box.bottom - Math.max(72, box.height * 0.26)) continue;
          var bc = getComputedStyle(b);
          if (bc.display === 'none' || bc.visibility === 'hidden' || parseFloat(bc.opacity) < 0.5) continue;
          if (b.classList.contains('vb-bar')) continue;
          b.classList.add('vb-bar');
          bars.push(b);
        }
      }
      vb.barEls = bars;
      if (!marked.length && !bars.length) return;
      if (!document.getElementById('vb-ui-css')) {
        var s = document.createElement('style');
        s.id = 'vb-ui-css';
        s.textContent = 'html.vb-ui .vb-ui{opacity:1!important}' +
          'html:not(.vb-ui) .vb-bar{opacity:0!important;pointer-events:none;transition:opacity .18s ease}';
        document.head.appendChild(s);
      }
      vb.uiWake();
    },
    /// The bar comes up while the pointer is being used and goes back a beat after
    /// the hand leaves — which is how it behaves in its own tab. 收回 clears the
    /// marks, so the page owns the fade again.
    uiWake: function () {
      var on = function () {
        var awake = (vb.uiEls && vb.uiEls.length) || (vb.barEls && vb.barEls.length);
        if (!awake) return;
        document.documentElement.classList.add('vb-ui');
        clearTimeout(vb.uiT);
        vb.uiT = setTimeout(function () {
          document.documentElement.classList.remove('vb-ui');
        }, 2600);
      };
      vb.uiOff = on;
      if (!vb.uiSeen) {
        vb.uiSeen = 1;
        ['pointermove', 'mousedown', 'wheel', 'touchstart'].forEach(function (t) {
          addEventListener(t, on, true);
        });
      }
      on();
    },
    uiSleep: function () {
      clearTimeout(vb.uiT);
      document.documentElement.classList.remove('vb-ui');
      if (vb.uiEls) {
        for (var i = 0; i < vb.uiEls.length; i++) vb.uiEls[i].classList.remove('vb-ui');
        vb.uiEls = null;
      }
      if (vb.barEls) {
        for (var k = 0; k < vb.barEls.length; k++) vb.barEls[k].classList.remove('vb-bar');
        vb.barEls = null;
      }
    },
    /// A feed already paints one full-screen picture per item, and it moves that
    /// picture with a transform its own framework keeps re-writing. So nothing
    /// here touches layout: the rail, the offsets and the <video> stay exactly as
    /// the site made them, and only the branches around them that hold no player
    /// at all — nav, search bar, the side of likes and comments — are folded away.
    /// Clearing the transforms instead pulled the picture off screen and hid its
    /// own neighbours, which is the black small window this replaced.
    cinemaFeed: function (v) {
      var style = document.getElementById('vb-css');
      if (!style) {
        style = document.createElement('style');
        style.id = 'vb-css';
        (document.head || document.documentElement).appendChild(style);
      }
      /// Folding the chrome away is a mark rather than a style write, so the pass
      /// below can run again and again: a feed re-builds its own controls every
      /// time an item comes on screen, and a one-time dress was gone by the first
      /// flip.
      ///
      /// The four rules after the fold hand back what the folded chrome still
      /// reserves. A feed sizes its item for a whole browser window, so the band its
      /// nav bar stands in, the gutter its like column stands in and the strip its
      /// own control bar stands in stay empty even once they are folded away, and
      /// that is the picture pinned to the top-left corner of the small window. An
      /// inline style cannot clear them — the page sets some of the same sizes
      /// !important — so they are cleared by mark too.
      style.textContent = '[data-vbh]{display:none!important}' +
        'html[data-vbfeed],html[data-vbfeed] body{min-width:0!important}' +
        '[data-vbtop]{padding-top:0!important}' +
        '[data-vbside]{padding-left:0!important;padding-right:0!important}' +
        '[data-vbwide]{width:100%!important;max-width:100%!important;flex-basis:100%!important}' +
        '[data-vbfill]{top:0!important;bottom:0!important}';
      var vh = innerHeight || 1, vw = innerWidth || 1;
      var box = function (e) { return e.getBoundingClientRect(); };
      /// A feed item is a panel the size of the screen, and the rail they sit on
      /// holds one of them per item. So a panel that is *elsewhere* — not where
      /// the playing one is — is the neighbour you are about to flip to, which is
      /// empty until the site slides it in front. Folding those away is how the
      /// small window went black the moment a flip happened. Panels stacked on
      /// top of the player are only overlays, and those are what this pass is for.
      var rail = function (e, mine) {
        var r = box(e);
        return r.width >= vw * 0.6 && r.height >= vh * 0.6 &&
               (Math.abs(r.top - mine.top) > vh * 0.25 || Math.abs(r.left - mine.left) > vw * 0.25);
      };
      var keep = [];
      /// Fold the page *around* the item and nothing inside it. The like column,
      /// the caption and the site's own progress bar are hung inside the panel and
      /// are the video's own furniture: the viewer asked for those to stay, and
      /// once the picture is given the full frame they overlay it the same way
      /// they do in a whole window.
      var from = vb.panel(v) || v;
      for (var node = from; node && node.nodeType === 1; node = node.parentElement) keep.push(node);
      for (var c = 0; c < keep.length; c++) {
        var el = keep[c], parent = el.parentElement;
        if (!parent) continue;
        var mine = box(el);
        /// Fold at every level above the panel. The neighbour panel is safe: it is
        /// skipped by name below, and folding only the levels outside the item is
        /// what keeps the item's own controls on screen.
        for (var i = 0; i < parent.children.length; i++) {
          var sib = parent.children[i];
          if (sib === el || keep.indexOf(sib) >= 0 || rail(sib, mine)) continue;
          if (sib.id && sib.id.indexOf('vb-') === 0) continue;
          /// A preloaded neighbour item still holds the video that is about to
          /// become this one; folding it away is what left nothing to watch.
          if (sib.querySelector && sib.querySelector('video')) continue;
          /// Folded even when its own box is collapsed: these sites park a floating
          /// control in a zero-height wrapper, where it overflows and still shows.
          sib.setAttribute('data-vbh', '1');
        }
      }
      vb.unfit();
      vb.fit(v);
      vb.hud();
      vb.cinemaOn = true;
      clearInterval(vb.dressInt);
      vb.dressInt = setInterval(function () {
        if (!vb.inMini || !vb.cinemaOn) { clearInterval(vb.dressInt); vb.dressInt = 0; return; }
        var now = vb.top();
        if (now) vb.cinemaFeed(now);
      }, 1500);
      /// A window that changes shape — into the small window, 竖屏 to 横屏, a drag
      /// of the edge — is precisely when the measured marks stop matching, and the
      /// site needs a moment to re-lay-out before they can be taken again. That
      /// beat is the "打开小窗时画面先大后小闪一下"; waiting for the next 1.5s pass
      /// stretches it into a second and a half of the wrong layout.
      if (!vb.resizeSeen) {
        vb.resizeSeen = 1;
        addEventListener('resize', function () {
          clearTimeout(vb.resizeT);
          vb.resizeT = setTimeout(function () {
            var now = vb.top();
            if (vb.inMini && vb.cinemaOn && now && vb.isFeed()) vb.cinemaFeed(now);
          }, 180);
        });
      }
      return 'feed';
    },
    /// The feed's own item box, or null when the page does not stack items and
    /// there is no line to draw. Recognised by shape: a screen-sized box with a
    /// twin of its own size parked somewhere else — the next item waiting on the
    /// rail. Class names are no use here; these sites re-hash them every deploy.
    panel: function (v) {
      var vw = innerWidth || 1, vh = innerHeight || 1;
      for (var n = v.parentElement; n && n.nodeType === 1 && n.tagName !== 'HTML'; n = n.parentElement) {
        var r = n.getBoundingClientRect();
        if (r.width < vw * 0.55 || r.height < vh * 0.55) continue;
        var p = n.parentElement;
        if (!p) continue;
        for (var i = 0; i < p.children.length; i++) {
          var c = p.children[i].getBoundingClientRect();
          if (Math.abs(c.width - r.width) > 8 || Math.abs(c.height - r.height) > 8) continue;
          if (Math.abs(c.top - r.top) > 8 || Math.abs(c.left - r.left) > 8) return n;
        }
      }
      return null;
    },
    /// Drops the measured marks so the pass below can measure the page it is
    /// looking at now. A `data-vbwide` taken while the item was laid out for a
    /// whole window keeps squeezing a picture that has since moved into the small
    /// one, and clearing only at 收回 means the wrong one stays on screen.
    unfit: function () {
      var on = document.querySelectorAll('[data-vbtop],[data-vbside],[data-vbwide],[data-vbfill]');
      for (var i = 0; i < on.length; i++) {
        on[i].removeAttribute('data-vbtop');
        on[i].removeAttribute('data-vbside');
        on[i].removeAttribute('data-vbwide');
        on[i].removeAttribute('data-vbfill');
      }
    },
    /// Name of each reserved band comes from what it does to the player's own box,
    /// never from a class name: these sites re-hash those with every deploy, and a
    /// name found by hand would leave the picture back in the corner on the next
    /// rebuild. Every mark sits on the chain the player is hung from, so the site's
    /// own slide offsets — the ones that go black when they are cleared — are left
    /// exactly as it made them.
    fit: function (v) {
      var box = function (e) { return e.getBoundingClientRect(); };
      var px = function (s) { var n = parseFloat(s); return n > 0 ? n : 0; };
      document.documentElement.setAttribute('data-vbfeed', '1');
      for (var node = v; node && node.nodeType === 1; node = node.parentElement) {
        var p = node.parentElement;
        if (!p || p.nodeType !== 1) break;
        var r = box(node), rp = box(p), cs = getComputedStyle(p);
        var pt = px(cs.paddingTop), side = px(cs.paddingLeft) + px(cs.paddingRight);
        /// The band above: the player starts exactly one parent padding down, and
        /// that padding is where the site's nav bar used to reach.
        if (pt > 8 && Math.abs(r.top - rp.top - pt) < 6) p.setAttribute('data-vbtop', '1');
        /// The gutters left and right: the player was squeezed by about the padding
        /// its own parent carries.
        if (side > 16 && rp.width - r.width >= side * 0.6) p.setAttribute('data-vbside', '1');
        if (rp.width - r.width > 16) node.setAttribute('data-vbwide', '1');
      }
      /// The strip below: the box the picture is fitted into stops short of the one
      /// it stands on by about a control bar's height.
      for (var s = v; s && s.parentElement; s = s.parentElement) {
        var ss = box(s), sp = box(s.parentElement);
        if (sp.height - ss.height > 16 && sp.width - ss.width < 8) {
          s.setAttribute('data-vbfill', '1');
          break;
        }
      }
    },
    undressFeed: function () {
      var on = document.querySelectorAll('[data-vbh],[data-vbtop],[data-vbside],[data-vbwide],[data-vbfill]');
      for (var i = 0; i < on.length; i++) {
        on[i].removeAttribute('data-vbh');
        on[i].removeAttribute('data-vbtop');
        on[i].removeAttribute('data-vbside');
        on[i].removeAttribute('data-vbwide');
        on[i].removeAttribute('data-vbfill');
      }
      document.documentElement.removeAttribute('data-vbfeed');
      clearInterval(vb.dressInt);
      vb.dressInt = 0;
    },
    uncinema: function () {
      vb.undressFeed();
      var rec = vb.rec || [];
      for (var i = rec.length - 1; i >= 0; i--) {
        var r = rec[i];
        if (r[2]) r[0].style.setProperty(r[1], r[2], r[3]); else r[0].style.removeProperty(r[1]);
      }
      vb.rec = [];
      vb.cinemaOn = false;
      vb.feedOn = null;
      vb.ownUi = false;
      vb.uiSleep();
      var el = vb.hudEl;
      if (el) el.remove();
      vb.hudEl = null;
      clearInterval(vb.hudInt);
      return 'off';
    },

    /// What the page itself adds inside the small window: a seek rail along the
    /// bottom, on the long-video pages whose own controls were folded away. That
    /// is all. Every button of ours lives in the strip to the right of the video,
    /// in a webview of our own, so nothing here can stand in front of the player.
    hud: function () {
      if (vb.hudEl) return vb.hudEl;
      var style = document.createElement('style');
      style.id = 'vb-hud-css';
      style.textContent =
        '#vb-hud{position:fixed;inset:0;z-index:2147483646;pointer-events:none;' +
        'font:12px/1.4 system-ui,sans-serif;color:#e8f5f0}' +
        /// A feed keeps its own progress bar, so ours shows only where the site
        /// lost one.
        '#vb-hud.vb-feed .vb-bottom{display:none}' +
        /// Same for a player whose bar the dress left in place: two progress bars
        /// stacked on one picture is worse than none.
        '#vb-hud.vb-own .vb-bottom{display:none}' +
        '#vb-hud .vb-bottom{position:absolute;left:0;right:0;bottom:0;padding:12px 14px 14px;' +
        'background:linear-gradient(transparent,rgba(6,16,14,.86));opacity:0;' +
        'transition:opacity .18s ease}' +
        '#vb-hud .vb-bottom.vb-show{opacity:1}' +
        /// Faded out is not the same as out of reach: without this the invisible
        /// rail still eats the click meant for the player underneath it.
        '#vb-hud .vb-bottom:not(.vb-show) *{pointer-events:none}' +
        '#vb-hud .vb-track{height:6px;border-radius:99px;background:rgba(232,245,240,.28);cursor:pointer}' +
        '#vb-hud .vb-fill{height:100%;width:0;border-radius:99px;background:#3ecf8e}' +
        '#vb-hud .vb-row{display:flex;align-items:center;gap:7px;margin-top:9px;flex-wrap:wrap}' +
        /// Never let a nowrap item shrink: it then overflows its box and paints
        /// over the control sitting next to it.
        '#vb-hud .vb-row>*{flex:none}' +
        '#vb-hud .vb-time{font-variant-numeric:tabular-nums;white-space:nowrap}' +
        '#vb-hud .vb-lab{opacity:.75;white-space:nowrap}' +
        '#vb-hud .vb-vol{pointer-events:auto;flex:0 1 92px;min-width:60px;height:6px;border-radius:99px;' +
        'background:rgba(232,245,240,.28);cursor:pointer}' +
        '#vb-hud .vb-vol i{display:block;height:100%;width:50%;border-radius:99px;background:#e8f5f0}' +
        '#vb-hud .vb-note{flex:1 1 100%;opacity:.85;white-space:nowrap;overflow:hidden;' +
        'text-overflow:ellipsis}' +
        '#vb-hud .vb-note:empty{display:none}' +
        '#vb-badge{position:fixed;top:14px;left:50%;transform:translateX(-50%);z-index:2147483646;' +
        'padding:5px 11px;border-radius:99px;background:rgba(6,16,14,.78);color:#e8f5f0;' +
        'font:12px/1.3 system-ui,sans-serif;pointer-events:none}';
      // One style tag per page, however many times the picture is dressed and
      // undressed: `uncinema` takes the bar away but has no reason to touch CSS.
      if (!document.getElementById('vb-hud-css')) {
        (document.head || document.documentElement).appendChild(style);
      }
      var el = document.createElement('div');
      el.id = 'vb-hud';
      el.innerHTML =
        '<div class="vb-bottom">' +
        '<div class="vb-track"><div class="vb-fill"></div></div>' +
        '<div class="vb-row">' +
        '<span class="vb-time">--:--</span>' +
        '<span class="vb-lab">音量</span><div class="vb-vol"><i></i></div>' +
        '<span class="vb-note"></span>' +
        '</div></div>';
      // One drag helper: the rail and the volume bar are both "point, and keep
      // pointing until the button comes up".
      var scrub = function (box, on) {
        var go = function (e) {
          var r = box.getBoundingClientRect();
          on(Math.min(1, Math.max(0, (e.clientX - r.left) / Math.max(1, r.width))));
        };
        box.addEventListener('pointerdown', function (e) {
          e.stopPropagation();
          go(e);
          var up = function () {
            removeEventListener('pointermove', go, true);
            removeEventListener('pointerup', up, true);
          };
          addEventListener('pointermove', go, true);
          addEventListener('pointerup', up, true);
        });
      };
      scrub(el.querySelector('.vb-track'), function (ratio) {
        var v = vb.top();
        if (!v || !isFinite(v.duration) || v.duration <= 0) return;
        try { v.currentTime = ratio * v.duration; } catch (err) {}
        vb.tick();
      });
      scrub(el.querySelector('.vb-vol'), function (ratio) {
        vb.setVol(ratio);
      });
      el.classList.toggle('vb-feed', vb.isFeed());
      el.classList.toggle('vb-own', !!vb.ownUi);
      document.documentElement.appendChild(el);
      vb.hudEl = el;
      return el;
    },
    setVol: function (ratio) {
      vb.volume = ratio;
      document.querySelectorAll('video').forEach(function (v) {
        v.volume = ratio;
        v.muted = ratio <= 0.001;
      });
      vb.tick();
    },
    setRate: function (r) {
      vb.rate = r;
      document.querySelectorAll('video').forEach(function (v) { v.playbackRate = r; });
      vb.note('倍速已切到 ' + r + '×，长按画面可临时再快一倍');
      vb.tick();
    },
    /// One switch with two ends. The small window's speaker and the sidebar's
    /// button both write `video.muted`, and both read it back off the page poll —
    /// so whichever end you used, including the site's own mute, the other one
    /// shows the same thing within a moment.
    setMute: function (on) {
      document.querySelectorAll('video').forEach(function (v) { v.muted = !!on; });
      document.querySelectorAll('audio').forEach(function (a) { a.muted = !!on; });
      vb.tick();
    },
    /// The strip's 倍速 button. The step is taken here rather than out there
    /// because the speed is the page's own business: a site that resets it under
    /// us should be reported as it now is, not as the number we last wrote.
    cycleRate: function () {
      var v = vb.top();
      var now = v ? Math.round((v.playbackRate || 1) * 100) / 100 : (vb.rate || 1);
      var i = vb.rates.indexOf(now);
      vb.setRate(vb.rates[(i + 1) % vb.rates.length]);
    },
    /// The strip's speaker: a wheel over it steps the volume. Muted counts as
    /// silence, so the first notch up is also the way back to being audible.
    nudgeVol: function (d) {
      var v = vb.top();
      if (!v) return vb.note('没找到播放器');
      vb.setVol(Math.min(1, Math.max(0, (v.muted ? 0 : v.volume) + d)));
    },
    toggle: function () {
      var v = vb.top();
      if (!v) return vb.note('没找到播放器');
      if (v.paused) { v.play().catch(function () {}); } else { v.pause(); }
      vb.tick();
    },
    /// Holding the picture runs 2× on top of whatever speed is set, and lets go
    /// back to it — the way a viewer skips through the part they already watched.
    boost: function (on) {
      var v = vb.top();
      if (!v) return;
      if (on) {
        if (vb.boosted) return;
        vb.boosted = true;
        v.playbackRate = (vb.rate || 1) * 2;
        vb.badge('2× 快进中，松开恢复');
      } else {
        if (!vb.boosted) return;
        vb.boosted = false;
        v.playbackRate = vb.rate || 1;
        vb.badge(null);
      }
    },
    badge: function (text) {
      var el = vb.badgeEl;
      if (!text) { if (el) el.remove(); vb.badgeEl = null; return; }
      if (!el) {
        el = document.createElement('div');
        el.id = 'vb-badge';
        document.documentElement.appendChild(el);
        vb.badgeEl = el;
      }
      el.textContent = text;
    },
    note: function (text) {
      var el = vb.hudEl;
      if (!el) return;
      var n = el.querySelector('.vb-note');
      if (n) n.textContent = text || '';
      /// A feed has no bottom bar of ours to write in — the site's own controls
      /// stand where it would — so the line floats at the top of the picture for a
      /// moment instead of going unsaid. Same for any player that kept its bar.
      if (!vb.isFeed() && !vb.ownUi) return;
      vb.badge(text || null);
      clearTimeout(vb.noteTimer);
      if (text) vb.noteTimer = setTimeout(function () { vb.badge(null); }, 2600);
    },
    /// The next episode is a control the site owns, so it is looked for rather
    /// than guessed at — and the click is only believed once the page has
    /// actually moved. A vertical feed (抖音/快手) has no such button at all:
    /// there the next item is one flick away, which is what the fallback does.
    next: function () { return vb.step(false); },
    /// One episode up or down. A feed answers the same gesture by scrolling; a
    /// list page has a 上一集 / 下一集 box that has to be found and clicked.
    prev: function () { return vb.step(true); },
    step: function (back) {
      if (vb.isFeed()) return vb.flip(back ? -1 : 1);
      var word = back ? '上一集' : '下一集';
      var hit = vb.stepBtn(back);
      if (!hit) return back ? vb.note('页面上没找到「上一集」按钮') : vb.flick();
      var before = vb.tag();
      hit.click();
      vb.note('正在打开' + word + '…');
      // Only a page that moved counts. A button that was clicked but did nothing
      // is reported as such rather than papered over with a scroll the viewer
      // never asked for.
      setTimeout(function () {
        vb.note(vb.tag() === before ? '点了页面上的「' + word + '」，但没换集' : '已跳到' + word);
      }, 2000);
    },
    /// What counts as "a different thing is playing now".
    tag: function () {
      var v = vb.top();
      return location.href + '|' + document.title + '|' + ((v && v.currentSrc) || '').slice(0, 90);
    },
    /// One item up or down a swipe feed. These pages answer the wheel gesture at
    /// whatever element is under the pointer, not at the window, so the event is
    /// aimed at the centre of the picture; if that moves nothing the arrow key is
    /// tried, which is the other thing a feed is written to answer.
    ///
    /// There is a third shape, and the rail has to answer it too: a feed's
    /// *landing* wall (抖音精选 before anything is clicked) is one real scroll box
    /// whose video has never started, so it paints at zero height and answers
    /// neither a flick nor an arrow key — its next item is one screen further
    /// down. Which lever to pull comes from the page, not from the host: a
    /// picture that fills the frame is a swiper, a frame with no picture is a
    /// wall. Measured, not assumed: on the wall a hand-written wheel only dragged
    /// the scroll box 100 px and then reported that nothing could page.
    flip: function (dir) {
      if (vb.pictureShare() < 0.4 && vb.slideScreen(dir)) {
        return vb.note(dir > 0 ? '已往下翻一屏' : '已往上翻一屏');
      }
      var before = vb.tag();
      var x = Math.round(innerWidth / 2), y = Math.round(innerHeight / 2);
      var el = document.elementFromPoint(x, y) || document.body;
      var d = { bubbles: true, cancelable: true, clientX: x, clientY: y };
      /// The sign is deliberately the opposite of a real wheel: a hand-written
      /// `wheel` event has no pointer-momentum behind it, and these swipers read
      /// the bare delta the other way round — measured on 抖音, a positive delta
      /// went back to the previous item.
      if (el) el.dispatchEvent(new WheelEvent('wheel', Object.assign({ deltaY: -dir * innerHeight * 0.8 }, d)));
      var moved = function () { return vb.tag() !== before; };
      setTimeout(function () {
        if (moved()) return vb.note(dir > 0 ? '已滑到下一条' : '已回到上一条');
        var k = dir > 0 ? 'ArrowDown' : 'ArrowUp';
        var ev = new KeyboardEvent('keydown', Object.assign({ key: k, code: k, keyCode: dir > 0 ? 40 : 38 }, d));
        /// Flagged so the handler below does not take this as a second press and
        /// skip two items on the way.
        ev.__vb = 1;
        document.dispatchEvent(ev);
        setTimeout(function () {
          if (moved()) return vb.note(dir > 0 ? '已滑到下一条' : '已回到上一条');
          if (vb.slideScreen(dir)) return vb.note(dir > 0 ? '已往下翻一屏' : '已往上翻一屏');
          vb.note('这一页翻不动，也没找到下一条');
        }, 1200);
      }, 700);
      vb.note('翻页中…');
    },
    /// How much of the small window the playing picture covers: 1 for a feed that
    /// owns the frame, 0 for a wall of cards that has not started playing.
    pictureShare: function () {
      var v = vb.top();
      if (!v) return 0;
      var r = v.getBoundingClientRect();
      var area = (innerWidth || 1) * (innerHeight || 1);
      return Math.min(1, Math.max(0, (r.width * r.height) / area));
    },
    /// Hand the page's own scroll box one screen over, and only claim it moved
    /// when the box says it did.
    slideScreen: function (dir) {
      var box = vb.scroller();
      if (!box) return false;
      var at = box.scrollTop;
      box.scrollTop = at + dir * box.clientHeight;
      return box.scrollTop !== at;
    },
    /// Two shapes of "the next one" exist on these sites: a list that scrolls,
    /// and an immersive feed that answers a flick. Both are tried, and the note
    /// says which one actually moved.
    flick: function () {
      if (vb.isFeed()) return vb.flip(1);
      var box = vb.scroller();
      var before = vb.tag();
      if (box) {
        var top = box.scrollTop;
        box.scrollTop = top + box.clientHeight;
        setTimeout(function () {
          vb.note(box.scrollTop === top ? '这一页翻不动，也没找到下一集' : '已往下翻一屏');
        }, 500);
        return;
      }
      var d = { bubbles: true, cancelable: true };
      window.dispatchEvent(new WheelEvent('wheel', Object.assign({ deltaY: 900 }, d)));
      document.dispatchEvent(new KeyboardEvent('keydown', Object.assign({ key: 'ArrowDown', keyCode: 40 }, d)));
      setTimeout(function () {
        vb.note(vb.tag() === before ? '没找到下一集，也没能滑到下一条' : '已滑到下一条');
      }, 1200);
    },
    scroller: function () {
      var best = null, span = 300;
      document.querySelectorAll('*').forEach(function (e) {
        var s = e.scrollHeight - e.clientHeight;
        if (e.clientHeight > 200 && s > span) { span = s; best = e; }
      });
      return best;
    },
    /// The box that steps one episode up or down, recognised by the marks sites
    /// leave on it, then by the two characters a viewer would read, then by a
    /// bare `next` / `prev` class token.
    stepBtn: function (back) {
      var want = back
        ? { sel: '.list-version__item.prev,.video-pagenavi__item.prev,.prev-play,' +
              '.bpx-player-ctrl-prev,.ytp-prev-button,' +
              '[class*="playprev"],[class*="btn-prev"],[aria-label*="上一集"],[title="上一集"]',
            words: ['上一集', '上一话', '上集', 'PREVIOUS', 'PREV', 'prev'],
            tokens: ['prev', 'previous', 'prev-btn', 'prevbtn', 'ctrl-prev', 'player-prev',
              'prev-video', 'prev-episode', 'video-prev'] }
        : { sel: '.list-version__item.next,.video-pagenavi__item.next,.next-play,' +
              '.bpx-player-ctrl-next,.ytp-next-button,' +
              '[class*="playnext"],[class*="btn-next"],[aria-label*="下一集"],[title="下一集"]',
            words: ['下一集', '下一话', '下集', 'NEXT', 'next'],
            tokens: ['next', 'next-btn', 'nextbtn', 'ctrl-next', 'player-next',
              'next-video', 'next-episode', 'video-next'] };
      /// Our own rail says 下一集 too, and must never be the answer.
      var ours = function (e) { return !!(e.closest && e.closest('#vb-hud,#vb-badge')); };
      /// Visibility is measured with client rectangles because these sites park
      /// their player controls in `position: fixed` boxes, whose offsetParent is
      /// null. A control the player has folded away still answers a click, so
      /// only the guessed-at passes below demand a box on screen.
      var shown = function (e) {
        if (ours(e)) return false;
        var r = e.getBoundingClientRect();
        return r.width >= 2 && r.height >= 2 && e.getClientRects().length > 0;
      };
      var marked = document.querySelectorAll(want.sel);
      for (var i = 0; i < marked.length; i++) {
        if (!ours(marked[i])) return marked[i];
      }
      var words = document.querySelectorAll('a,button,[role="button"],li,span');
      for (var j = 0; j < words.length; j++) {
        var e = words[j];
        if (!shown(e)) continue;
        var t = (e.textContent || '').replace(/\s+/g, '');
        if (want.words.indexOf(t) >= 0) return e;
      }
      /// Last pass before giving up: a player control named exactly "next".
      /// Sites style these as icon-only boxes, so there is no text to match on.
      var boxes = document.querySelectorAll('[class]');
      for (var k = 0; k < boxes.length; k++) {
        var el2 = boxes[k];
        /// Anchors are left out: a bare next class on a link is a pagination
        /// arrow far more often than the next episode.
        if (el2.tagName === 'A' || !shown(el2)) continue;
        var hit = el2.className.toString().toLowerCase().split(/[\s_-]+/);
        for (var m = 0; m < hit.length; m++) {
          if (want.tokens.indexOf(hit[m]) >= 0) return el2;
        }
      }
      return null;
    },
    show: function () {
      var el = vb.hud();
      var bottom = el.querySelector('.vb-bottom');
      bottom.classList.add('vb-show');
      vb.tick();
      clearTimeout(vb.hudTimer);
      vb.hudTimer = setTimeout(function () { bottom.classList.remove('vb-show'); }, 4200);
      clearInterval(vb.hudInt);
      vb.hudInt = setInterval(vb.tick, 300);
    },
    tick: function () {
      var el = vb.hudEl, v = vb.top();
      if (!el || !v) return;
      el.classList.toggle('vb-feed', vb.isFeed());
      var d = isFinite(v.duration) && v.duration > 0 ? v.duration : 0;
      var fill = el.querySelector('.vb-fill');
      if (fill) fill.style.width = (d ? Math.min(100, (v.currentTime / d) * 100) : 0) + '%';
      var time = el.querySelector('.vb-time');
      if (time) {
        time.textContent = vb.fmt(v.currentTime) + (d ? ' / ' + vb.fmt(d) : '') +
          (v.paused ? ' · 已暂停' : '');
      }
      var muted = !!v.muted || v.volume === 0;
      var vol = el.querySelector('.vb-vol');
      if (vol) vol.firstElementChild.style.width = Math.round((muted ? 0 : v.volume) * 100) + '%';
    },
  };

  document.addEventListener('click', function () {
    /// The release after a hold counts as a click; the rail staying up while the
    /// finger is still down is the point, so it must not also flash on letting go.
    if (vb.inMini && !vb.boosted) vb.show();
  }, true);
  // Long press anywhere but the rail itself: the sites put their own click
  // handlers over the picture, and a hold is not one of the things they answer.
  addEventListener('pointerdown', function (e) {
    if (!vb.inMini || e.button !== 0) return;
    var el = e.target;
    el = el && el.nodeType === 1 ? el : el && el.parentElement;
    if (el && el.closest && el.closest('#vb-hud')) return;
    clearTimeout(vb.holdTimer);
    vb.holdTimer = setTimeout(function () { vb.boost(true); }, 260);
  }, true);
  var letgo = function () {
    clearTimeout(vb.holdTimer);
    vb.boost(false);
  };
  addEventListener('pointerup', letgo, true);
  addEventListener('pointercancel', letgo, true);
  addEventListener('blur', letgo, true);
  // In the small window a feed has no scrollbar to grab and the mouse is often
  // nowhere near the page, so the arrow keys are the page-flip: down is the next
  // item, up is the one before. The site gets the press first — these feeds
  // answer the arrow key themselves — and our own flip is only sent when the page
  // demonstrably did not move, so one press never becomes two items.
  document.addEventListener('keydown', function (e) {
    if (!vb.inMini || e.__vb || e.repeat) return;
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    if (!vb.isFeed()) return;
    var dir = e.key === 'ArrowDown' ? 1 : -1;
    e.preventDefault();
    var before = vb.tag();
    setTimeout(function () {
      if (vb.tag() === before) vb.flip(dir);
    }, 550);
  }, true);
  // A feed card on these sites is an `<a target="_blank">`, and a player has no
  // second window to hand it to: the click means "play this here". The site's
  // own handler is cut out so it cannot also try to open a window, and the
  // address is given to this same tab. The host does the same thing when a page
  // asks for a new window, but this path does not wait for that request, which
  // is what made a left click look dead — the request is not always raised.
  document.addEventListener('click', function (e) {
    if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
    var el = e.target;
    el = el && el.nodeType === 1 ? el : el && el.parentElement;
    var a = el && el.closest ? el.closest('a[target="_blank"]') : null;
    if (!a) return;
    var href = a.href || '';
    if (href.indexOf('http') !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    window.location.href = href;
  }, true);
  // In the small window a right click means "take me back to the app", so the
  // page's own menu is dropped and a counter the poller reads is bumped instead.
  document.addEventListener('contextmenu', function (e) {
    if (!vb.inMini) return;
    vb.ctx++;
    e.preventDefault();
  }, true);
})()
"#;

fn each_tab(app: &AppHandle, script: &str) {
    let state = app.state::<AppState>();
    let tabs = state.tabs.lock().unwrap();
    for tab in tabs.iter() {
        let _ = tab.webview.eval(script);
    }
}

fn eval_in(app: &AppHandle, label: &str, script: &str) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let tabs = state.tabs.lock().unwrap();
    let tab = tabs
        .iter()
        .find(|t| t.label == label)
        .ok_or_else(|| AppError::Message(format!("标签 {label} 不存在")))?;
    tab.webview.eval(script)?;
    Ok(())
}

/// Hides every window and silences playback. Refuses when the global shortcut
/// never registered: a hidden app with no way to bring it back is the one state
/// the user cannot get out of.
pub fn hide_everything(app: &AppHandle) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let combo = state.boss_combo.lock().unwrap().clone();
    if !*state.boss_key.lock().unwrap() {
        return Err(AppError::Message(format!(
            "快捷键 {combo} 未注册成功（可能被其他程序占用），已取消隐藏"
        )));
    }
    each_tab(app, "window.__vb && window.__vb.silence()");
    for label in [MAIN, MINI] {
        if let Some(w) = app.get_window(label) {
            let _ = w.hide();
        }
    }
    *state.boss_hidden.lock().unwrap() = true;
    *state.main_hidden.lock().unwrap() = true;
    state.record(&format!("boss: hidden, {combo} restores"));
    Ok(())
}

/// Pauses and mutes every page without touching the windows, so the tray menu
/// can quiet the app down while the user still has something on screen.
pub fn silence_all(app: &AppHandle) {
    each_tab(app, "window.__vb && window.__vb.silence()");
    app.state::<AppState>().record("tray: all pages paused and muted");
}

/// Windows away, sound left alone. This is the background hide — listening to a
/// page with nothing on the desktop — and deliberately not the boss key, which
/// also pauses and mutes.
pub fn hide_to_tray(app: &AppHandle) {
    for label in [MAIN, MINI] {
        if let Some(w) = app.get_window(label) {
            let _ = w.hide();
        }
    }
    *app.state::<AppState>().main_hidden.lock().unwrap() = true;
    app.state::<AppState>().record("hidden to tray");
}

/// Fold the main window into the notification area and leave the small window
/// floating: this is the "only the video is on the desktop" shape the viewer asks
/// for once a page is in the mini window. It is the opposite of the ✕ and of the
/// boss key — nothing is paused, nothing is muted, and the tray's 显示魔芋播放器
/// brings it back.
///
/// Refused with a reason when there is no small window to keep watching: hiding
/// the only visible window on nothing but a tray icon leaves a running app with
/// no video and no window, which reads as a crash.
pub fn tuck_main(app: &AppHandle, on: bool) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    if on && state.mini_tab.lock().unwrap().is_none() {
        return Err(AppError::Message(
            "小窗里还没有页面：先把标签行的 ⧉ 按一下，把视频搬进小窗，主界面才收得掉".into(),
        ));
    }
    if let Some(w) = app.get_window(MAIN) {
        if on {
            w.hide()?;
        } else {
            raise(&w, true);
        }
    }
    *state.main_hidden.lock().unwrap() = on;
    state.record(if on {
        "main window tucked away, small window left on top"
    } else {
        "main window back from the corner"
    });
    if !on {
        layout(app);
        relayout_soon(app);
    }
    Ok(())
}

/// Whether the main window is currently tucked away, for the two buttons that
/// show the same state: the small window's rail and the main window's title row.
pub fn main_tucked(app: &AppHandle) -> bool {
    *app.state::<AppState>().main_hidden.lock().unwrap()
}

/// Brings one window back in front of the user.
///
/// Un-minimising has to come first: a window that was hidden while minimised
/// comes back *minimised* from `show` alone, which reads as a tray icon that
/// does nothing when clicked.
fn raise(w: &tauri::Window, focus: bool) {
    let _ = w.unminimize();
    let _ = w.show();
    if focus {
        let _ = w.set_focus();
    }
}

pub fn show_everything(app: &AppHandle) {
    if let Some(w) = app.get_window(MAIN) {
        raise(&w, true);
    }
    let has_mini = app.state::<AppState>().mini_tab.lock().unwrap().is_some();
    if has_mini {
        if let Some(w) = app.get_window(MINI) {
            raise(&w, false);
        }
    }
    layout(app);
    relayout_soon(app);
    each_tab(app, "window.__vb && window.__vb.restore()");
    let state = app.state::<AppState>();
    *state.boss_hidden.lock().unwrap() = false;
    *state.main_hidden.lock().unwrap() = false;
    state.record("windows restored");
}
