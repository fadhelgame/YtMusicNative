# YtMusicNative

Two lightweight YouTube Music desktop clients for Windows, written in Rust.

| | **YtMusic-Native** | **YtMusic-WebView** |
|---|---|---|
| What it is | Fully native client: its own UI and audio player, no browser engine | The official music.youtube.com page in a tiny WebView2 shell |
| UI | [egui](https://github.com/emilk/egui) | The real YouTube Music web UI |
| Audio | [rodio](https://github.com/RustAudio/rodio) + [yt-dlp](https://github.com/yt-dlp/yt-dlp) | The official web player |
| Feature coverage | Home feed, Explore, search, library playlists, liked songs, radio queue, synced lyrics | Everything the website does, plus animated lyrics and tray controls |
| Memory (measured) | ~100 MB, 1 process | ~600 MB, 9 processes |
| Stability | Depends on unofficial APIs; can break when YouTube changes | As stable as the website |

> **Unofficial project.** Not affiliated with, endorsed by, or sponsored by Google or YouTube.
> "YouTube Music" is a trademark of Google LLC. See [Disclaimer](#disclaimer).

## Download

Grab the latest `.exe` files from the [Releases page](https://github.com/fadhelgame/YtMusicNative/releases).
They are built from this repository by GitHub Actions. The binaries are not code-signed, so
Windows SmartScreen may warn on first launch.

## Requirements

- Windows 10 or 11 (x64)
- [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) — preinstalled on Windows 11
- **YtMusic-Native only:** `yt-dlp` and `deno` (yt-dlp needs a JavaScript runtime for YouTube):

  ```
  winget install yt-dlp.yt-dlp
  ```

  The winget package installs Deno as a dependency.

## YtMusic-Native

A from-scratch client that never loads the YouTube Music website while you listen.

- Home feed with your personal recommendations (Listen again, Quick picks, mixes) and an Explore page
- Search songs, browse your library playlists and Liked Music
- Playing a recommended song continues with its radio; the "Up next" panel shows the queue
- Shuffle, repeat, like, seek, volume; the next track is prefetched for instant transitions
- Time-synced lyrics that follow the song; click a line to jump to it
- Near-zero CPU while playing (event-driven UI, a few repaints per second)

### Sign-in

Click **Sign in with Google**. A one-time window opens Google's own sign-in page; your password is
typed into Google's page and never passes through this program. After sign-in, the session cookies
for `music.youtube.com` are stored encrypted with Windows DPAPI (bound to your Windows account) in
`%LOCALAPPDATA%\YtMusicNative\session.bin`, and the web engine is closed again. **Sign out** deletes them.

Cookies are only sent to `music.youtube.com`.

### Current limitations

- The first track takes about 10 seconds to start (stream extraction plus a delay YouTube requires).
- Audio is fetched anonymously at 128 kbps AAC; higher Premium bitrates are not used yet.
- Playlists load their first page only (about 100 tracks).
- Home and Explore show the first page of shelves only; artist pages are not supported yet.
- No tray icon or media-key integration yet.
- Downloaded audio is cached in `%LOCALAPPDATA%\YtMusicNative\cache` (last 40 tracks).

### Command line

```
YtMusic-Native.exe --play "song name" [--lyrics]
```

Searches, plays the first result, and optionally opens the lyrics view.

## YtMusic-WebView

The official web app in a ~2 MB Rust executable (wry + tao), using the WebView2 engine that ships
with Windows instead of bundling a browser.

- Closing the window keeps the music playing in the system tray
- Tray menu: Show/Hide, Lyrics, Play/Pause, Next, Previous, Quit
- Animated full-screen lyrics overlay: active line highlighted, others dimmed and blurred,
  cascading scroll, blurred album-art background; click a line to seek, Esc to close
- Single instance, remembers window size and position
- Asks WebView2 to reduce memory while hidden in the tray

Because it runs the same web page, memory use is similar to a browser tab.

## Lyrics sources

Both apps fetch lyrics from several sources in parallel and pick the best match by track duration:

1. [LRCLIB](https://lrclib.net) — time-synced
2. NetEase Cloud Music — time-synced
3. LRCLIB — plain text
4. [lyrics.ovh](https://lyrics.ovh) — plain text

Lyrics are line-synced, not word-synced. Lyrics belong to their respective rights holders; this
project only displays what these services return and does not store or redistribute them.

## Building from source

Install [Rust](https://rustup.rs) (MSVC toolchain) and the Visual Studio C++ Build Tools, then:

```
cargo build --release
```

Outputs:

- `target\release\ytmusic.exe` — WebView client
- `target\release\ytmusic-native.exe` — native client

Run the tests (the `--ignored` ones hit the network):

```
cargo test --workspace
cargo test --workspace -- --ignored --nocapture
```

## Project layout

```
src/            WebView client (main.rs), lyrics fetcher (lyrics.rs), page overlay (lyrics.js)
native/src/     Native client: app and pages (main.rs), theme and widgets (widgets.rs),
                InnerTube API (api.rs), audio (player.rs), sign-in (auth.rs)
```

## Disclaimer

This software is provided for personal and educational use.

- The native client talks to YouTube Music's internal, undocumented API and plays audio outside the
  official player. This may be against the YouTube Terms of Service. **Use it at your own risk**,
  including any risk to your Google account.
- The project does not remove ads from the official player, does not provide downloads for
  redistribution, and does not include any content from YouTube.
- A YouTube Music Premium subscription is recommended; please support the artists you listen to.

## Contributing

Issues and pull requests are welcome. YouTube changes its pages and APIs often, so reports of
broken selectors or API responses are especially useful.

## License

[MIT](LICENSE)
