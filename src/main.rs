// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod lyrics;

use std::{fs, path::PathBuf};

use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Icon as WindowIcon, Theme, Window, WindowBuilder},
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon as TrayIcon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
};
use windows::{
    core::{w, Interface},
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS},
        System::Threading::CreateMutexW,
        UI::WindowsAndMessaging::{
            FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        },
    },
};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

const APP_TITLE: &str = "YouTube Music";
const APP_URL: &str = "https://music.youtube.com/";

/// Chromium flags for the WebView2 process.
/// The first group mirrors wry's defaults (setting custom args replaces them).
const BROWSER_ARGS: &str = concat!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection",
    " --autoplay-policy=no-user-gesture-required",
    // Keep playback and queue logic running smoothly while hidden in the tray.
    " --disable-background-timer-throttling",
    " --disable-renderer-backgrounding",
    " --disable-backgrounding-occluded-windows",
);

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    /// JavaScript to run in the page (results coming back from worker threads).
    Script(String),
}

/// Handles a message posted by the injected page script (see lyrics.js).
fn on_ipc(body: &str, proxy: &tao::event_loop::EventLoopProxy<UserEvent>) {
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    if msg["type"] != "lyrics" {
        return;
    }
    let text = |k: &str| msg[k].as_str().unwrap_or_default().trim().to_owned();
    let key = msg["key"].as_str().unwrap_or_default().to_owned();
    let query = lyrics::Query {
        title: text("title"),
        artist: text("artist"),
        album: text("album"),
        duration: msg["duration"].as_f64().unwrap_or(0.0),
    };
    if query.title.is_empty() {
        return;
    }
    // Network I/O happens off the UI thread; the result is posted back as an event.
    let proxy = proxy.clone();
    std::thread::spawn(move || {
        let payload = lyrics::fetch_json(&key, &query);
        let js = format!("window.__ytmLyrics&&window.__ytmLyrics.receive({payload})");
        let _ = proxy.send_event(UserEvent::Script(js));
    });
}

