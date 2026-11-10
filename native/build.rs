// Embeds the app icon into the .exe (shown in Explorer and the taskbar).
fn main() {
    println!("cargo:rerun-if-changed=../assets/icon.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../assets/icon.ico");
        res.compile().expect("failed to embed icon");
    }
}
