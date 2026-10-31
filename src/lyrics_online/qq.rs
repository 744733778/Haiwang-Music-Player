//! QQ 音乐歌词源。
//!
//! - 搜索：`music.search.SearchCgiService / DoSearchForQQMusicLite`
//! - 歌词：`music.musichallSong.PlayLyricInfo / GetPlayLyricInfo`，返回加密 hex，
//!   用 `qq_crypt` 做魔改 3DES 解密后是 zlib 流，inflate 得到 QRC XML。
//!
//! 这两条都不需要登录账号（匿名 `tmeLoginType=-1`）。

use super::{qq_crypt, Fetched, LyricSource, RemoteSong};
use crate::library::Song;
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};

const UA: &str = "okhttp/3.14.9";

pub struct Qq;

fn comm() -> Value {
    json!({
        "ct": 11,
        "cv": "1003006",
        "v": "1003006",
        "os_ver": "15",
        "phonetype": "24122RKC7C",
        "rom": "Redmi/miro/miro:15/AE3A.240806.005/OS2.0.102.0.VOMCNXM:user/release-keys",
        "tmeAppID": "qqmusiclight",
        "nettype": "NETWORK_WIFI",
        "udid": "0",
        "uid": "0"
    })
}

fn call(method: &str, module: &str, param: Value) -> Result<Value> {
    // QQ 的 2001 是频控（同一 IP 请求太密），隔一会儿重试通常就过
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..3u32 {
        match call_once(method, module, &param) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let throttled = e.to_string().contains("2001");
                last_err = Some(e);
                if throttled && attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(500 + attempt as u64 * 500));
                    continue;
                }
                break;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("QQ 请求失败")))
}

fn call_once(method: &str, module: &str, param: &Value) -> Result<Value> {
    let body = json!({
        "comm": comm(),
        "request": { "method": method, "module": module, "param": param }
    });
    let text = ureq::post("https://u.y.qq.com/cgi-bin/musicu.fcg")
        .set("Content-Type", "application/json")
        .set("User-Agent", UA)
        .set("Cookie", "tmeLoginType=-1;")
        .timeout(std::time::Duration::from_secs(15))
        .send_string(&body.to_string())?
        .into_string()?;
    let v: Value = serde_json::from_str(&text)?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(anyhow!("QQ 接口返回 code={}", code));
    }
    let req = v.get("request").ok_or_else(|| anyhow!("QQ 响应缺少 request"))?;
    let rcode = req.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    if rcode != 0 {
        return Err(anyhow!("QQ 接口返回 code={}（可能触发了风控）", rcode));
    }
    Ok(req.get("data").cloned().unwrap_or(Value::Null))
}

fn b64(s: &str) -> String {
    B64.encode(s.as_bytes())
}

fn inflate(bytes: &[u8]) -> Result<String> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes).read_to_end(&mut out)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn decrypt_hex(hex: &str) -> Result<String> {
    let hex = hex.trim();
    if hex.is_empty() {
        return Err(anyhow!("空歌词"));
    }
    let mut raw = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16);
        let lo = (bytes[i + 1] as char).to_digit(16);
        match (hi, lo) {
            (Some(h), Some(l)) => raw.push(((h << 4) | l) as u8),
            _ => i += 1,
        }
        i += 2;
    }
    inflate(&qq_crypt::decrypt(&raw))
}

impl LyricSource for Qq {
    fn label(&self) -> &'static str {
        "QQ 音乐"
    }

    fn search(&self, song: &Song, limit: usize) -> Result<Vec<RemoteSong>> {
        let query = super::default_query(song);
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let q = if query.chars().count() > 60 {
            query.chars().take(60).collect::<String>()
        } else {
            query.to_string()
        };
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64 % 100_000_000_000_000)
            .unwrap_or(12345);
        let data = call(
            "DoSearchForQQMusicLite",
            "music.search.SearchCgiService",
            json!({
                "search_id": seed.to_string(),
                "remoteplace": "search.android.keyboard",
                "query": q,
                // 7 = 只搜歌曲；0 会被 QQ 以 2001 拒绝
                "search_type": 7,
                "num_per_page": limit,
                "page_num": 1,
                "highlight": 0,
                "nqc_flag": 0,
                "page_id": 1,
                "grp": 1
            }),
        )?;
        let songs = data
            .get("body")
            .and_then(|b| b.get("item_song"))
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(songs
            .iter()
            .filter_map(|s| {
                let id = s.get("id").and_then(|v| v.as_i64())?;
                let mid = s.get("mid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let artists: Vec<String> = s
                    .get("singer")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                            .map(|n| n.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                Some(RemoteSong {
                    source: "qq",
                    id: id.to_string(),
                    extra: mid,
                    name: s
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    artists,
                    album: s
                        .get("album")
                        .and_then(|a| a.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    duration_ms: s
                        .get("interval")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                        * 1000,
                    score: 0,
                })
            })
            .collect())
    }

    fn fetch(&self, song: &RemoteSong) -> Result<Option<Fetched>> {
        let song_id: i64 = song.id.parse().unwrap_or(0);
        let artists = song.artists.join(", ");
        let interval = (song.duration_ms / 1000) as i64;
        let data = call(
            "GetPlayLyricInfo",
            "music.musichallSong.PlayLyricInfo",
            json!({
                "albumName": b64(&song.album),
                "crypt": 1,
                "ct": 19,
                "cv": 2111,
                "interval": interval,
                "lrc_t": 0,
                "qrc": 1,
                "qrc_t": 0,
                "roma": 0,
                "roma_t": 0,
                "singerName": b64(&artists),
                "songID": song_id,
                "songName": b64(&song.name),
                "trans": 1,
                "trans_t": 0,
                "type": 0
            }),
        )?;

        let lyric_hex = data.get("lyric").and_then(|v| v.as_str()).unwrap_or("");
        if lyric_hex.is_empty() {
            return Ok(None);
        }
        let plain = decrypt_hex(lyric_hex)?;
        let lrc = super::convert::qrc_to_lrc(&plain);
        if lrc.trim().is_empty() {
            return Ok(None);
        }
        let translation = data
            .get("trans")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .and_then(|hex| decrypt_hex(hex).ok())
            .map(|t| super::convert::qrc_to_lrc(&t))
            .filter(|t| !t.trim().is_empty());

        Ok(Some(Fetched { lrc, translation }))
    }
}
