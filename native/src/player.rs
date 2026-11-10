//! Audio: fetching a track's audio file (via yt-dlp) and playing it (rodio).

use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Downloaded tracks kept on disk for instant replay.
const CACHE_FILES: usize = 40;

pub struct Audio {
    _device: MixerDeviceSink,
    player: Player,
}

impl Audio {
    pub fn new() -> Result<Self, String> {
        let mut device = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        device.log_on_drop(false);
        let player = Player::connect_new(device.mixer());
        Ok(Self { _device: device, player })
    }

    /// Starts playing a file and returns its duration in seconds (0 if unknown).
    pub fn load(&self, path: &Path) -> Result<f64, String> {
        use rodio::Source;
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        let source = Decoder::try_from(file).map_err(|e| format!("Cannot decode audio: {e}"))?;
        let duration = source.total_duration().map(|d| d.as_secs_f64()).unwrap_or(0.0);
        self.player.stop();
        self.player.append(source);
        self.player.play();
        Ok(duration)
    }

    pub fn stop(&self) {
        self.player.stop();
    }
    pub fn play(&self) {
        self.player.play();
    }
    pub fn pause(&self) {
        self.player.pause();
    }
    pub fn finished(&self) -> bool {
        self.player.empty()
    }
    pub fn position(&self) -> f64 {
        self.player.get_pos().as_secs_f64()
    }
    pub fn seek(&self, seconds: f64) {
        let _ = self.player.try_seek(Duration::from_secs_f64(seconds.max(0.0)));
    }
    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
    }
}

fn cache_dir() -> PathBuf {
    let dir = crate::auth::data_dir().join("cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn cached_path(id: &str) -> PathBuf {
    cache_dir().join(format!("{id}.m4a"))
}

/// Downloads the audio of a track into the cache (blocking; run on a worker thread).
pub fn fetch_audio(id: &str) -> Result<PathBuf, String> {
    // Video ids are [A-Za-z0-9_-]; refuse anything else before building a command line.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err("Invalid track id".into());
    }
    let done = cached_path(id);
    if done.exists() {
        return Ok(done);
    }
    let partial = cache_dir().join(format!("{id}.partial.m4a"));
    let _ = fs::remove_file(&partial);

    let (exe, path_env) = locate_ytdlp();
    let output = Command::new(exe)
        .env("PATH", path_env)
        .args(["-f", "140/bestaudio[ext=m4a]", "--no-playlist", "--no-part", "--no-progress"])
        .args(["--no-warnings", "--fixup", "never", "-o"])
        .arg(&partial)
        .arg(format!("https://music.youtube.com/watch?v={id}"))
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Cannot run yt-dlp ({e}). Is it installed?"))?;

    if !output.status.success() || !partial.exists() {
        let _ = fs::remove_file(&partial);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("unknown error");
        return Err(format!("Could not load audio: {}", reason.trim()));
    }
    fs::rename(&partial, &done).map_err(|e| e.to_string())?;
    trim_cache();
    Ok(done)
}

/// Finds yt-dlp and builds a PATH that also contains its helper tools (deno),
/// even when this process was started before winget updated the user's PATH.
fn locate_ytdlp() -> (PathBuf, std::ffi::OsString) {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut exe = PathBuf::from("yt-dlp.exe");
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let winget = PathBuf::from(local).join("Microsoft").join("WinGet");
        dirs.push(winget.join("Links"));
        if let Ok(entries) = fs::read_dir(winget.join("Packages")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("yt-dlp.yt-dlp_") {
                    let candidate = entry.path().join("yt-dlp.exe");
                    if candidate.exists() {
                        exe = candidate;
                    }
                    dirs.push(entry.path());
                } else if name.starts_with("DenoLand.Deno_") {
                    dirs.push(entry.path());
                }
            }
        }
    }
    (exe, std::env::join_paths(dirs).unwrap_or_default())
}

fn trim_cache() {
    let Ok(entries) = fs::read_dir(cache_dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(CACHE_FILES) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live end-to-end audio check (downloads one track and decodes it):
    /// `cargo test -p ytmusic-native -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_fetch_and_decode() {
        use rodio::Source;
        let started = std::time::Instant::now();
        let path = fetch_audio("kijpcUv-b8M").expect("fetch failed");
        println!("fetched {} bytes in {:.1}s", fs::metadata(&path).unwrap().len(), started.elapsed().as_secs_f64());
        let mut source = Decoder::try_from(fs::File::open(&path).unwrap()).expect("decode failed");
        println!("channels={:?} rate={:?} duration={:?}", source.channels(), source.sample_rate(), source.total_duration());
        let head: Vec<f32> = source.by_ref().take(2_000_000).collect();
        let peak = head.iter().fold(0f32, |m, s| m.max(s.abs()));
        let loud = head.iter().filter(|s| s.abs() > 0.001).count();
        println!("first {} samples: peak={peak:.3} non-silent={loud}", head.len());
        let seek = source.try_seek(Duration::from_secs(120));
        let after: Vec<f32> = source.by_ref().take(100_000).collect();
        let peak_after = after.iter().fold(0f32, |m, s| m.max(s.abs()));
        println!("seek to 2:00: {seek:?}, samples after={} peak={peak_after:.3}", after.len());
        assert!(loud > 1000 && peak_after > 0.001);
    }
}
