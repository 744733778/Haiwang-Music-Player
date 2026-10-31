//! 酷狗音乐歌词源。
//!
//! 搜索走 `complexsearch.kugou.com`，歌词走 `lyrics.kugou.com` 的 search + download，
//! download 返回 base64 的 KRC（`krc1` 头 + 定长 XOR + zlib）。
//! 请求需要 md5 签名：`md5(SECRET + k1=v1k2=v2…（键名排序） + SECRET)`。

use super::{Fetched, LyricSource, RemoteSong};
use crate::library::Song;
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const SIGN_SECRET: &str = "LnT6xpN3khm36zse0QzvmgTZ3waWdRSA";
const KRC_XOR_KEY: [u8; 16] = [
    64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105,
];

pub struct Kugou;

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn md5_hex(s: &str) -> String {
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// 拼接成排序后的 `k=v` 串（无分隔符，酷狗就是这么算的）
fn sign(params: &[(String, String)]) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::from(SIGN_SECRET);
    for (k, v) in &sorted {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
    }
    s.push_str(SIGN_SECRET);
    md5_hex(&s)
}

fn request(url: &str, params: Vec<(String, String)>, module: &str, extra_headers: &[(&str, &str)]) -> Result<Value> {
    let client_ms = now_ms();
    let mid = md5_hex(&client_ms.to_string());

    let mut p = params;
    if module != "Lyric" {
        p.push(("userid".into(), "0".into()));
        p.push(("appid".into(), "3116".into()));
        p.push(("token".into(), String::new()));
        p.push(("clienttime".into(), (client_ms / 1000).to_string()));
        p.push(("iscorrection".into(), "1".into()));
        p.push(("uuid".into(), "-".into()));
        p.push(("mid".into(), mid.clone()));
        p.push(("dfid".into(), "-".into()));
        p.push(("clientver".into(), "11070".into()));
        p.push(("platform".into(), "AndroidFilter".into()));
    } else {
        p.push(("appid".into(), "3116".into()));
        p.push(("clientver".into(), "11070".into()));
    }
    let signature = sign(&p);
    p.push(("signature".into(), signature));

    let qs: String = p
        .iter()
        .map(|(k, v)| format!("{}={}", k, super::netease::urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let full = format!("{}?{}", url, qs);

    let ua = format!("Android14-1070-11070-201-0-{}-wifi", module);
    let mut req = ureq::get(&full)
        .set("User-Agent", &ua)
        .set("Connection", "Keep-Alive")
        .set("KG-Rec", "1")
        .set("KG-RC", "1")
        .set("KG-CLIENTTIMEMS", &client_ms.to_string())
        .set("mid", &mid)
        .timeout(std::time::Duration::from_secs(15));
    for (k, v) in extra_headers {
        req = req.set(k, v);
    }
    let text = req.call()?.into_string()?;
    let v: Value = serde_json::from_str(&text)?;
    if let Some(code) = v.get("error_code").and_then(|c| c.as_i64()) {
        if code != 0 && code != 200 {
            return Err(anyhow!("酷狗接口返回 error_code={}", code));
        }
    }
    Ok(v)
}

/// KRC 解密：跳过 4 字节头，逐字节 XOR，再 zlib 解压
fn krc_decrypt(raw: &[u8]) -> Result<String> {
    use std::io::Read;
    if raw.len() <= 4 || &raw[..4] != b"krc1" {
        return Err(anyhow!("不是 KRC 数据"));
    }
    let data = &raw[4..];
    let mut xored = Vec::with_capacity(data.len());
    for (i, b) in data.iter().enumerate() {
        xored.push(b ^ KRC_XOR_KEY[i % KRC_XOR_KEY.len()]);
    }
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&xored[..]).read_to_end(&mut out)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

impl Kugou {
    fn search_once(&self, keyword: &str, limit: usize) -> Result<Vec<RemoteSong>> {
        let v = request(
            "http://complexsearch.kugou.com/v2/search/song",
            vec![
                ("sorttype".into(), "0".into()),
                ("keyword".into(), keyword.to_string()),
                ("pagesize".into(), limit.to_string()),
                ("page".into(), "1".into()),
            ],
            "SearchSong",
            &[("x-router", "complexsearch.kugou.com")],
        )?;

        let lists = v
            .get("data")
            .and_then(|d| d.get("lists"))
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(lists
            .iter()
            .filter_map(|s| {
                let hash = s
                    .get("FileHash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                if hash.is_empty() {
                    return None;
                }
                let artists: Vec<String> = s
                    .get("Singers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                            .map(|n| n.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                // 时长字段在不同接口里叫 Duration / HQDuration，单位秒
                let dur = s
                    .get("Duration")
                    .and_then(|v| v.as_u64())
                    .or_else(|| s.get("HQDuration").and_then(|v| v.as_u64()))
                    .unwrap_or(0);
                Some(RemoteSong {
                    source: "kugou",
                    id: hash.clone(),
                    extra: hash,
                    name: s
                        .get("SongName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    artists,
                    album: s
                        .get("AlbumName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    duration_ms: dur * 1000,
                    score: 0,
                })
            })
            .collect())
    }
}

impl LyricSource for Kugou {
    fn label(&self) -> &'static str {
        "酷狗"
    }

    fn search(&self, song: &Song, limit: usize) -> Result<Vec<RemoteSong>> {
        // 酷狗对「标题 歌手」这种长串不友好，先用标题搜；搜不到再退回「标题 歌手」
        let title = super::score::normalize_song_title(&song.title, &song.artist);
        let mut attempts: Vec<String> = vec![super::score::strip_title_suffix(&title)];
        let with_artist = super::default_query(song);
        if !attempts.iter().any(|a| a == &with_artist) {
            attempts.push(with_artist);
        }
        for keyword in attempts {
            let keyword = keyword.trim().to_string();
            if keyword.is_empty() {
                continue;
            }
            let rows = self.search_once(&keyword, limit).unwrap_or_default();
            if !rows.is_empty() {
                return Ok(rows);
            }
        }
        Ok(Vec::new())
    }

    fn fetch(&self, song: &RemoteSong) -> Result<Option<Fetched>> {
        let artists = song.artists.join("/");
        let search = request(
            "https://lyrics.kugou.com/v1/search",
            vec![
                ("duration".into(), song.duration_ms.to_string()),
                ("hash".into(), song.id.clone()),
                ("keyword".into(), format!("{} - {}", artists, song.name)),
                ("lrctxt".into(), "1".into()),
                ("man".into(), "no".into()),
            ],
            "Lyric",
            &[],
        )?;

        let empty = Vec::new();
        let cands = search
            .get("candidates")
            .and_then(|c| c.as_array())
            .unwrap_or(&empty);
        // 按 score 取最高分候选（没有 score 就取第一个）
        let best = cands
            .iter()
            .max_by_key(|c| c.get("score").and_then(|s| s.as_i64()).unwrap_or(0));
        let Some(best) = best else {
            return Ok(None);
        };
        // 酷狗这里 id 是**字符串**（"221524943"），不能只按数字解
        let id = best
            .get("id")
            .and_then(|v| {
                v.as_i64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();
        let accesskey = best
            .get("accesskey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() || accesskey.is_empty() {
            return Ok(None);
        }

        let dl = request(
            "http://lyrics.kugou.com/download",
            vec![
                ("accesskey".into(), accesskey),
                ("charset".into(), "utf8".into()),
                ("client".into(), "mobi".into()),
                ("fmt".into(), "krc".into()),
                ("id".into(), id),
                ("ver".into(), "1".into()),
            ],
            "Lyric",
            &[],
        )?;

        let content = dl.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if content.is_empty() {
            return Ok(None);
        }
        let raw = B64.decode(content)?;
        let text = match krc_decrypt(&raw) {
            Ok(t) => t,
            Err(_) => {
                // 不是 KRC（罕见）：当作纯文本歌词
                String::from_utf8_lossy(&raw).into_owned()
            }
        };

        let lrc = super::convert::krc_to_lrc(&text);
        if lrc.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(Fetched {
            lrc,
            translation: None,
        }))
    }
}
