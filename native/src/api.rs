//! Minimal client for YouTube Music's internal (InnerTube) API.
//! This API is undocumented and can change without notice.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha1::{Digest, Sha1};

const ORIGIN: &str = "https://music.youtube.com";
const CLIENT_VERSION: &str = "1.20261104.03.00";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// Search filter: songs only.
const SONGS_FILTER: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA==";

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds; 0 when unknown.
    pub duration: f64,
    pub thumb: String,
}

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: String,
    pub title: String,
}

/// What happens when a card on Home/Explore is activated.
#[derive(Clone, Debug)]
pub enum Target {
    /// A single song: play it and continue with its radio.
    Track(Track),
    /// A playlist or album page (browse id).
    Browse(String),
    /// A mix / radio (watch-playlist id).
    Watch(String),
}

#[derive(Clone, Debug)]
pub struct Card {
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub enum Items {
    Tracks(Vec<Track>),
    Cards(Vec<Card>),
}

/// One shelf of the Home / Explore feed, e.g. "Quick picks".
#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub items: Items,
}

pub struct Api {
    agent: ureq::Agent,
    cookies: Option<String>,
}

impl Api {
    pub fn new(cookies: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Self { agent, cookies }
    }

    pub fn signed_in(&self) -> bool {
        self.cookies.is_some()
    }

    fn post(&self, endpoint: &str, mut body: Value) -> Result<Value, String> {
        body["context"] = json!({
            "client": { "clientName": "WEB_REMIX", "clientVersion": CLIENT_VERSION, "hl": "en" }
        });
        let url = format!("{ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false");
        let mut req = self
            .agent
            .post(&url)
            .header("User-Agent", UA)
            .header("Content-Type", "application/json")
            .header("Origin", ORIGIN)
            .header("X-Origin", ORIGIN)
            .header("Referer", "https://music.youtube.com/")
            .header("X-Youtube-Client-Name", "67")
            .header("X-Youtube-Client-Version", CLIENT_VERSION);
        if let Some(cookies) = &self.cookies {
            req = req.header("Cookie", cookies).header("X-Goog-AuthUser", "0");
            if let Some(auth) = sapisid_hash(cookies) {
                req = req.header("Authorization", &auth);
            }
        }
        let mut resp = req.send(body.to_string()).map_err(|e| match e {
            ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403) => {
                "Session expired or access denied. Please sign in again.".to_owned()
            }
            other => format!("Request failed: {other}"),
        })?;
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    pub fn search(&self, query: &str) -> Result<Vec<Track>, String> {
        let v = self.post("search", json!({ "query": query, "params": SONGS_FILTER }))?;
        Ok(tracks_in(&v))
    }

    /// `browse_id` is a playlist browse id such as "VLLM" (Liked Music).
    pub fn playlist_tracks(&self, browse_id: &str) -> Result<Vec<Track>, String> {
        let v = self.post("browse", json!({ "browseId": browse_id }))?;
        Ok(tracks_in(&v))
    }

    /// The personalised Home feed (generic when signed out).
    pub fn home(&self) -> Result<Vec<Section>, String> {
        Ok(sections_in(&self.post("browse", json!({ "browseId": "FEmusic_home" }))?))
    }

    /// New releases, charts and trending.
    pub fn explore(&self) -> Result<Vec<Section>, String> {
        Ok(sections_in(&self.post("browse", json!({ "browseId": "FEmusic_explore" }))?))
    }

    /// The play queue YouTube Music builds for a song (its radio) or for a mix.
    pub fn watch_queue(&self, video_id: Option<&str>, playlist_id: Option<&str>) -> Result<Vec<Track>, String> {
        let mut body = json!({
            "enablePersistentPlaylistPanel": true,
            "isAudioOnly": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
            "watchEndpointMusicSupportedConfigs": { "watchEndpointMusicConfig": {
                "hasPersistentPlaylistPanel": true,
                "musicVideoType": "MUSIC_VIDEO_TYPE_ATV"
            }}
        });
        if let Some(id) = video_id {
            body["videoId"] = json!(id);
        }
        match (playlist_id, video_id) {
            (Some(list), _) => body["playlistId"] = json!(list),
            (None, Some(id)) => {
                body["playlistId"] = json!(format!("RDAMVM{id}"));
                body["params"] = json!("wAEB");
            }
            (None, None) => return Err("Nothing to play".into()),
        }
        let v = self.post("next", body)?;
        let mut items = Vec::new();
        collect(&v, "playlistPanelVideoRenderer", &mut items);
        Ok(items.into_iter().filter_map(parse_panel_track).collect())
    }

