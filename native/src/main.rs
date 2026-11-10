// Fully native YouTube Music client: egui UI, rodio audio, no web engine
// (WebView2 is only started for the one-time `--login` window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod auth;
#[allow(dead_code)]
#[path = "../../src/lyrics.rs"]
mod lyrics;
mod player;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Duration,
};

use api::{thumb_sized, Api, Playlist, Track};
use eframe::egui::{
    self, pos2, vec2, Align, Color32, FontId, Label, Layout, Rect, RichText, Sense, UiBuilder,
};
use player::Audio;

const ACCENT: Color32 = Color32::from_rgb(255, 64, 96);
const DIM: Color32 = Color32::from_gray(150);
const ROW_HEIGHT: f32 = 52.0;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--login") {
        auth::run_login_window();
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("YouTube Music Native")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ytmusic-native",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

enum Msg {
    Tracks { title: String, result: Result<Vec<Track>, String> },
    Playlists(Result<Vec<Playlist>, String>),
    Audio { id: String, result: Result<PathBuf, String> },
    Lyrics { id: String, found: Option<lyrics::Found> },
    LoginClosed,
}

#[derive(PartialEq)]
enum State {
    Idle,
    Loading,
    Playing,
    Paused,
}

struct App {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    api: Arc<Api>,
    audio: Option<Audio>,

    query: String,
    list_title: String,
    list: Vec<Track>,
    busy: bool,
    status: Option<String>,
    playlists: Vec<Playlist>,
    selected_playlist: Option<String>,
    signing_in: bool,

    queue: Vec<Track>,
    current: Option<usize>,
    state: State,
    volume: f32,
    fetching: HashSet<String>,
    seek_drag: Option<f64>,

    show_lyrics: bool,
    lyrics: Option<lyrics::Found>,
    lyrics_for: String,
    lyrics_loading: bool,
    scroll: f32,
    browse_until: f64,
    /// Set by `--play <query>`: play the first search result automatically.
    autoplay: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        egui_extras::install_image_loaders(&ctx);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(12, 12, 12);
        visuals.window_fill = Color32::from_rgb(24, 24, 24);
        visuals.selection.bg_fill = Color32::from_rgb(120, 20, 40);
        visuals.slider_trailing_fill = true;
        ctx.set_visuals(visuals);
        ctx.all_styles_mut(|style| {
            style.interaction.selectable_labels = false;
            style.spacing.item_spacing = vec2(8.0, 6.0);
        });

