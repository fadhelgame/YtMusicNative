// Fully native YouTube Music client: egui UI, rodio audio, no web engine
// (WebView2 is only started for the one-time `--login` window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod auth;
#[allow(dead_code)]
#[path = "../../src/lyrics.rs"]
mod lyrics;
mod player;
mod widgets;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Duration,
};

use api::{Api, Card, Items, Playlist, Section, Target, Track};
use eframe::egui::{
    self, pos2, vec2, Align, Color32, FontId, Label, Layout, Margin, Rect, RichText, Sense,
};
use egui_material_icons::icons::*;
use player::Audio;
use widgets::*;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--login") {
        auth::run_login_window();
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("YouTube Music Native")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([760.0, 520.0])
            .with_icon(window_icon())
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "ytmusic-native",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

const LOGO_PNG: &[u8] = include_bytes!("../../assets/icon.png");
/// Font family used for lyrics (Plus Jakarta Sans, falling back to the UI fonts).
const LYRICS_FONT: &str = "lyrics";

fn window_icon() -> egui::IconData {
    let image = image::load_from_memory(LOGO_PNG).expect("embedded icon").to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData { rgba: image.into_raw(), width, height }
}

fn lyrics_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(LYRICS_FONT.into()))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "PlusJakartaSans".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../assets/PlusJakartaSans-SemiBold.ttf"))),
    );
    let mut family = vec!["PlusJakartaSans".to_owned()];
    family.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(egui::FontFamily::Name(LYRICS_FONT.into()), family);
    ctx.set_fonts(fonts);
}

enum Msg {
    Feed { explore: bool, result: Result<Vec<Section>, String> },
    Tracks { title: String, liked_list: bool, result: Result<Vec<Track>, String> },
    Radio { seed: String, result: Result<Vec<Track>, String> },
    Playlists(Result<Vec<Playlist>, String>),
    Audio { id: String, result: Result<PathBuf, String> },
    Lyrics { id: String, found: Option<lyrics::Found> },
    LikeFailed { id: String, was_liked: bool, error: String },
    LoginClosed,
}

#[derive(PartialEq)]
enum State {
    Idle,
    Loading,
    Playing,
    Paused,
}

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Home,
    Explore,
    Search,
    List,
}

#[derive(PartialEq, Clone, Copy)]
enum Repeat {
    Off,
    All,
    One,
}

/// Something the user clicked; applied after the frame's UI pass.
enum Action {
    /// Play this song and continue with its radio.
    Radio(Track),
    Open(Card),
    /// Play the current track list starting at this row.
    PlayList(usize),
    Jump(usize),
}

struct App {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    api: Arc<Api>,
    audio: Option<Audio>,

    page: Page,
    home: Option<Arc<Vec<Section>>>,
    explore: Option<Arc<Vec<Section>>>,
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
    shuffle: bool,
    repeat: Repeat,
    liked: HashSet<String>,
    fetching: HashSet<String>,
    /// Duration reported by the decoder, for tracks whose metadata had none.
    loaded_duration: f64,
    rng: u64,

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
        style_window(cc);
        install_fonts(&ctx);
        egui_material_icons::initialize(&ctx);
        apply_theme(&ctx);

