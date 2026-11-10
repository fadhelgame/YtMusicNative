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
    pub subtitle: String,
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

    pub fn library_playlists(&self) -> Result<Vec<Playlist>, String> {
        let v = self.post("browse", json!({ "browseId": "FEmusic_liked_playlists" }))?;
        let mut items = Vec::new();
        collect(&v, "musicTwoRowItemRenderer", &mut items);
        let mut out: Vec<Playlist> = vec![Playlist {
            id: "VLLM".into(),
            title: "Liked Music".into(),
            subtitle: "Auto playlist".into(),
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
                subtitle: runs_text(&item["subtitle"]),
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
