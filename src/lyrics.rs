//! Multi-source lyrics fetcher.
//!
//! Sources are queried in parallel and ranked:
//!   1. LRCLIB   (time-synced)
//!   2. NetEase  (time-synced)
//!   3. LRCLIB   (plain text)
//!   4. lyrics.ovh (plain text)
//! The page falls back to YouTube Music's own Lyrics tab if all of them miss.

use std::time::Duration;

use serde_json::{json, Value};

const UA: &str = "YtMusicRust/0.1 (desktop lyrics client)";
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";
/// Max difference (seconds) between the playing track and a candidate.
const DURATION_TOLERANCE: f64 = 3.5;

pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds; 0 when unknown.
    pub duration: f64,
}

pub struct Line {
    pub t: f64,
    pub text: String,
}

pub struct Found {
    pub source: &'static str,
    pub synced: bool,
    pub lines: Vec<Line>,
}

/// Fetches lyrics and returns the JSON payload handed to the page.
pub fn fetch_json(key: &str, q: &Query) -> String {
    let found = fetch(q);
    let (source, synced, lines) = match &found {
        Some(f) => (f.source, f.synced, f.lines.as_slice()),
        None => ("", false, &[][..]),
    };
    json!({
        "key": key,
        "source": source,
        "synced": synced,
        "lines": lines.iter().map(|l| json!({ "t": l.t, "text": l.text })).collect::<Vec<_>>(),
    })
    .to_string()
}

pub fn fetch(q: &Query) -> Option<Found> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();

    let ((lrc_synced, lrc_plain), netease_synced) = std::thread::scope(|s| {
        let a = s.spawn(|| lrclib(&agent, q));
        let b = s.spawn(|| netease(&agent, q));
        (a.join().unwrap_or((None, None)), b.join().unwrap_or(None))
    });

    if let Some(lines) = lrc_synced {
        return Some(Found { source: "LRCLIB", synced: true, lines });
    }
    if let Some(lines) = netease_synced {
        return Some(Found { source: "NetEase Cloud Music", synced: true, lines });
    }
    if let Some(text) = lrc_plain {
        return Some(Found { source: "LRCLIB", synced: false, lines: plain_lines(&text) });
    }
    lyrics_ovh(&agent, q).map(|text| Found {
        source: "lyrics.ovh",
        synced: false,
        lines: plain_lines(&text),
    })
}

fn get(agent: &ureq::Agent, url: &str, query: &[(&str, &str)], netease: bool) -> Option<Value> {
    let mut req = agent.get(url).header("User-Agent", if netease { BROWSER_UA } else { UA });
    if netease {
        req = req.header("Referer", "https://music.163.com/");
    }
    for (k, v) in query {
        req = req.query(k, v);
    }
    let mut resp = req.call().ok()?;
    let body = resp.body_mut().read_to_string().ok()?;
    serde_json::from_str(&body).ok()
}

// ---------------------------------------------------------------- LRCLIB

fn lrclib(agent: &ureq::Agent, q: &Query) -> (Option<Vec<Line>>, Option<String>) {
    let mut plain: Option<String> = None;
    let mut take = |v: &Value| -> Option<Vec<Line>> {
        if plain.is_none() {
            plain = non_empty(v["plainLyrics"].as_str());
        }
        non_empty(v["syncedLyrics"].as_str())
            .map(|s| parse_lrc(&s))
            .filter(|l| !l.is_empty())
    };

    // Exact signature match first.
    if q.duration > 0.0 {
        let dur = format!("{}", q.duration.round() as i64);
        let hit = get(
            agent,
            "https://lrclib.net/api/get",
            &[
                ("track_name", &q.title),
                ("artist_name", &q.artist),
                ("album_name", &q.album),
                ("duration", &dur),
            ],
            false,
        );
        if let Some(lines) = hit.as_ref().and_then(&mut take) {
            return (Some(lines), plain);
        }
    }

    // Fuzzy search, closest duration wins.
    let title = clean_title(&q.title);
    let results = get(
        agent,
        "https://lrclib.net/api/search",
        &[("track_name", &title), ("artist_name", &q.artist)],
        false,
    );
    let mut candidates: Vec<&Value> = results
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    candidates.retain(|c| duration_ok(q.duration, c["duration"].as_f64().unwrap_or(0.0)));
    candidates.sort_by(|a, b| {
        let da = (a["duration"].as_f64().unwrap_or(0.0) - q.duration).abs();
        let db = (b["duration"].as_f64().unwrap_or(0.0) - q.duration).abs();
        da.total_cmp(&db)
    });
    for c in candidates {
        if let Some(lines) = take(c) {
            return (Some(lines), plain);
        }
    }
    (None, plain)
}

// --------------------------------------------------------------- NetEase