        let (tx, rx) = channel();
        let (audio, status) = match Audio::new() {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(format!("No audio output device: {e}"))),
        };
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1;
        let mut app = Self {
            ctx,
            tx,
            rx,
            api: Arc::new(Api::new(auth::load_cookies())),
            audio,
            page: Page::Home,
            home: None,
            explore: None,
            query: String::new(),
            list_title: String::new(),
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
            shuffle: false,
            repeat: Repeat::Off,
            liked: HashSet::new(),
            fetching: HashSet::new(),
            loaded_duration: 0.0,
            rng: seed,
            show_lyrics: false,
            lyrics: None,
            lyrics_for: String::new(),
            lyrics_loading: false,
            scroll: 0.0,
            browse_until: 0.0,
            autoplay: false,
        };
        app.load_feed(false);
        if app.api.signed_in() {
            app.load_playlists();
        }
        // `ytmusic-native.exe --play "song name" [--lyrics]`
        let args: Vec<String> = std::env::args().collect();
        if let Some(query) = args.iter().position(|a| a == "--play").and_then(|i| args.get(i + 1)) {
            app.query = query.clone();
            app.autoplay = true;
            app.show_lyrics = args.iter().any(|a| a == "--lyrics");
            app.search();
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

    fn load_feed(&mut self, explore: bool) {
        let api = self.api.clone();
        self.spawn(move || Msg::Feed {
            explore,
            result: if explore { api.explore() } else { api.home() },
        });
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.show_lyrics = false;
        self.status = None;
        if page != Page::List {
            self.selected_playlist = None;
        }
        if page == Page::Explore && self.explore.is_none() {
            self.load_feed(true);
        }
        if page == Page::Home && self.home.is_none() {
            self.load_feed(false);
        }
    }

    fn search(&mut self) {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.go(Page::Search);
        self.busy = true;
        self.list.clear();
        self.list_title = format!("Results for \"{query}\"");
        let api = self.api.clone();
        self.spawn(move || Msg::Tracks {
            title: format!("Results for \"{query}\""),
            liked_list: false,
            result: api.search(&query),
        });
    }

    fn open_playlist(&mut self, id: String, title: String) {
        self.go(Page::List);
        self.busy = true;
        self.list.clear();
        self.list_title = title.clone();
        self.selected_playlist = Some(id.clone());
        let api = self.api.clone();
        let liked_list = id == "VLLM";
        self.spawn(move || Msg::Tracks { title, liked_list, result: api.playlist_tracks(&id) });
    }

    fn open_card(&mut self, card: Card) {
        match card.target {
            Target::Track(track) => self.play_radio(track),
            Target::Browse(id) => {
                self.go(Page::List);
                self.busy = true;
                self.list.clear();
                self.list_title = card.title.clone();
                let api = self.api.clone();
                // Album rows carry no cover or artist of their own; borrow the card's.
                let artist = card.subtitle.rsplit('\u{2022}').next().unwrap_or_default().trim().to_owned();
                self.spawn(move || Msg::Tracks {
                    title: card.title,
                    liked_list: false,
                    result: api.playlist_tracks(&id).map(|mut tracks| {
                        for t in &mut tracks {
                            if t.thumb.is_empty() {
                                t.thumb = card.thumb.clone();
                            }
                            if t.artist.is_empty() {
                                t.artist = artist.clone();
                            }
                        }
                        tracks
                    }),
                });
            }
            Target::Watch(id) => {
                self.go(Page::List);
                self.busy = true;
                self.list.clear();
                self.list_title = card.title.clone();
                let api = self.api.clone();
                self.spawn(move || Msg::Tracks {
                    title: card.title,
                    liked_list: false,
                    result: api.watch_queue(None, Some(&id)),
                });
            }
        }
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
        self.liked.clear();
        self.home = None;
        self.go(Page::Home);
    }

    fn toggle_like(&mut self) {
        let Some(id) = self.current_track().map(|t| t.id.clone()) else { return };
        if !self.api.signed_in() {
            self.status = Some("Sign in to like songs.".into());
            return;
        }
        let was_liked = self.liked.contains(&id);
        if was_liked {
            self.liked.remove(&id);
        } else {
            self.liked.insert(id.clone());
        }
        let api = self.api.clone();
        self.spawn(move || match api.set_like(&id, !was_liked) {
            Ok(()) => Msg::LikeFailed { id: String::new(), was_liked, error: String::new() },
            Err(error) => Msg::LikeFailed { id, was_liked, error },
        });
    }

    // -------------------------------------------------------- playback

    fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.queue.get(i))
    }

    fn duration(&self) -> f64 {
        match self.current_track() {
            Some(t) if t.duration > 0.0 => t.duration,
            Some(_) => self.loaded_duration,
            None => 0.0,
        }
    }

    fn position(&self) -> f64 {
        match (&self.audio, &self.state) {
            (Some(a), State::Playing | State::Paused) => a.position(),
            _ => 0.0,
        }
    }

    fn seek(&mut self, seconds: f64) {
        if let (Some(a), State::Playing | State::Paused) = (&self.audio, &self.state) {
            a.seek(seconds);
            self.browse_until = 0.0;
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

    fn request_lyrics(&mut self, track: Track, duration: f64) {
        self.spawn(move || {
            let query = lyrics::Query {
                title: track.title,
                artist: track.artist,
                album: track.album,
                duration,
            };
            Msg::Lyrics { id: track.id, found: lyrics::fetch(&query) }
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
        self.scroll = 0.0;
        self.loaded_duration = 0.0;
        self.lyrics = None;
        self.lyrics_for = track.id.clone();
        self.lyrics_loading = true;
        self.request_audio(track.id.clone());
        // Lyrics are matched by duration; if the metadata has none, wait for the decoder's.
        if track.duration > 0.0 {
            let duration = track.duration;
            self.request_lyrics(track, duration);
        }
    }

    /// Plays one song now and fills the queue with its radio in the background.
    fn play_radio(&mut self, track: Track) {
        let seed = track.id.clone();
        self.queue = vec![track];
        self.start(0);
        let api = self.api.clone();
        self.spawn(move || {
            let result = api.watch_queue(Some(&seed), None);
            Msg::Radio { seed, result }
        });
    }

    fn random_index(&mut self) -> usize {
        // xorshift64: plenty for picking the next song.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % self.queue.len().max(1) as u64) as usize
    }

    /// `auto` is true when a song ended by itself (repeat-one applies only then).
    fn step(&mut self, delta: isize, auto: bool) {
        let Some(current) = self.current else { return };
        let len = self.queue.len();
        if len == 0 {
            return;
        }
        if auto && self.repeat == Repeat::One {
            self.start(current);
            return;
        }
        if self.shuffle && delta > 0 && len > 1 {
            let mut next = self.random_index();
            if next == current {
                next = (next + 1) % len;
            }
            self.start(next);
            return;
        }
        let next = current as isize + delta;
        if next >= 0 && (next as usize) < len {
            self.start(next as usize);
        } else if delta > 0 && self.repeat == Repeat::All {
            self.start(0);
        } else if delta > 0 {
            self.state = State::Idle;
        }
    }

    fn previous(&mut self) {
        if self.position() > 4.0 {
            self.seek(0.0);
        } else {
            self.step(-1, false);
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

    fn prefetch_next(&mut self) {
        if self.shuffle {
            return;
        }
        if let Some(next) = self.current.and_then(|i| self.queue.get(i + 1)) {
            self.request_audio(next.id.clone());
        }
    }

    fn on_audio(&mut self, id: String, result: Result<PathBuf, String>) {
        self.fetching.remove(&id);
        let Some(track) = self.current_track().filter(|t| t.id == id).cloned() else {
            return; // a prefetch finished, or the user moved on
        };
        if self.state != State::Loading {
            return;
        }
        let loaded = result.and_then(|path| {
            let audio = self.audio.as_ref().ok_or("No audio output device")?;
            audio.set_volume(self.volume);
            audio.load(&path).inspect_err(|_| {
                let _ = std::fs::remove_file(&path);
            })
        });
        match loaded {
            Ok(duration) => {
                self.state = State::Playing;
                self.loaded_duration = duration;
                if track.duration <= 0.0 {
                    self.request_lyrics(track, duration);
                }
                self.prefetch_next();
            }
            Err(e) => {
                self.state = State::Idle;
                self.status = Some(e);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Feed { explore, result } => match result {
                Ok(sections) => {
                    let sections = Some(Arc::new(sections));
                    if explore {
                        self.explore = sections;
                    } else {
                        self.home = sections;
                    }
                }
                Err(e) => {
                    self.status = Some(e);
                    let empty = Some(Arc::new(Vec::new()));
                    if explore {
                        self.explore = empty;
                    } else {
                        self.home = empty;
                    }
                }
            },
            Msg::Tracks { title, liked_list, result } => {
                // Ignore results for a page the user already left.
                if title != self.list_title {
                    return;
                }
                self.busy = false;
                match result {
                    Ok(tracks) => {
                        if tracks.is_empty() {
                            self.status = Some("Nothing found.".into());
                        }
                        if liked_list {
                            self.liked.extend(tracks.iter().map(|t| t.id.clone()));
                        }
                        self.list = tracks;
                        if std::mem::take(&mut self.autoplay) && !self.list.is_empty() {
                            self.queue = self.list.clone();
                            self.start(0);
                        }
                    }
                    Err(e) => self.status = Some(e),
                }
            }
            Msg::Radio { seed, result } => {
                let still_seed = self.queue.len() == 1 && self.queue[0].id == seed;
                if let (true, Ok(mut tracks)) = (still_seed, result) {
                    tracks.retain(|t| t.id != seed);
                    self.queue.extend(tracks);
                    if self.state == State::Playing {
                        self.prefetch_next();
                    }
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
            Msg::LikeFailed { id, was_liked, error } => {
                if !id.is_empty() {
                    // Roll the optimistic change back.
                    if was_liked {
                        self.liked.insert(id);
                    } else {
                        self.liked.remove(&id);
                    }
                    self.status = Some(error);
                }
            }
            Msg::LoginClosed => {
                self.signing_in = false;
                self.api = Arc::new(Api::new(auth::load_cookies()));
                if self.api.signed_in() {
                    self.load_playlists();
                    self.home = None;
                    self.go(Page::Home);
                }
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Radio(track) => self.play_radio(track),
            Action::Open(card) => self.open_card(card),
            Action::PlayList(i) => {
                self.queue = self.list.clone();
                self.start(i);
            }
            Action::Jump(i) => self.start(i),
        }
    }

    // -------------------------------------------------------------- UI

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.add(
                egui::Image::from_bytes("bytes://logo.png", LOGO_PNG)
                    .fit_to_exact_size(vec2(28.0, 28.0))
                    .corner_radius(6),
            );
            ui.label(RichText::new("YT Music").size(18.0).strong().color(TEXT));
        });
        ui.add_space(14.0);

        let plain = !self.show_lyrics;
        if nav_item(ui, ICON_HOME.codepoint, "Home", plain && self.page == Page::Home) {
            self.go(Page::Home);
        }
        if nav_item(ui, ICON_SEARCH.codepoint, "Search", plain && self.page == Page::Search) {
            self.go(Page::Search);
        }
        if nav_item(ui, ICON_EXPLORE.codepoint, "Explore", plain && self.page == Page::Explore) {
            self.go(Page::Explore);
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new("Your library").size(12.0).color(MUTED));
        });
        ui.add_space(2.0);

        if !self.api.signed_in() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("Sign in to see your playlists and get personal picks.").color(SUB));
            });
            ui.add_space(6.0);
            if self.signing_in {
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.spinner();
                    ui.label(RichText::new("Waiting for sign-in…").color(SUB));
                });
            } else if nav_item(ui, ICON_LOGIN.codepoint, "Sign in", false) {
                self.sign_in();
            }
            return;
        }

        let mut open: Option<(String, String)> = None;
        let mut sign_out = false;
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            sign_out = nav_item(ui, ICON_LOGOUT.codepoint, "Sign out", false);
            ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for p in &self.playlists {
                        let selected = plain
                            && self.page == Page::List
                            && self.selected_playlist.as_deref() == Some(p.id.as_str());
                        let icon = if p.id == "VLLM" { ICON_FAVORITE } else { ICON_QUEUE_MUSIC };
                        if nav_item(ui, icon.codepoint, &p.title, selected) {
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

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(FIELD)
            .corner_radius(20)
            .inner_margin(Margin::symmetric(14, 7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ICON_SEARCH.codepoint).size(18.0).color(SUB));
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Search songs, artists, albums")
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY),
                    );
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.search();
                    }
                });
            });
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        if let Some(status) = &self.status {
            ui.add_space(6.0);
            ui.colored_label(Color32::from_rgb(255, 170, 90), status);
        }
    }

    fn feed_page(&mut self, ui: &mut egui::Ui, explore: bool) -> Option<Action> {
        let feed = if explore { self.explore.clone() } else { self.home.clone() };
        let Some(sections) = feed else {
            ui.centered_and_justified(|ui| ui.spinner());
            return None;
        };
        let mut action = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(if explore { "Explore" } else { "Home" }).size(26.0).strong().color(TEXT));
            self.status_line(ui);
            if sections.is_empty() {
                ui.add_space(20.0);
                ui.label(RichText::new("Nothing to show right now.").color(SUB));
                return;
            }
            let playing = self.current_track().map(|t| t.id.clone());

            // Featured: the first item of the first shelf.
            if !explore {
                ui.add_space(10.0);
                match sections.first().map(|s| &s.items) {
                    Some(Items::Tracks(tracks)) => {
                        let track = &tracks[0];
                        if hero(ui, &track.title, &track.artist, &track.thumb) {
                            action = Some(Action::Radio(track.clone()));
                        }
                    }
                    Some(Items::Cards(cards)) => {
                        let card = &cards[0];
                        if hero(ui, &card.title, &card.subtitle, &card.thumb) {
                            action = Some(Action::Open(card.clone()));
                        }
                    }
                    None => {}
                }
            }

            for (index, section) in sections.iter().enumerate() {
                ui.add_space(20.0);
                ui.label(RichText::new(&section.title).size(18.0).strong().color(TEXT));
                ui.add_space(6.0);
                match &section.items {
                    Items::Cards(cards) => {
                        egui::ScrollArea::horizontal()
                            .id_salt(("shelf", explore, index))
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 14.0;
                                    for card in cards {
                                        if card_tile(ui, card) {
                                            action = Some(Action::Open(card.clone()));
                                        }
                                    }
                                });
                                ui.add_space(12.0);
                            });
                    }
                    Items::Tracks(tracks) => {
                        let width = ui.available_width();
                        let columns = ((width / 300.0).floor() as usize).clamp(1, 3);
                        let column_width = (width - (columns as f32 - 1.0) * 10.0) / columns as f32;
                        let shown = tracks.len().min(columns * 4);
                        for chunk in tracks[..shown].chunks(columns) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                for track in chunk {
                                    let opts = RowOpts {
                                        active: playing.as_deref() == Some(track.id.as_str()),
                                        ..Default::default()
                                    };
                                    if song_row(ui, vec2(column_width, 56.0), track, opts) {
                                        action = Some(Action::Radio(track.clone()));
                                    }
                                }
                            });
                        }
                    }
                }
            }
            ui.add_space(20.0);
        });
        action
    }

    fn list_page(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        ui.add_space(4.0);
        let title = if self.list_title.is_empty() { "Search" } else { self.list_title.as_str() };
        ui.horizontal(|ui| {
            ui.add(Label::new(RichText::new(title).size(26.0).strong().color(TEXT)).truncate());
            if self.busy {
                ui.spinner();
            }
        });
        self.status_line(ui);
        if self.list.is_empty() && !self.busy && self.status.is_none() {
            ui.add_space(12.0);
            ui.label(RichText::new("Type in the search box above and press Enter.").color(SUB));
        }
        ui.add_space(8.0);

        let playing = self.current_track().map(|t| t.id.clone());
        let mut action = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, 58.0, self.list.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for i in range {
                let track = &self.list[i];
                let opts = RowOpts {
                    active: playing.as_deref() == Some(track.id.as_str()),
                    album: true,
                    duration: true,
                };
                if song_row(ui, vec2(ui.available_width(), 56.0), track, opts) {
                    action = Some(Action::PlayList(i));
                }
            }
        });
        action
    }

    fn queue_panel(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Up next").size(18.0).strong().color(TEXT));
        });
        ui.add_space(6.0);
        if self.queue.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("Pick a song and the queue shows up here.").color(SUB));
            });
            return None;
        }
        let mut action = None;
        let current = self.current;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, 54.0, self.queue.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for i in range {
                let opts = RowOpts { active: current == Some(i), duration: true, ..Default::default() };
                if song_row(ui, vec2(ui.available_width(), 52.0), &self.queue[i], opts) {
                    action = Some(Action::Jump(i));
                }
            }
        });
        action
    }

    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let bar = ui.max_rect();
        let track = self.current_track().cloned();
        let position = self.position();
        let duration = self.duration();

        // Thin progress line across the top; click or drag to seek.
        let hit = Rect::from_min_max(bar.min, pos2(bar.right(), bar.top() + 12.0));
        let seek = ui.interact(hit, ui.scope_id().with("seek"), Sense::click_and_drag());
        let thickness = if seek.hovered() || seek.dragged() { 5.0 } else { 3.0 };
        let line = Rect::from_min_size(bar.min, vec2(bar.width(), thickness));
        ui.painter().rect_filled(line, 0.0, Color32::from_rgb(38, 38, 44));
        if duration > 0.0 {
            let mut fraction = (position / duration).clamp(0.0, 1.0) as f32;
            if let Some(pointer) = seek.interact_pointer_pos().filter(|_| seek.dragged() || seek.clicked()) {
                fraction = ((pointer.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
                if seek.drag_stopped() || seek.clicked() {
                    self.seek(fraction as f64 * duration);
                }
            }
            ui.painter().rect_filled(
                Rect::from_min_size(bar.min, vec2(bar.width() * fraction, thickness)),
                0.0,
                ACCENT,
            );
        }
        if seek.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let content = Rect::from_min_max(pos2(bar.left() + 14.0, bar.top() + 8.0), pos2(bar.right() - 14.0, bar.bottom()));
        let side_width = ((content.width() - 250.0) / 2.0).max(120.0);

        // Left: cover, title, artist.
        let left = Rect::from_min_size(content.min, vec2(side_width, content.height()));
        let art = Rect::from_center_size(pos2(left.left() + 24.0, left.center().y), vec2(48.0, 48.0));
        cover(ui, art, track.as_ref().map(|t| t.thumb.as_str()).unwrap_or_default(), 120, 8);
        let text = Rect::from_min_max(pos2(art.right() + 12.0, left.center().y - 19.0), pos2(left.right() - 8.0, left.bottom()));
        sub_ui(ui, text, Layout::top_down(Align::LEFT), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let (title, artist) = track
                .as_ref()
                .map(|t| (t.title.as_str(), t.artist.as_str()))
                .unwrap_or(("Nothing playing", ""));
            ui.add(Label::new(RichText::new(title).size(14.0).strong().color(TEXT)).truncate());
            ui.add(Label::new(RichText::new(artist).size(12.5).color(SUB)).truncate());
        });

        // Centre: transport controls.
        let centre = Rect::from_center_size(content.center(), vec2(236.0, content.height()));
        sub_ui(ui, centre, Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let tint = |on: bool| if on { ACCENT } else { ICON };
            if icon_button(ui, ICON_SHUFFLE.codepoint, 20.0, tint(self.shuffle)).on_hover_text("Shuffle").clicked() {
                self.shuffle = !self.shuffle;
            }
            if icon_button(ui, ICON_SKIP_PREVIOUS.codepoint, 26.0, TEXT).clicked() {
                self.previous();
            }
            if self.state == State::Loading {
                ui.add_sized(vec2(42.0, 42.0), egui::Spinner::new().color(TEXT));
            } else {
                let icon = if self.state == State::Playing { ICON_PAUSE } else { ICON_PLAY_ARROW };
                if play_button(ui, icon.codepoint, 42.0).clicked() {
                    self.toggle_play();
                }
            }
            if icon_button(ui, ICON_SKIP_NEXT.codepoint, 26.0, TEXT).clicked() {
                self.step(1, false);
            }
            let (icon, on, hint) = match self.repeat {
                Repeat::Off => (ICON_REPEAT, false, "Repeat: off"),
                Repeat::All => (ICON_REPEAT, true, "Repeat: all"),
                Repeat::One => (ICON_REPEAT_ONE, true, "Repeat: one"),
            };
            if icon_button(ui, icon.codepoint, 20.0, tint(on)).on_hover_text(hint).clicked() {
                self.repeat = match self.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
            }
        });

        // Right: time, lyrics, like, volume (laid out right-to-left).
        let right = Rect::from_min_max(pos2(content.right() - side_width, content.top()), content.max);
        sub_ui(ui, right, Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().slider_width = 84.0;
            if ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false)).changed() {
                if let Some(a) = &self.audio {
                    a.set_volume(self.volume);
                }
            }
            ui.label(RichText::new(ICON_VOLUME_UP.codepoint).size(20.0).color(ICON));
            let liked = track.as_ref().is_some_and(|t| self.liked.contains(&t.id));
            let heart = if liked { ACCENT } else { ICON };
            if icon_button(ui, ICON_FAVORITE.codepoint, 20.0, heart).on_hover_text("Like").clicked() {
                self.toggle_like();
            }
            let lyrics_tint = if self.show_lyrics { ACCENT } else { ICON };
            if icon_button(ui, ICON_LYRICS.codepoint, 20.0, lyrics_tint).on_hover_text("Lyrics").clicked() {
                self.show_lyrics = !self.show_lyrics;
            }
            if right.width() > 330.0 {
                ui.add_space(6.0);
                let time = format!("{} / {}", fmt_time(position), fmt_time(duration));
                ui.label(RichText::new(time).size(12.5).color(SUB));
            }
        });
    }

    fn lyrics_view(&mut self, ui: &mut egui::Ui) {
        let Some(track) = self.current_track().cloned() else {
            ui.centered_and_justified(|ui| ui.label(RichText::new("Play a song to see lyrics").size(20.0).color(SUB)));
            return;
        };
        let full = ui.available_rect_before_wrap();
        let wide = full.width() > 760.0;
        let art_width = if wide { full.width() * 0.38 } else { 0.0 };
        let lyric_rect = Rect::from_min_max(pos2(full.left() + art_width, full.top()), full.max);

        if wide {
            let size = (art_width - 60.0).min(full.height() * 0.55).max(80.0);
            let art = Rect::from_center_size(
                pos2(full.left() + art_width / 2.0 - 10.0, full.top() + full.height() * 0.42),
                vec2(size, size),
            );
            cover(ui, art, &track.thumb, 544, 16);
            let text = Rect::from_min_size(pos2(art.left(), art.bottom() + 16.0), vec2(size, 60.0));
            sub_ui(ui, text, Layout::top_down(Align::Center), |ui| {
                ui.add(Label::new(RichText::new(&track.title).size(20.0).strong().color(TEXT)).truncate());
                ui.add(Label::new(RichText::new(&track.artist).size(15.0).color(SUB)).truncate());
            });
        }

        let Some(found) = self.lyrics.take() else {
            let text = if self.lyrics_loading { "Searching lyrics…" } else { "No lyrics found for this song" };
            sub_ui(ui, lyric_rect, Layout::top_down(Align::LEFT), |ui| {
                ui.centered_and_justified(|ui| ui.label(RichText::new(text).size(20.0).color(SUB)));
            });
            return;
        };
        // Keep the lyrics clear of the window top and of the source note / player bar.
        let body = Rect::from_min_max(
            pos2(lyric_rect.left(), lyric_rect.top() + 14.0),
            pos2(lyric_rect.right(), lyric_rect.bottom() - 40.0),
        );
        if found.synced {
            self.synced_lyrics(ui, body, &found);
        } else {
            sub_ui(ui, body.shrink2(vec2(28.0, 0.0)), Layout::top_down(Align::LEFT), |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.add_space(LYRICS_PAD);
                    for line in &found.lines {
                        let text = RichText::new(&line.text).font(lyrics_font(22.0)).color(Color32::from_gray(225));
                        ui.label(text);
                    }
                    ui.add_space(LYRICS_PAD);
                });
            });
        }
        edge_fade(ui, body);
        let note = if found.synced { "" } else { "  ·  not time-synced" };
        ui.painter().text(
            pos2(lyric_rect.left() + 28.0, lyric_rect.bottom() - 8.0),
            egui::Align2::LEFT_BOTTOM,
            format!("Lyrics: {}{note}", found.source),
            FontId::proportional(12.0),
            MUTED,
        );
        self.lyrics = Some(found);
    }

    /// Apple-Music-style synced lyrics: the active line is bright and glides to
    /// a fixed anchor; the rest fade with distance. Click a line to seek.
    fn synced_lyrics(&mut self, ui: &mut egui::Ui, rect: Rect, found: &lyrics::Found) {
        let ctx = ui.ctx().clone();
        let position = self.position() + 0.25;
        let active = found.lines.iter().rposition(|l| l.t <= position);
        let font = lyrics_font(30.0);
        let wrap = (rect.width() - 56.0).max(100.0);
        let painter = ui.painter_at(rect);
        let id = ui.scope_id().with("lyrics");

        // Layout every line once per frame (egui caches unchanged galleys).
        let mut top = LYRICS_PAD;
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
        let total = top + LYRICS_PAD;

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
        if let Some(t) = seek_to {
            self.seek(t);
        }
    }
}