fn main() {
    if !acquire_single_instance() {
        return;
    }

    let data_dir = data_dir();
    let _ = fs::create_dir_all(&data_dir);
    let state_file = data_dir.join("window.state");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    // Forward tray + menu events into the main event loop.
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Tray(e));
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let (icon_rgba, icon_size) = app_icon();

    let mut builder = WindowBuilder::new()
        .with_title(APP_TITLE)
        .with_theme(Some(Theme::Dark))
        .with_inner_size(PhysicalSize::new(1280u32, 800u32))
        .with_min_inner_size(PhysicalSize::new(480u32, 360u32))
        .with_window_icon(WindowIcon::from_rgba(icon_rgba.clone(), icon_size, icon_size).ok());
    let saved = WindowState::load(&state_file);
    if let Some(s) = &saved {
        builder = builder
            .with_inner_size(PhysicalSize::new(s.w, s.h))
            .with_position(PhysicalPosition::new(s.x, s.y))
            .with_maximized(s.maximized);
    }
    let window = builder.build(&event_loop).expect("failed to create window");

    // Persistent profile (login, cookies, cache) lives in %LOCALAPPDATA%\YtMusicRust.
    let mut web_context = WebContext::new(Some(data_dir.join("webview")));
    let ipc_proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(APP_URL)
        .with_initialization_script(include_str!("lyrics.js"))
        .with_ipc_handler(move |req| {
            // Only the YouTube Music page itself may talk to the host.
            if req.uri().host() == Some("music.youtube.com") {
                on_ipc(req.body(), &ipc_proxy);
            }
        })
        .with_background_color((3, 3, 3, 255)) // no white flash on startup
        .with_autoplay(true)
        .with_devtools(cfg!(debug_assertions))
        .with_theme(wry::Theme::Dark)
        .with_additional_browser_args(BROWSER_ARGS)
        .build(&window)
        .expect("failed to create WebView2 (is the WebView2 runtime installed?)");

    // Tray menu.
    let menu = Menu::new();
    let item_show = MenuItem::new("Show / Hide", true, None);
    let item_play = MenuItem::new("Play / Pause", true, None);
    let item_next = MenuItem::new("Next", true, None);
    let item_prev = MenuItem::new("Previous", true, None);
    let item_lyrics = MenuItem::new("Lyrics", true, None);
    let item_quit = MenuItem::new("Quit", true, None);
    let _ = menu.append_items(&[
        &item_show,
        &item_lyrics,
        &PredefinedMenuItem::separator(),
        &item_play,
        &item_next,
        &item_prev,
        &PredefinedMenuItem::separator(),
        &item_quit,
    ]);
    let tray = TrayIconBuilder::new()
        .with_tooltip(APP_TITLE)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(TrayIcon::from_rgba(icon_rgba, icon_size, icon_size).expect("icon"))
        .build()
        .expect("failed to create tray icon");

    event_loop.run(move |event, _, control_flow| {
        // Pure event-driven loop: zero CPU while idle.
        *control_flow = ControlFlow::Wait;
        let _keep_alive = (&tray, &web_context);

        match event {
            // Closing the window keeps the music playing in the tray.
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => hide(&window, &webview),

            Event::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => set_low_memory(&webview, false),

            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            })) => toggle(&window, &webview),

            Event::UserEvent(UserEvent::Script(js)) => {
                let _ = webview.evaluate_script(&js);
            }

            Event::UserEvent(UserEvent::Menu(e)) => {
                if e.id == *item_show.id() {
                    toggle(&window, &webview);
                } else if e.id == *item_play.id() {
                    page_call(&webview, "control('play')");
                } else if e.id == *item_next.id() {
                    page_call(&webview, "control('next')");
                } else if e.id == *item_prev.id() {
                    page_call(&webview, "control('prev')");
                } else if e.id == *item_lyrics.id() {
                    if !window.is_visible() || window.is_minimized() {
                        toggle(&window, &webview);
                    }
                    page_call(&webview, "toggle()");
                } else if e.id == *item_quit.id() {
                    WindowState::capture(&window).save(&state_file);
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}

fn hide(window: &Window, webview: &WebView) {
    window.set_visible(false);
    // Ask WebView2 to trim/swap its working set while we sit in the tray.
    set_low_memory(webview, true);
}

fn toggle(window: &Window, webview: &WebView) {
    if window.is_visible() && !window.is_minimized() {
        hide(window, webview);
    } else {
        set_low_memory(webview, false);
        window.set_visible(true);
        window.set_minimized(false);
        window.set_focus();
    }
}

/// Calls into the injected page script (lyrics.js), e.g. `control('next')`.
fn page_call(webview: &WebView, call: &str) {
    let js = format!("window.__ytmLyrics&&window.__ytmLyrics.{call}");
    let _ = webview.evaluate_script(&js);
}

/// WebView2 memory target: LOW while hidden, NORMAL while in use.
fn set_low_memory(webview: &WebView, low: bool) {
    let level = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    unsafe {
        if let Ok(core) = webview.controller().CoreWebView2() {
            if let Ok(core19) = core.cast::<ICoreWebView2_19>() {
                let _ = core19.SetMemoryUsageTargetLevel(level);
            }
        }
    }
}

/// Returns false if another instance is running (and brings it to the front).
fn acquire_single_instance() -> bool {
    unsafe {
        // The handle is intentionally leaked: it must live as long as the process.
        let _mutex = CreateMutexW(None, true, w!("YtMusicRust.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(hwnd) = FindWindowW(None, w!("YouTube Music")) {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return false;
        }
    }
    true
}

fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("YtMusicRust")
}

struct WindowState {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    maximized: bool,
}

impl WindowState {
    fn capture(window: &Window) -> Self {
        let pos = window.outer_position().unwrap_or_default();
        let size = window.inner_size();
        Self {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
            maximized: window.is_maximized(),
        }
    }

    fn save(&self, path: &PathBuf) {
        // Don't persist the bogus geometry of a minimized window.
        if self.w < 100 || self.h < 100 || self.x <= -30000 {
            return;
        }
        let _ = fs::write(
            path,
            format!("{},{},{},{},{}", self.x, self.y, self.w, self.h, self.maximized as u8),
        );
    }

    fn load(path: &PathBuf) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let mut p = text.trim().split(',');
        Some(Self {
            x: p.next()?.parse().ok()?,
            y: p.next()?.parse().ok()?,
            w: p.next()?.parse().ok()?,
            h: p.next()?.parse().ok()?,
            maximized: p.next()? == "1",
        })
    }
}

/// Procedurally drawn icon (red disc, white play triangle) so no image
/// decoder or asset file is needed.
fn app_icon() -> (Vec<u8>, u32) {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    let r = N as f32 / 2.0 - 1.0;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 - c, y as f32 - c);
            let dist = (fx * fx + fy * fy).sqrt();
            // 1px anti-aliased edge.
            let alpha = (r - dist + 0.5).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            // Play triangle pointing right, in unit coords relative to radius.
            let (ux, uy) = (fx / r, fy / r);
            let in_triangle = ux > -0.22 && ux < 0.42 && uy.abs() < (0.42 - ux) * 0.58;
            let (cr, cg, cb) = if in_triangle { (255, 255, 255) } else { (255, 0, 51) };
            let i = ((y * N + x) * 4) as usize;
            rgba[i] = cr;
            rgba[i + 1] = cg;
            rgba[i + 2] = cb;
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    (rgba, N)
}
