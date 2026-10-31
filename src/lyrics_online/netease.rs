//! 网易云音乐歌词源。搜索接口 + 歌词接口都是公开的，不需要账号。

use super::{Fetched, LyricSource, RemoteSong};
use crate::library::Song;
use anyhow::Result;
use serde::Deserialize;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

pub struct Netease;

#[derive(Deserialize)]
struct SearchResp {
    result: Option<SearchWrap>,
}
#[derive(Deserialize)]
struct SearchWrap {
    songs: Option<Vec<CandSong>>,
}
#[derive(Deserialize)]
struct CandSong {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    artists: Vec<Artist>,
    album: Option<Album>,
    #[serde(default)]
    duration: u64,
}
#[derive(Deserialize)]
struct Artist {
    #[serde(default)]
    name: String,
}
#[derive(Deserialize)]
struct Album {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct LyricResp {
    lrc: Option<LrcField>,
    tlyric: Option<LrcField>,
}
#[derive(Deserialize)]
struct LrcField {
    lyric: Option<String>,
}

fn get(url: &str) -> Result<String> {
    Ok(ureq::get(url)
        .set("Referer", "https://music.163.com/")
        .set("User-Agent", UA)
        .set("Cookie", "appver=2.0.2")
        .timeout(std::time::Duration::from_secs(12))
        .call()?
        .into_string()?)
}

impl LyricSource for Netease {
    fn label(&self) -> &'static str {
        "网易云"
    }

    fn search(&self, song: &Song, limit: usize) -> Result<Vec<RemoteSong>> {
        let query = super::default_query(song);
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let body = get(&format!(
            "https://music.163.com/api/search/get/web?s={}&type=1&limit={}&offset=0",
            urlencode(&query),
            limit
        ))?;
        let parsed: SearchResp = serde_json::from_str(&body)?;
        let songs = parsed.result.and_then(|r| r.songs).unwrap_or_default();
        Ok(songs
            .into_iter()
            .map(|s| {
                let artists: Vec<String> = s
                    .artists
                    .into_iter()
                    .map(|a| a.name)
                    .filter(|n| !n.is_empty())
                    .collect();
                RemoteSong {
                    source: "netease",
                    id: s.id.to_string(),
                    extra: String::new(),
                    name: s.name,
                    artists,
                    album: s.album.map(|a| a.name).unwrap_or_default(),
                    duration_ms: s.duration,
                    score: 0,
                }
            })
            .collect())
    }

    fn fetch(&self, song: &RemoteSong) -> Result<Option<Fetched>> {
        let body = get(&format!(
            "https://music.163.com/api/song/lyric?id={}&lv=1&kv=1&tv=-1",
            song.id
        ))?;
        let parsed: LyricResp = serde_json::from_str(&body)?;
        let lrc = parsed.lrc.and_then(|l| l.lyric).unwrap_or_default();
        if !lrc.contains('[') || lrc.trim().is_empty() {
            return Ok(None);
        }
        let tlyric = parsed
            .tlyric
            .and_then(|l| l.lyric)
            .filter(|s| !s.trim().is_empty());
        Ok(Some(Fetched {
            lrc,
            translation: tlyric,
        }))
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}