/// Empty space above the first and below the last lyric line.
const LYRICS_PAD: f32 = 40.0;

/// Fades the top and bottom edges of `rect` into the background, so lyric
/// lines dissolve instead of being cut off mid-glyph.
fn edge_fade(ui: &egui::Ui, rect: Rect) {
    const HEIGHT: f32 = 44.0;
    let mut mesh = egui::Mesh::default();
    let mut band = |solid: f32, clear: f32| {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(rect.left(), solid), BG);
        mesh.colored_vertex(pos2(rect.right(), solid), BG);
        mesh.colored_vertex(pos2(rect.right(), clear), Color32::TRANSPARENT);
        mesh.colored_vertex(pos2(rect.left(), clear), Color32::TRANSPARENT);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    };
    band(rect.top(), rect.top() + HEIGHT);
    band(rect.bottom(), rect.bottom() - HEIGHT);
    ui.painter().add(mesh);
}

const TITLE_BAR: f32 = 34.0;
const SIDEBAR_WIDTH: f32 = 200.0;
const QUEUE_WIDTH: f32 = 290.0;

/// The app draws its own title bar: it continues the colours of the panels
/// below it, so the window has no visible caption strip. Drag to move,
/// double-click to maximise.
fn title_bar(ui: &mut egui::Ui, wide: bool) {
    let rect = ui.max_rect();
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(rect, 0, BG);
    let side = Rect::from_min_max(rect.min, pos2(rect.left() + SIDEBAR_WIDTH, rect.bottom()));
    ui.painter().rect_filled(side, 0, SIDE);
    if wide {
        let queue = Rect::from_min_max(pos2(rect.right() - QUEUE_WIDTH, rect.top()), rect.max);
        ui.painter().rect_filled(queue, 0, PANEL);
    }

    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let drag = ui.interact(rect, ui.scope_id().with("drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    sub_ui(ui, rect, Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if window_button(ui, WindowGlyph::Close) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let glyph = if maximized { WindowGlyph::Restore } else { WindowGlyph::Maximize };
        if window_button(ui, glyph) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        if window_button(ui, WindowGlyph::Minimize) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    });
}