        let (tx, rx) = channel();
        let (audio, status) = match Audio::new() {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(format!("No audio output device: {e}"))),
        };
        let mut app = Self {
            ctx,
            tx,
            rx,
            api: Arc::new(Api::new(auth::load_cookies())),
            audio,
            query: String::new(),
            list_title: "Search".into(),
            list: Vec::new(),
            busy: false,
            status,
            playlists: Vec::new(),
            selected_playlist: None,
            signing_in: false,
            queue: Vec::new(),
            current: None,
            state: State::Idle,
            volume: 0.8,
            fetching: HashSet::new(),
            seek_drag: None,
            show_lyrics: false,
            lyrics: None,
            lyrics_for: String::new(),
            lyrics_loading: false,
            scroll: 0.0,
            browse_until: 0.0,
            autoplay: false,
        };
        // `ytmusic-native.exe --play "song name" [--lyrics]`
        let args: Vec<String> = std::env::args().collect();
        if let Some(query) = args.iter().position(|a| a == "--play").and_then(|i| args.get(i + 1)) {
            app.query = query.clone();
            app.autoplay = true;
            app.show_lyrics = args.iter().any(|a| a == "--lyrics");
            app.search();
        } else if app.api.signed_in() {
            app.load_playlists();
            app.open_playlist("VLLM".into(), "Liked Music".into());
        }
        app
    }

    /// Runs `work` on a worker thread and delivers its message to the UI thread.
    fn spawn(&self, work: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    // ------------------------------------------------------------ data

    fn search(&mut self) {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.busy = true;
        self.status = None;
        self.selected_playlist = None;
        let api = self.api.clone();
        self.spawn(move || Msg::Tracks {
            title: format!("Results for \"{query}\""),
            result: api.search(&query),
        });
    }

    fn open_playlist(&mut self, id: String, title: String) {
        self.busy = true;
        self.status = None;
        self.show_lyrics = false;
        self.selected_playlist = Some(id.clone());
        let api = self.api.clone();
        self.spawn(move || Msg::Tracks { title, result: api.playlist_tracks(&id) });
    }

    fn load_playlists(&mut self) {
        let api = self.api.clone();
        self.spawn(move || Msg::Playlists(api.library_playlists()));
    }

    fn sign_in(&mut self) {
        self.signing_in = true;
        self.spawn(|| {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("--login").status();
            }
            Msg::LoginClosed
        });
    }

    fn sign_out(&mut self) {
        auth::clear();
        self.api = Arc::new(Api::new(None));
        self.playlists.clear();
        self.selected_playlist = None;
    }

    // -------------------------------------------------------- playback

    fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.queue.get(i))
    }

    fn position(&self) -> f64 {
        match (&self.audio, &self.state) {
            (Some(a), State::Playing | State::Paused) => a.position(),
            _ => 0.0,
        }
    }

    fn request_audio(&mut self, id: String) {
        if !self.fetching.insert(id.clone()) {
            return;
        }
        self.spawn(move || {
            let result = player::fetch_audio(&id);
            Msg::Audio { id, result }
        });
    }

    fn start(&mut self, index: usize) {
        let Some(track) = self.queue.get(index).cloned() else {
            self.state = State::Idle;
            return;
        };
        if let Some(a) = &self.audio {
            a.stop();
        }
        self.current = Some(index);
        self.state = State::Loading;
        self.status = None;
        self.seek_drag = None;
        self.scroll = 0.0;
        self.lyrics = None;
        self.lyrics_for = track.id.clone();
        self.lyrics_loading = true;
        self.request_audio(track.id.clone());

        self.spawn(move || {
            let query = lyrics::Query {
                title: track.title,
                artist: track.artist,
                album: track.album,
                duration: track.duration,
            };
            Msg::Lyrics { id: track.id, found: lyrics::fetch(&query) }
        });
    }

    fn step(&mut self, delta: isize) {
        let Some(current) = self.current else { return };
        let next = current as isize + delta;
        if next >= 0 && (next as usize) < self.queue.len() {
            self.start(next as usize);
        } else if delta > 0 {
            self.state = State::Idle;
        }
    }

    fn toggle_play(&mut self) {
        let Some(audio) = &self.audio else { return };
        match self.state {
            State::Playing => {
                audio.pause();
                self.state = State::Paused;
            }
            State::Paused => {
                audio.play();
                self.state = State::Playing;
            }
            State::Idle => {
                if let Some(i) = self.current {
                    self.start(i);
                }
            }
            State::Loading => {}
        }
    }

    fn on_audio(&mut self, id: String, result: Result<PathBuf, String>) {
        self.fetching.remove(&id);
        let is_current = self.current_track().is_some_and(|t| t.id == id);
        if !is_current || self.state != State::Loading {
            return; // a prefetch finished, or the user moved on
        }
        let loaded = result.and_then(|path| {
            let audio = self.audio.as_ref().ok_or("No audio output device")?;
            audio.set_volume(self.volume);
            audio.load(&path).inspect_err(|_| {
                let _ = std::fs::remove_file(&path);
            })
        });
        match loaded {
            Ok(()) => {
                self.state = State::Playing;
                // Prefetch the next track so the transition is instant.
                if let Some(next) = self.current.and_then(|i| self.queue.get(i + 1)) {
                    self.request_audio(next.id.clone());
                }
            }
            Err(e) => {
                self.state = State::Idle;
                self.status = Some(e);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Tracks { title, result } => {
                self.busy = false;
                match result {
                    Ok(tracks) => {
                        if tracks.is_empty() {
                            self.status = Some("Nothing found.".into());
                        }
                        self.ctx.forget_all_images();
                        self.list_title = title;
                        self.list = tracks;
                        if std::mem::take(&mut self.autoplay) && !self.list.is_empty() {
                            self.queue = self.list.clone();
                            self.start(0);
                        }
                    }
                    Err(e) => self.status = Some(e),
                }
            }
            Msg::Playlists(Ok(playlists)) => self.playlists = playlists,
            Msg::Playlists(Err(e)) => self.status = Some(e),
            Msg::Audio { id, result } => self.on_audio(id, result),
            Msg::Lyrics { id, found } => {
                if id == self.lyrics_for {
                    self.lyrics = found;
                    self.lyrics_loading = false;
                    self.scroll = 0.0;
                }
            }
            Msg::LoginClosed => {
                self.signing_in = false;
                self.api = Arc::new(Api::new(auth::load_cookies()));
                if self.api.signed_in() {
                    self.load_playlists();
                    self.open_playlist("VLLM".into(), "Liked Music".into());
                }
            }
        }
    }

    // -------------------------------------------------------------- UI

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(RichText::new("YouTube Music").size(20.0).strong());
        ui.label(RichText::new("native client").color(DIM).small());
        ui.add_space(10.0);

        if ui.selectable_label(!self.show_lyrics && self.selected_playlist.is_none(), "Search").clicked() {
            self.selected_playlist = None;
            self.show_lyrics = false;
            self.list_title = "Search".into();
            self.list.clear();
        }
        ui.separator();
        ui.label(RichText::new("LIBRARY").color(DIM).small());

        if !self.api.signed_in() {
            ui.label(RichText::new("Sign in to see your playlists and liked songs.").color(DIM));
            if self.signing_in {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Waiting for sign-in window…");
                });
            } else if ui.button("Sign in with Google").clicked() {
                self.sign_in();
            }
            return;
        }

        let mut open: Option<(String, String)> = None;
        let mut sign_out = false;
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.add_space(8.0);
            sign_out = ui.small_button("Sign out").clicked();
            ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for p in &self.playlists {
                        let selected = self.selected_playlist.as_deref() == Some(p.id.as_str());
                        let label = Label::new(RichText::new(&p.title).color(if selected {
                            Color32::WHITE
                        } else {
                            Color32::from_gray(200)
                        }))
                        .truncate()
                        .sense(Sense::click());
                        if ui.add(label).on_hover_text(&p.subtitle).clicked() {
                            open = Some((p.id.clone(), p.title.clone()));
                        }
                    }
                });
            });
        });
        if let Some((id, title)) = open {
            self.open_playlist(id, title);
        }
        if sign_out {
            self.sign_out();
        }
    }

    fn track_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 90.0).max(120.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search songs…")
                    .desired_width(width),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Search").clicked() || enter {
                self.search();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.list_title).size(22.0).strong());
            if self.busy {
                ui.spinner();
            }
        });
        if let Some(status) = &self.status {
            ui.colored_label(Color32::from_rgb(255, 170, 90), status);
        }
        ui.add_space(4.0);

        let playing_id = self.current_track().map(|t| t.id.clone());
        let mut clicked: Option<usize> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            ROW_HEIGHT,
            self.list.len(),
            |ui, range| {
                for i in range {
                    let track = &self.list[i];
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 6.0, Color32::from_gray(32));
                    }
                    let is_playing = playing_id.as_deref() == Some(track.id.as_str());
                    track_row(ui, rect, track, is_playing);
                    if response.clicked() {
                        clicked = Some(i);
                    }
                }
            },
        );
        if let Some(i) = clicked {
            self.queue = self.list.clone();
            self.start(i);
        }
    }

    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let track = self.current_track().cloned();
        let position = self.position();
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            match &track {
                Some(t) if !t.thumb.is_empty() => {
                    ui.add(
                        egui::Image::new(thumb_sized(&t.thumb, 120))
                            .fit_to_exact_size(vec2(56.0, 56.0))
                            .corner_radius(6),
                    );
                }
                _ => {
                    ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                }
            }
            ui.allocate_ui_with_layout(vec2(210.0, 56.0), Layout::top_down(Align::LEFT), |ui| {
                ui.add_space(8.0);
                let (title, artist) = track
                    .as_ref()
                    .map(|t| (t.title.as_str(), t.artist.as_str()))
                    .unwrap_or(("Nothing playing", ""));
                ui.add(Label::new(RichText::new(title).strong()).truncate());
                ui.add(Label::new(RichText::new(artist).color(DIM)).truncate());
            });

            if ui.button(RichText::new("⏮").size(18.0)).clicked() {
                // Restart the song unless we are at its very beginning.
                if position > 4.0 {
                    if let Some(a) = &self.audio {
                        a.seek(0.0);
                    }
                } else {
                    self.step(-1);
                }
            }
            if self.state == State::Loading {
                ui.add_sized(vec2(36.0, 30.0), egui::Spinner::new());
            } else {
                let icon = if self.state == State::Playing { "⏸" } else { "▶" };
                if ui.add_sized(vec2(36.0, 30.0), egui::Button::new(RichText::new(icon).size(18.0))).clicked() {
                    self.toggle_play();
                }
            }
            if ui.button(RichText::new("⏭").size(18.0)).clicked() {
                self.step(1);
            }

            let duration = track.as_ref().map(|t| t.duration).unwrap_or(0.0).max(1.0);
            let mut shown = self.seek_drag.unwrap_or(position).min(duration);
            ui.label(RichText::new(fmt_time(shown)).monospace().color(DIM));
            ui.spacing_mut().slider_width = (ui.available_width() - 290.0).max(80.0);
            let seek = ui.add(egui::Slider::new(&mut shown, 0.0..=duration).show_value(false));
            if seek.dragged() {
                self.seek_drag = Some(shown);
            } else if seek.drag_stopped() || seek.changed() {
                if let (Some(a), State::Playing | State::Paused) = (&self.audio, &self.state) {
                    a.seek(shown);
                }
                self.seek_drag = None;
            }
            ui.label(RichText::new(fmt_time(duration)).monospace().color(DIM));

            ui.spacing_mut().slider_width = 80.0;
            ui.label("🔊");
            if ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false)).changed() {
                if let Some(a) = &self.audio {
                    a.set_volume(self.volume);
                }
            }
            if ui.selectable_label(self.show_lyrics, "Lyrics").clicked() {
                self.show_lyrics = !self.show_lyrics;
            }
        });
    }

    fn lyrics_view(&mut self, ui: &mut egui::Ui) {
        let Some(track) = self.current_track().cloned() else {
            ui.centered_and_justified(|ui| ui.label(RichText::new("Play a song to see lyrics").size(20.0).color(DIM)));
            return;
        };
        let full = ui.available_rect_before_wrap();
        let wide = full.width() > 820.0;
        let art_width = if wide { full.width() * 0.36 } else { 0.0 };
        let lyric_rect = Rect::from_min_max(pos2(full.left() + art_width, full.top()), full.max);

        if wide {
            let art_rect = Rect::from_min_size(full.min, vec2(art_width, full.height()));
            ui.scope_builder(UiBuilder::new().max_rect(art_rect), |ui| {
                ui.vertical_centered(|ui| {
                    let size = (art_width - 70.0).min(full.height() * 0.55).max(80.0);
                    ui.add_space(((full.height() - size - 80.0) * 0.4).max(10.0));
                    if !track.thumb.is_empty() {
                        ui.add(
                            egui::Image::new(thumb_sized(&track.thumb, 544))
                                .fit_to_exact_size(vec2(size, size))
                                .corner_radius(14),
                        );
                    }
                    ui.add_space(14.0);
                    ui.add(Label::new(RichText::new(&track.title).size(20.0).strong()).truncate());
                    ui.add(Label::new(RichText::new(&track.artist).size(15.0).color(DIM)).truncate());
                });
            });
        }

        let Some(found) = self.lyrics.take() else {
            let text = if self.lyrics_loading { "Searching lyrics…" } else { "No lyrics found for this song" };
            ui.scope_builder(UiBuilder::new().max_rect(lyric_rect), |ui| {
                ui.centered_and_justified(|ui| ui.label(RichText::new(text).size(20.0).color(DIM)));
            });
            return;
        };
        let body = Rect::from_min_max(lyric_rect.min, pos2(lyric_rect.right(), lyric_rect.bottom() - 26.0));
        if found.synced {
            self.synced_lyrics(ui, body, &found);
        } else {
            ui.scope_builder(UiBuilder::new().max_rect(body.shrink2(vec2(24.0, 12.0))), |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for line in &found.lines {
                        ui.label(RichText::new(&line.text).size(22.0).color(Color32::from_gray(225)));
                    }
                });
            });
        }
        let note = if found.synced { "" } else { "  ·  not time-synced" };
        ui.painter().text(
            pos2(lyric_rect.left() + 24.0, lyric_rect.bottom() - 8.0),
            egui::Align2::LEFT_BOTTOM,
            format!("Lyrics: {}{note}", found.source),
            FontId::proportional(12.0),
            Color32::from_gray(110),
        );
        self.lyrics = Some(found);
    }

    /// Apple-Music-style synced lyrics: the active line is bright and glides to
    /// a fixed anchor; the rest fade with distance. Click a line to seek.
    fn synced_lyrics(&mut self, ui: &mut egui::Ui, rect: Rect, found: &lyrics::Found) {
        let ctx = ui.ctx().clone();
        let position = self.position() + 0.25;
        let active = found.lines.iter().rposition(|l| l.t <= position);
        let font = FontId::proportional(30.0);
        let wrap = (rect.width() - 56.0).max(100.0);
        let painter = ui.painter_at(rect);
        let id = ui.scope_id().with("lyrics");

        // Layout every line once per frame (egui caches unchanged galleys).
        let mut top = 0.0;
        let mut laid = Vec::with_capacity(found.lines.len());
        for (i, line) in found.lines.iter().enumerate() {
            let distance = active.map(|a| (i as i32 - a as i32).unsigned_abs()).unwrap_or(2);
            let target = if active == Some(i) { 1.0 } else { (0.42 - 0.05 * distance as f32).max(0.16) };
            // Quantised so the text-layout cache is not invalidated every frame.
            let alpha = (ctx.animate_value_with_time(id.with(i), target, 0.35) * 24.0).round() / 24.0;
            let text = if line.text.is_empty() { "•  •  •" } else { line.text.as_str() };
            let galley = painter.layout(text.to_owned(), font.clone(), Color32::WHITE.gamma_multiply(alpha), wrap);
            let height = galley.size().y;
            laid.push((top, galley));
            top += height + 20.0;
        }
        let total = top;

        // Smoothly glide towards the active line unless the user is scrolling.
        let now = ctx.input(|i| i.time);
        if ui.rect_contains_pointer(rect) {
            let wheel = ctx.input(|i| i.smooth_scroll_delta().y);
            if wheel != 0.0 {
                self.scroll = (self.scroll - wheel).clamp(0.0, (total - 80.0).max(0.0));
                self.browse_until = now + 3.0;
            }
        }
        if now >= self.browse_until {
            let anchor = laid.get(active.unwrap_or(0)).map(|l| l.0).unwrap_or(0.0);
            let target = (anchor - rect.height() * 0.3).max(0.0);
            let dt = ctx.input(|i| i.stable_dt).min(0.1);
            self.scroll += (target - self.scroll) * (1.0 - (-dt * 7.0).exp());
            if (target - self.scroll).abs() > 0.5 {
                ctx.request_repaint();
            } else {
                self.scroll = target;
            }
        } else {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        let mut seek_to = None;
        for (i, (line_top, galley)) in laid.into_iter().enumerate() {
            let pos = pos2(rect.left() + 28.0, rect.top() + line_top - self.scroll);
            let line_rect = Rect::from_min_size(pos, galley.size());
            if line_rect.bottom() < rect.top() || line_rect.top() > rect.bottom() {
                continue;
            }
            let response = ui.interact(line_rect.intersect(rect), id.with(("line", i)), Sense::click());
            if response.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                seek_to = Some(found.lines[i].t);
            }
            painter.galley(pos, galley, Color32::WHITE);
        }
        if let (Some(t), Some(a)) = (seek_to, &self.audio) {
            a.seek(t);
            self.browse_until = 0.0;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        if self.state == State::Playing {
            if self.audio.as_ref().is_some_and(|a| a.finished()) {
                self.step(1);
            }
            // Keep the clock moving (and auto-advance working while minimised)
            // without burning CPU: a few frames per second is enough.
            let interval = if self.show_lyrics { 100 } else { 500 };
            ctx.request_repaint_after(Duration::from_millis(interval));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("player").exact_size(76.0).show(ui, |ui| self.player_bar(ui));
        egui::Panel::left("sidebar").exact_size(210.0).show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            if self.show_lyrics {
                self.lyrics_view(ui);
            } else {
                self.track_list(ui);
            }
        });
    }
}