    /// Adds or removes a song from Liked Music. Requires sign-in.
    pub fn set_like(&self, video_id: &str, liked: bool) -> Result<(), String> {
        if !self.signed_in() {
            return Err("Sign in to like songs.".into());
        }
        let endpoint = if liked { "like/like" } else { "like/removelike" };
        self.post(endpoint, json!({ "target": { "videoId": video_id } })).map(|_| ())
    }

    pub fn library_playlists(&self) -> Result<Vec<Playlist>, String> {
        let v = self.post("browse", json!({ "browseId": "FEmusic_liked_playlists" }))?;
        let mut items = Vec::new();
        collect(&v, "musicTwoRowItemRenderer", &mut items);
        let mut out: Vec<Playlist> = vec![Playlist {
            id: "VLLM".into(),
            title: "Liked Music".into(),
        }];
        for item in items {
            let Some(id) = item["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str() else {
                continue;
            };
            if !id.starts_with("VL") || out.iter().any(|p| p.id == id) {
                continue;
            }
            out.push(Playlist {
                id: id.to_owned(),
                title: runs_text(&item["title"]),
            });
        }
        Ok(out)
    }
}

/// `Authorization: SAPISIDHASH <ts>_<sha1(ts SAPISID origin)>`, as the web client sends.
fn sapisid_hash(cookies: &str) -> Option<String> {
    let find = |name: &str| {
        cookies.split(';').find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == name).then(|| v.to_owned())
        })
    };
    let sapisid = find("SAPISID").or_else(|| find("__Secure-3PAPISID"))?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let digest = Sha1::digest(format!("{ts} {sapisid} {ORIGIN}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("SAPISIDHASH {ts}_{hex}"))
}

// ------------------------------------------------------------ JSON parsing

/// Collects every object stored under `key`, anywhere in the tree.
fn collect<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == key {
                    out.push(child);
                } else {
                    collect(child, key, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|c| collect(c, key, out)),
        _ => {}
    }
}

fn find_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| map.values().find_map(|c| find_str(c, key))),
        Value::Array(items) => items.iter().find_map(|c| find_str(c, key)),
        _ => None,
    }
}

fn runs_text(v: &Value) -> String {
    v["runs"]
        .as_array()
        .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect())
        .unwrap_or_default()
}

fn tracks_in(v: &Value) -> Vec<Track> {
    let mut items = Vec::new();
    collect(v, "musicResponsiveListItemRenderer", &mut items);
    items.into_iter().filter_map(parse_track).collect()
}

fn parse_track(r: &Value) -> Option<Track> {
    let id = r["playlistItemData"]["videoId"]
        .as_str()
        .or_else(|| find_str(&r["flexColumns"], "videoId"))
        .or_else(|| find_str(&r["overlay"], "videoId"))?
        .to_owned();

    let cols = r["flexColumns"].as_array()?;
    let col = |i: usize| -> &Value {
        cols.get(i)
            .map(|c| &c["musicResponsiveListItemFlexColumnRenderer"]["text"])
            .unwrap_or(&Value::Null)
    };
    let title = runs_text(col(0));
    if title.is_empty() {
        return None;
    }

    // Second column: "Artist • Album • 3:55" (search) or just the artists (playlists).
    let groups: Vec<String> = runs_text(col(1))
        .split('\u{2022}')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let artist = groups.first().cloned().unwrap_or_default();

    let fixed = runs_text(&r["fixedColumns"][0]["musicResponsiveListItemFixedColumnRenderer"]["text"]);
    let duration = parse_duration(&fixed)
        .or_else(|| groups.iter().rev().find_map(|g| parse_duration(g)))
        .unwrap_or(0.0);

    // Album is the third column in playlists, or the second group in search
    // results (where the third column holds the play count instead).
    let is_album = |s: &str| {
        !s.is_empty() && parse_duration(s).is_none() && !s.ends_with(" plays") && !s.ends_with(" views")
    };
    let mut album = runs_text(col(2));
    if !is_album(&album) {
        album = groups.get(1).filter(|g| is_album(g)).cloned().unwrap_or_default();
    }

    let thumb = r["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"]
        .as_array()
        .and_then(|t| t.first())
        .and_then(|t| t["url"].as_str())
        .unwrap_or_default()
        .to_owned();

    Some(Track { id, title, artist, album, duration, thumb })
}