/// Minimise / maximise / close button of the custom title bar.
fn window_button(ui: &mut egui::Ui, glyph: WindowGlyph) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(46.0, TITLE_BAR), Sense::click());
    if response.hovered() {
        let fill = if glyph == WindowGlyph::Close { Color32::from_rgb(232, 17, 35) } else { HOVER };
        ui.painter().rect_filled(rect, 0, fill);
    }
    let stroke = egui::Stroke::new(1.0, if response.hovered() { Color32::WHITE } else { ICON });
    let painter = ui.painter();
    // Thin line glyphs, like the native Windows 11 caption buttons.
    let c = rect.center().round() + vec2(0.5, 0.5);
    let r = 5.0;
    let square = |offset: egui::Vec2, size: f32| {
        let rect = Rect::from_min_size(c + offset - vec2(r, r), egui::Vec2::splat(size));
        painter.rect_stroke(rect, 1, stroke, egui::StrokeKind::Middle);
    };
    match glyph {
        WindowGlyph::Minimize => {
            painter.line_segment([c + vec2(-r, 0.0), c + vec2(r, 0.0)], stroke);
        }
        WindowGlyph::Maximize => square(vec2(0.0, 0.0), 2.0 * r),
        WindowGlyph::Restore => {
            square(vec2(2.0, 0.0), 2.0 * r - 2.0);
            let (x0, x1, y0, y1) = (c.x - r + 2.0, c.x + r, c.y - r, c.y + r - 2.0);
            painter.line_segment([pos2(x0, y0 + 2.0), pos2(x0, y0)], stroke);
            painter.line_segment([pos2(x0, y0), pos2(x1, y0)], stroke);
            painter.line_segment([pos2(x1, y0), pos2(x1, y1)], stroke);
            painter.line_segment([pos2(x1, y1), pos2(x1 - 2.0, y1)], stroke);
        }
        WindowGlyph::Close => {
            painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
            painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
        }
    }
    response.clicked()
}