fn track_row(ui: &mut egui::Ui, rect: Rect, track: &Track, is_playing: bool) {
    let inner = rect.shrink2(vec2(8.0, 6.0));
    let thumb = Rect::from_min_size(inner.min, vec2(40.0, 40.0));
    if !track.thumb.is_empty() {
        egui::Image::new(thumb_sized(&track.thumb, 80)).corner_radius(4).paint_at(ui, thumb);
    }
    let show_album = inner.width() > 560.0;
    let duration_width = 54.0;
    let text_left = thumb.right() + 12.0;
    let text_right = inner.right() - duration_width;
    let album_left = if show_album { text_left + (text_right - text_left) * 0.6 } else { text_right };

    let main = Rect::from_min_max(pos2(text_left, inner.top()), pos2(album_left - 12.0, inner.bottom()));
    ui.scope_builder(UiBuilder::new().max_rect(main).layout(Layout::top_down(Align::LEFT)), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let title = RichText::new(&track.title).color(if is_playing { ACCENT } else { Color32::WHITE });
        ui.add(Label::new(title).truncate());
        ui.add(Label::new(RichText::new(&track.artist).color(DIM)).truncate());
    });
    if show_album {
        let album = Rect::from_min_max(pos2(album_left, inner.top()), pos2(text_right - 8.0, inner.bottom()));
        ui.scope_builder(UiBuilder::new().max_rect(album).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.add(Label::new(RichText::new(&track.album).color(DIM)).truncate());
        });
    }
    ui.painter().text(
        pos2(inner.right(), inner.center().y),
        egui::Align2::RIGHT_CENTER,
        fmt_time(track.duration),
        FontId::monospace(13.0),
        DIM,
    );
}

fn fmt_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
