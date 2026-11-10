//! Sign-in handling.
//!
//! The user signs in on Google's own page inside a one-time WebView2 window
//! (`ytmusic-native.exe --login`). Once signed in, the session cookies for
//! music.youtube.com are stored encrypted with Windows DPAPI (bound to the
//! current Windows user) and the web engine is closed again. The password
//! never passes through this program.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use windows::Win32::{
    Foundation::{LocalFree, HLOCAL},
    Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB},
};
use wry::{WebContext, WebViewBuilder};

const LOGIN_URL: &str =
    "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";

pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("YtMusicNative");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn cookie_file() -> PathBuf {
    data_dir().join("session.bin")
}

/// Returns the saved `Cookie:` header value, if the user is signed in.
pub fn load_cookies() -> Option<String> {
    let encrypted = fs::read(cookie_file()).ok()?;
    String::from_utf8(dpapi(&encrypted, false)?).ok()
}

fn save_cookies(header: &str) -> bool {
    match dpapi(header.as_bytes(), true) {
        Some(encrypted) => fs::write(cookie_file(), encrypted).is_ok(),
        None => false,
    }
}

/// Signs out: forgets the saved session and the login window's profile.
pub fn clear() {
    let _ = fs::remove_file(cookie_file());
    let _ = fs::remove_dir_all(data_dir().join("login-profile"));
}

fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if encrypt {
            CryptProtectData(&input, None, None, None, None, 0, &mut output).ok()?;
        } else {
            CryptUnprotectData(&input, None, None, None, None, 0, &mut output).ok()?;
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut _)));
        Some(bytes)
    }
}

/// Runs the sign-in window. Exits the process when signed in or closed.
pub fn run_login_window() {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music")
        .with_inner_size(LogicalSize::new(520.0, 720.0))
        .build(&event_loop)
        .expect("failed to create login window");

    let mut context = WebContext::new(Some(data_dir().join("login-profile")));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(LOGIN_URL)
        .build(&window)
        .expect("failed to create WebView2");

    let mut seen_signed_in = false;
    event_loop.run(move |event, _, control_flow| {
        let _keep_alive = (&window, &context);
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(1200));
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let on_music = webview
                    .url()
                    .map(|u| u.starts_with("https://music.youtube.com"))
                    .unwrap_or(false);
                if !on_music {
                    return;
                }
                let Ok(cookies) = webview.cookies_for_url("https://music.youtube.com/") else {
                    return;
                };
                let signed_in = cookies
                    .iter()
                    .any(|c| c.name() == "SAPISID" || c.name() == "__Secure-3PAPISID");
                if !signed_in {
                    return;
                }
                // Wait one extra tick so every cookie of the redirect chain has landed.
                if !seen_signed_in {
                    seen_signed_in = true;
                    return;
                }
                let header = cookies
                    .iter()
                    .map(|c| format!("{}={}", c.name(), c.value()))
                    .collect::<Vec<_>>()
                    .join("; ");
                save_cookies(&header);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