#[derive(PartialEq, Clone, Copy)]
enum WindowGlyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Asks Windows 11 for rounded window corners and no accent border.
/// (Ignored on Windows 10, which has neither.)
fn style_window(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    let Ok(handle) = cc.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = HWND(win.hwnd.get() as *mut _);
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    unsafe {
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &COLOR_NONE as *const _ as *const _, 4);
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        if self.state == State::Playing {
            if self.audio.as_ref().is_some_and(|a| a.finished()) {
                self.step(1, true);
            }
            // Keep the clock moving (and auto-advance working while minimised)
            // without burning CPU: a few frames per second is enough.
            let interval = if self.show_lyrics { 100 } else { 500 };
            ctx.request_repaint_after(Duration::from_millis(interval));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let wide = ui.available_width() >= 1040.0;
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing && ui.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_play();
        }

        let panel = |fill: Color32, margin: Margin| egui::Frame::new().fill(fill).inner_margin(margin);
        let mut action = None;

        egui::Panel::top("titlebar")
            .exact_size(TITLE_BAR)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| title_bar(ui, wide));
        egui::Panel::bottom("player")
            .exact_size(74.0)
            .show_separator_line(false)
            .frame(panel(BAR, Margin::ZERO))
            .show(ui, |ui| self.player_bar(ui));
        egui::Panel::left("sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .show_separator_line(false)
            .frame(panel(SIDE, Margin { left: 8, right: 8, top: 0, bottom: 10 }))
            .show(ui, |ui| self.sidebar(ui));
        if wide {
            egui::Panel::right("queue")
                .exact_size(QUEUE_WIDTH)
                .show_separator_line(false)
                .frame(panel(PANEL, Margin { left: 8, right: 8, top: 0, bottom: 10 }))
                .show(ui, |ui| action = self.queue_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(panel(BG, Margin { left: 22, right: 22, top: 4, bottom: 0 }))
            .show(ui, |ui| {
                if self.show_lyrics {
                    self.lyrics_view(ui);
                    return;
                }
                self.search_bar(ui);
                ui.add_space(8.0);
                let page_action = match self.page {
                    Page::Home => self.feed_page(ui, false),
                    Page::Explore => self.feed_page(ui, true),
                    Page::Search | Page::List => self.list_page(ui),
                };
                action = action.take().or(page_action);
            });

        if let Some(action) = action {
            self.apply(action);
        }
    }
}

fn fmt_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
