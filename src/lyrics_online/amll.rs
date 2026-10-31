//! AMLL TTML DB 歌词源（https://api.amll.dev）。
//!
//! 这是一个**人工校对**的歌词库，返回 TTML（逐字时间轴），质量比平台歌词高，
//! 也是「晴天」这类在原唱搜不到的平台限制下的可靠补充。缺点是收录量有限。
//!
//! 搜索用 `q=`（模糊匹配曲名/歌手/专辑），取歌词用 `ncmMusicId=` 或 `qqMusicId=`。

use super::{Fetched, LyricSource, RemoteSong};
use crate::library::Song;
use anyhow::Result;
use serde_json::Value;

const BASE: &str = "https://api.amll.dev";
const UA: &str = "rust-music/0.1 (+https://github.com/744733778)";

pub struct Amll;

fn get(url: &str) -> Result<Option<Value>> {
    let text = ureq::get(url)
        .set("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(12))
        .call()?
        .into_string()?;
    let v: Value = serde_json::from_str(&text)?;
    if v.get("status").and_then(|s| s.as_i64()) != Some(200) {
        return Ok(None);
    }
    Ok(v.get("data").cloned())
}

fn first_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string()
}

impl Amll {
    /// AMLL 的 `q` 是「多词取交集」，带上歌手经常直接 0 条；
    /// 所以按「结构化 → q 全串 → q 只标题」逐级降级，取第一个有结果的。
    fn attempts(song: &Song) -> Vec<Vec<(&'static str, String)>> {
        let title = super::score::normalize_song_title(&song.title, &song.artist);
        let base = super::score::strip_title_suffix(&title);
        let first_artist = super::score::split_artist_names(&song.artist)
            .into_iter()
            .next();
        let mut out: Vec<Vec<(&'static str, String)>> = Vec::new();
        if let Some(a) = first_artist {
            out.push(vec![
                ("musicName", title.clone()),
                ("artistName", a.clone()),
            ]);
            if base != title {
                out.push(vec![("musicName", base.clone()), ("artistName", a)]);
            }
        }
        let full = super::default_query(song);
        if !full.trim().is_empty() {
            out.push(vec![("q", full.clone())]);
        }
        // 关键兜底：只用标题（AMLL 的 q 是多词取交集，带歌手经常 0 条）
        if !base.trim().is_empty() && base != full {
            out.push(vec![("q", base.clone())]);
        }
        if !title.trim().is_empty() && title != base && title != full {
            out.push(vec![("q", title)]);
        }
        out
    }

    fn to_remote(data: &Value) -> Vec<RemoteSong> {
        let items = data
            .get("items")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();

        let mut out: Vec<RemoteSong> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for item in &items {
            let ncm = first_str(item, "ncmMusicIds");
            let qq = first_str(item, "qqMusicIds");
            let (platform, id) = if !ncm.is_empty() {
                ("ncm", ncm)
            } else if !qq.is_empty() {
                ("qq", qq)
            } else {
                continue;
            };
            let key = format!("{}:{}", platform, id);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);

            // artistNames / albumNames 是同一对象的多语言别名，全放进 artists 让评分能认出译名
            let artists: Vec<String> = item
                .get("artistNames")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();

            out.push(RemoteSong {
                source: "amll",
                id,
                extra: platform.to_string(),
                name: first_str(item, "musicNames"),
                artists,
                album: first_str(item, "albumNames"),
                // 官方搜索不返回时长，评分按时长未知处理
                duration_ms: 0,
                score: 0,
            });
        }
        out
    }
}

impl LyricSource for Amll {
    fn label(&self) -> &'static str {
        "AMLL 逐字库"
    }

    fn search(&self, song: &Song, limit: usize) -> Result<Vec<RemoteSong>> {
        for params in Self::attempts(song) {
            let qs: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, super::netease::urlencode(v)))
                .collect::<Vec<_>>()
                .join("&");
            if qs.is_empty() {
                continue;
            }
            let url = format!("{}/v1/lyrics/search?{}&pageSize={}", BASE, qs, limit.max(1));
            let Some(data) = get(&url)? else {
                continue;
            };
            let rows = Self::to_remote(&data);
            if !rows.is_empty() {
                return Ok(rows);
            }
        }
        Ok(Vec::new())
    }

    fn fetch(&self, song: &RemoteSong) -> Result<Option<Fetched>> {
        let key = if song.extra == "qq" { "qqMusicId" } else { "ncmMusicId" };
        let url = format!(
            "{}/v1/lyrics/get?{}={}",
            BASE,
            key,
            super::netease::urlencode(&song.id)
        );
        let Some(data) = get(&url)? else {
            return Ok(None);
        };
        let ttml = data
            .get("lyrics")
            .and_then(|l| l.as_str())
            .unwrap_or("")
            .to_string();
        if !ttml.contains("<tt") && !ttml.contains("<p ") {
            return Ok(None);
        }
        let lrc = super::convert::ttml_to_lrc(&ttml);
        if lrc.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(Fetched {
            lrc,
            translation: None,
        }))
    }
}