fn bullet_groups(text: &str) -> Vec<String> {
    text.split('\u{2022}')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_stat(s: &str) -> bool {
    let s = s.to_lowercase();
    s.ends_with(" plays") || s.ends_with(" views") || s.ends_with(" likes") || s.ends_with(" subscribers")
}

fn largest_thumb(thumbnails: &Value) -> String {
    thumbnails
        .as_array()
        .and_then(|t| t.last())
        .and_then(|t| t["url"].as_str())
        .unwrap_or_default()
        .to_owned()
}

fn sections_in(v: &Value) -> Vec<Section> {
    let mut shelves = Vec::new();
    collect(v, "musicCarouselShelfRenderer", &mut shelves);
    let mut out = Vec::new();
    for shelf in shelves {
        let title = runs_text(&shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]);
        let contents = shelf["contents"].as_array().map(Vec::as_slice).unwrap_or_default();
        let tracks: Vec<Track> = contents
            .iter()
            .filter_map(|c| parse_track(&c["musicResponsiveListItemRenderer"]))
            .collect();
        let cards: Vec<Card> = contents
            .iter()
            .filter_map(|c| parse_card(&c["musicTwoRowItemRenderer"]))
            .collect();
        let items = if tracks.len() >= cards.len() && !tracks.is_empty() {
            Items::Tracks(tracks)
        } else if !cards.is_empty() {
            Items::Cards(cards)
        } else {
            continue;
        };
        if !title.is_empty() {
            out.push(Section { title, items });
        }
    }
    out
}