fn netease(agent: &ureq::Agent, q: &Query) -> Option<Vec<Line>> {
    let term = format!("{} {}", clean_title(&q.title), q.artist);
    let search = get(
        agent,
        "https://music.163.com/api/search/get",
        &[("s", &term), ("type", "1"), ("limit", "8")],
        true,
    )?;
    let songs = search["result"]["songs"].as_array()?;
    let song = songs
        .iter()
        .find(|s| duration_ok(q.duration, s["duration"].as_f64().unwrap_or(0.0) / 1000.0))?;
    let id = song["id"].as_i64()?.to_string();

    let lyric = get(
        agent,
        "https://music.163.com/api/song/lyric",
        &[("id", &id), ("lv", "1"), ("kv", "1"), ("tv", "-1")],
        true,
    )?;
    let mut lines = parse_lrc(lyric["lrc"]["lyric"].as_str()?);
    // Drop credit lines such as "作词 : someone".
    lines.retain(|l| !is_credit_line(&l.text));
    // A real synced lyric has more than a couple of timestamps.
    (lines.iter().filter(|l| !l.text.is_empty()).count() >= 4).then_some(lines)
}

fn is_credit_line(text: &str) -> bool {
    let Some(idx) = text.find([':', '：']) else {
        return false;
    };
    let head = text[..idx].trim();
    head.chars().count() <= 12 && !head.is_ascii()
}

// ------------------------------------------------------------ lyrics.ovh

fn lyrics_ovh(agent: &ureq::Agent, q: &Query) -> Option<String> {
    let url = format!(
        "https://api.lyrics.ovh/v1/{}/{}",
        encode_path(&q.artist),
        encode_path(&clean_title(&q.title))
    );
    let v = get(agent, &url, &[], false)?;
    non_empty(v["lyrics"].as_str())
}

fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------- helpers

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn duration_ok(wanted: f64, candidate: f64) -> bool {
    // Unknown duration on our side: accept; we can't do better.
    wanted <= 0.0 || (candidate - wanted).abs() <= DURATION_TOLERANCE
}

/// Strips video-ish decorations: "Song (Official Video) [4K]" -> "Song".
fn clean_title(title: &str) -> String {
    const NOISE: [&str; 9] = [
        "official", "video", "audio", "lyric", "visualizer", "mv", "hd", "4k", "remaster",
    ];
    let mut out = String::new();
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close_ch = if rest.as_bytes()[open] == b'(' { ')' } else { ']' };
        let Some(len) = rest[open..].find(close_ch) else { break };
        let inner = rest[open + 1..open + len].to_lowercase();
        out.push_str(&rest[..open]);
        if !NOISE.iter().any(|n| inner.contains(n)) {
            out.push_str(&rest[open..open + len + 1]);
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() { title.trim().to_owned() } else { cleaned }
}

fn plain_lines(text: &str) -> Vec<Line> {
    text.lines()
        .map(|l| Line { t: -1.0, text: l.trim().to_owned() })
        .collect()
}

/// Parses LRC ("[mm:ss.xx] text", possibly several timestamps per line).
pub fn parse_lrc(lrc: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            match parse_timestamp(&rest[1..end]) {
                Some(t) => times.push(t),
                None => {
                    // Metadata tag like [ar:...] — not a lyric line.
                    times.clear();
                    rest = "";
                    break;
                }
            }
            rest = rest[end + 1..].trim_start();
        }
        if times.is_empty() {
            continue;
        }
        let text = strip_word_tags(rest);
        for t in times {
            lines.push(Line { t, text: text.clone() });
        }
    }
    lines.sort_by(|a, b| a.t.total_cmp(&b.t));
    lines
}

fn parse_timestamp(tag: &str) -> Option<f64> {
    let (min, sec) = tag.split_once(':')?;
    let min: f64 = min.trim().parse().ok()?;
    let sec: f64 = sec.trim().replace(':', ".").parse().ok()?;
    Some(min * 60.0 + sec)
}

/// Removes enhanced-LRC word tags such as "<00:12.34>".
fn strip_word_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(len) = rest[open..].find('>') else { break };
        let inner = &rest[open + 1..open + len];
        out.push_str(&rest[..open]);
        if parse_timestamp(inner).is_none() {
            out.push_str(&rest[open..open + len + 1]);
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let l = parse_lrc("[ar:x]\n[00:01.50]a\n[00:05.00][00:03.00]b <00:03.10>c\n[01:00:25]d");
        let got: Vec<(f64, &str)> = l.iter().map(|l| (l.t, l.text.as_str())).collect();
        assert_eq!(got, [(1.5, "a"), (3.0, "b c"), (5.0, "b c"), (60.25, "d")]);
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Song (Official Video) [4K]"), "Song");
        assert_eq!(clean_title("Song (feat. X)"), "Song (feat. X)");
    }

    /// Live network check: `cargo test -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_sources() {
        let q = Query {
            title: "Bohemian Rhapsody".into(),
            artist: "Queen".into(),
            album: "A Night at the Opera".into(),
            duration: 355.0,
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(8)))
            .build()
            .into();
        let (synced, plain) = lrclib(&agent, &q);
        println!("LRCLIB synced lines: {:?}, plain: {}", synced.map(|l| l.len()), plain.is_some());
        println!("NetEase synced lines: {:?}", netease(&agent, &q).map(|l| l.len()));
        println!("lyrics.ovh plain: {}", lyrics_ovh(&agent, &q).is_some());
        let f = fetch(&q).expect("no source returned lyrics");
        println!("winner: {} synced={} lines={}", f.source, f.synced, f.lines.len());
    }
}