fn parse_card(r: &Value) -> Option<Card> {
    let title = runs_text(&r["title"]);
    if title.is_empty() {
        return None;
    }
    let subtitle = runs_text(&r["subtitle"]);
    let thumb = largest_thumb(&r["thumbnailRenderer"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"]);
    let nav = &r["navigationEndpoint"];
    let target = if let Some(id) = nav["watchEndpoint"]["videoId"].as_str() {
        // Subtitle looks like "Song • Artist" or "Artist • 12M views".
        let artist = bullet_groups(&subtitle)
            .into_iter()
            .find(|g| !is_stat(g) && !matches!(g.as_str(), "Song" | "Video" | "Single" | "EP" | "Album"))
            .unwrap_or_default();
        Target::Track(Track { id: id.to_owned(), title: title.clone(), artist, thumb: thumb.clone(), ..Default::default() })
    } else if let Some(id) = nav["watchPlaylistEndpoint"]["playlistId"].as_str() {
        Target::Watch(id.to_owned())
    } else {
        let id = nav["browseEndpoint"]["browseId"].as_str()?;
        // Playlists and albums only; artist and category pages are not supported yet.
        if !(id.starts_with("VL") || id.starts_with("MPRE")) {
            return None;
        }
        Target::Browse(id.to_owned())
    };
    Some(Card { title, subtitle, thumb, target })
}

/// An entry of the "Up next" panel returned by the `next` endpoint.
fn parse_panel_track(r: &Value) -> Option<Track> {
    let id = r["videoId"].as_str()?.to_owned();
    let title = runs_text(&r["title"]);
    if title.is_empty() {
        return None;
    }
    let groups = bullet_groups(&runs_text(&r["longBylineText"]));
    let artist = groups.first().cloned().unwrap_or_default();
    let album = groups
        .get(1)
        .filter(|g| !is_stat(g) && !g.bytes().all(|b| b.is_ascii_digit()))
        .cloned()
        .unwrap_or_default();
    let duration = parse_duration(&runs_text(&r["lengthText"])).unwrap_or(0.0);
    let thumb = r["thumbnail"]["thumbnails"]
        .as_array()
        .and_then(|t| t.first())
        .and_then(|t| t["url"].as_str())
        .unwrap_or_default()
        .to_owned();
    Some(Track { id, title, artist, album, duration, thumb })
}

/// "3:55" or "1:02:03" -> seconds.
fn parse_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || !s.contains(':') {
        return None;
    }
    let mut total = 0.0;
    for part in s.split(':') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(total)
}

/// Rewrites a googleusercontent thumbnail URL to another square size.
pub fn thumb_sized(url: &str, px: u32) -> String {
    match url.rfind("=w") {
        Some(i) if url.contains("googleusercontent.com") => format!("{}=w{px}-h{px}-l90-rj", &url[..i]),
        _ => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("3:55"), Some(235.0));
        assert_eq!(parse_duration("1:02:03"), Some(3723.0));
        assert_eq!(parse_duration("Queen"), None);
    }

    fn describe(sections: &[Section]) {
        for s in sections {
            match &s.items {
                Items::Tracks(t) => println!(
                    "[tracks x{}] {} | first: {:?} / {:?} thumb={}",
                    t.len(), s.title, t[0].title, t[0].artist, !t[0].thumb.is_empty()
                ),
                Items::Cards(c) => println!(
                    "[cards  x{}] {} | first: {:?} / {:?} -> {} thumb={}",
                    c.len(), s.title, c[0].title, c[0].subtitle,
                    match &c[0].target { Target::Track(_) => "track", Target::Browse(_) => "browse", Target::Watch(_) => "watch" },
                    !c[0].thumb.is_empty()
                ),
            }
        }
    }

    #[test]
    #[ignore]
    fn live_home_explore_queue() {
        let api = Api::new(None);
        let home = api.home().expect("home failed");
        println!("--- home: {} sections", home.len());
        describe(&home);
        let explore = api.explore().expect("explore failed");
        println!("--- explore: {} sections", explore.len());
        describe(&explore);
        assert!(!home.is_empty() || !explore.is_empty());

        let radio = api.watch_queue(Some("BSTsnWoslP4"), None).expect("radio failed");
        println!("--- radio: {} tracks; first two: {:?}", radio.len(), radio.iter().take(2).map(|t| (&t.title, &t.artist, &t.album, t.duration)).collect::<Vec<_>>());
        assert!(radio.len() > 5 && radio[0].duration > 0.0);

        // Open the first browse/watch card found, as the UI would.
        let card = home.iter().chain(&explore).find_map(|s| match &s.items {
            Items::Cards(c) => c.iter().find(|c| !matches!(c.target, Target::Track(_))).cloned(),
            _ => None,
        });
        if let Some(card) = card {
            let tracks = match &card.target {
                Target::Browse(id) => api.playlist_tracks(id),
                Target::Watch(id) => api.watch_queue(None, Some(id)),
                Target::Track(_) => unreachable!(),
            }
            .expect("card open failed");
            println!("--- card {:?} ({:?}) -> {} tracks; first: {:?}", card.title, card.target, tracks.len(), tracks.first().map(|t| (&t.title, &t.artist, t.duration, !t.thumb.is_empty())));
        }
    }

    /// Live check against YouTube Music: `cargo test -p ytmusic-native -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_search() {
        let tracks = Api::new(None).search("bohemian rhapsody queen").expect("search failed");
        println!("results: {}", tracks.len());
        for t in tracks.iter().take(3) {
            println!("{t:?}");
        }
        assert!(!tracks.is_empty());
        assert!(tracks[0].duration > 0.0 && !tracks[0].artist.is_empty());
    }
}
