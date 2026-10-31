//! 在线歌词：多源搜索 + 匹配打分 + 下载落盘。
//!
//! 源的思路移植自 chthollyphile/folia-major：
//! - 同一首歌在多个源上搜，用 `score` 模块的算法打分（标题/歌手/专辑/时长）
//! - 分数够高（`STRONG_SCORE`）就直接取歌词；都没够高时，取所有源里最高分且过 `MIN_SCORE` 的
//! - 任一源报错/超时只当"没结果"，继续下一个源
//!
//! 各源取到的歌词（QRC / KRC / TTML / LRC）统一转成增强 LRC 再落盘，
//! 所以写出去的 `.lrc` 是通用格式，别的播放器也能用。

pub mod amll;
pub mod convert;
pub mod kugou;
pub mod netease;
pub mod qq;
pub mod qq_crypt;
pub mod score;

use crate::library::Song;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::time::Duration;

/// 一次搜索取多少候选
const SEARCH_LIMIT: usize = 30;
/// 达到这个分数就认定是同一首，直接取歌词
pub const STRONG_SCORE: i64 = 75;
/// 低于这个分数不写入（宁缺勿错）
pub const MIN_SCORE: i64 = 60;

/// 一个搜索候选
#[derive(Debug, Clone)]
pub struct RemoteSong {
    pub source: &'static str,
    /// 源内主键：网易数字 id / QQ 数字 id / 酷狗 hash / AMLL 平台 id
    pub id: String,
    /// 附加标识：QQ 的 mid、AMLL 的平台名（ncm 或 qq）
    pub extra: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub duration_ms: u64,
    pub score: i64,
}

impl RemoteSong {
    pub fn artist(&self) -> String {
        self.artists.join(", ")
    }
}

/// 取到的歌词
#[derive(Debug, Clone)]
pub struct Fetched {
    /// 增强 LRC 文本
    pub lrc: String,
    pub translation: Option<String>,
}

pub struct Match {
    pub source_label: &'static str,
    pub song: RemoteSong,
    pub lrc: String,
    pub translation: Option<String>,
}

pub trait LyricSource {
    fn label(&self) -> &'static str;
    /// 用歌曲元数据搜索候选（各源自己决定搜索词形态）
    fn search(&self, song: &Song, limit: usize) -> Result<Vec<RemoteSong>>;
    fn fetch(&self, song: &RemoteSong) -> Result<Option<Fetched>>;
}

/// 默认搜索词：「标题 歌手」（标题里重复写了歌手的会去掉）
pub fn default_query(song: &Song) -> String {
    let title = score::normalize_song_title(&song.title, &song.artist);
    if song.artist.trim().is_empty() {
        title
    } else {
        format!("{} {}", title, song.artist.trim())
    }
}

/// 默认源顺序：AMLL（人工校对逐字，质量最高）→ QQ → 网易 → 酷狗
pub fn default_order() -> Vec<String> {
    ["amll", "qq", "netease", "kugou"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub fn all_source_ids() -> Vec<&'static str> {
    vec!["amll", "qq", "netease", "kugou"]
}

pub fn source_label(id: &str) -> &'static str {
    match id {
        "amll" => "AMLL 逐字库",
        "qq" => "QQ 音乐",
        "netease" => "网易云",
        "kugou" => "酷狗",
        _ => "未知源",
    }
}

fn make(id: &str) -> Option<Box<dyn LyricSource>> {
    match id {
        "amll" => Some(Box::new(amll::Amll)),
        "qq" => Some(Box::new(qq::Qq)),
        "netease" => Some(Box::new(netease::Netease)),
        "kugou" => Some(Box::new(kugou::Kugou)),
        _ => None,
    }
}

/// 按源顺序找最佳歌词
pub fn best_for(song: &Song, order: &[String]) -> Result<Option<Match>> {
    let mut fallback: Option<RemoteSong> = None;

    for id in order {
        let Some(src) = make(id) else { continue };
        let cands = match src.search(song, SEARCH_LIMIT) {
            Ok(c) => c,
            // 单个源失败（风控、超时）只当没结果
            Err(_) => continue,
        };
        if cands.is_empty() {
            continue;
        }

        let mut scored: Vec<RemoteSong> = cands
            .into_iter()
            .map(|mut c| {
                let artist = c.artist();
                c.score = score::score(song, &c.name, &artist, &c.album, c.duration_ms);
                c
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score));

        let top = &scored[0];
        if top.score >= STRONG_SCORE {
            if let Ok(Some(f)) = src.fetch(top) {
                return Ok(Some(Match {
                    source_label: src.label(),
                    song: top.clone(),
                    lrc: f.lrc,
                    translation: f.translation,
                }));
            }
            // 分数够但取不到歌词，继续看后面的源
        }
        if fallback.as_ref().map_or(true, |f| top.score > f.score) {
            fallback = Some(top.clone());
        }
    }

    // 没有强势命中的，退化到「所有源里最高分」——但仍须过 MIN_SCORE
    if let Some(fb) = fallback {
        if fb.score >= MIN_SCORE {
            if let Some(src) = make(fb.source) {
                if let Ok(Some(f)) = src.fetch(&fb) {
                    return Ok(Some(Match {
                        source_label: src.label(),
                        song: fb.clone(),
                        lrc: f.lrc,
                        translation: f.translation,
                    }));
                }
            }
        }
    }

    // 最后手段：QQ 的「按歌名歌手直取」——不走搜索，绕开搜索接口的风控
    if let Some(src) = make("qq") {
        let probe = RemoteSong {
            source: "qq",
            id: "0".to_string(),
            extra: String::new(),
            name: song.title.clone(),
            artists: score::split_artist_names(&song.artist),
            album: song.album.clone(),
            duration_ms: song.duration_ms,
            score: 78,
        };
        if let Ok(Some(f)) = src.fetch(&probe) {
            return Ok(Some(Match {
                source_label: "QQ 音乐（按名称直取）",
                song: probe,
                lrc: f.lrc,
                translation: f.translation,
            }));
        }
    }
    Ok(None)
}

/// 把歌词写到音频同目录：`<文件名>.lrc`（有翻译再写 `<文件名>.tlyric.lrc`）
pub fn save_for_song(song: &Song, lrc: &str, tlyric: Option<&str>) -> Result<PathBuf> {
    let dir = song
        .path
        .parent()
        .ok_or_else(|| anyhow!("音频没有父目录"))?;
    let stem = song
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("lyric");
    let lrc_path = dir.join(format!("{}.lrc", stem));
    std::fs::write(&lrc_path, lrc.as_bytes())?;
    if let Some(t) = tlyric {
        let t = t.trim();
        if !t.is_empty() {
            let _ = std::fs::write(dir.join(format!("{}.tlyric.lrc", stem)), t.as_bytes());
        }
    }
    Ok(lrc_path)
}

pub fn download_for_song(song: &Song, order: &[String]) -> Result<Option<(PathBuf, Match)>> {
    match best_for(song, order)? {
        Some(m) => {
            let p = save_for_song(song, &m.lrc, m.translation.as_deref())?;
            Ok(Some((p, m)))
        }
        None => Ok(None),
    }
}

/// 批量下载缺失歌词。progress(已完成数, 总数, 状态文本)
pub fn download_missing(
    songs: &[Song],
    order: &[String],
    mut progress: impl FnMut(usize, usize, String),
) -> (usize, usize) {
    let total = songs.len();
    let (mut ok, mut fail) = (0usize, 0usize);
    for (i, s) in songs.iter().enumerate() {
        if s.has_lyric_file() {
            progress(i + 1, total, format!("跳过（已有歌词）：{}", s.display()));
            continue;
        }
        match download_for_song(s, order) {
            Ok(Some((_, m))) => {
                ok += 1;
                progress(
                    i + 1,
                    total,
                    format!(
                        "已保存（{} {} 分）：{}",
                        m.source_label, m.song.score, s.display()
                    ),
                );
            }
            Ok(None) => progress(i + 1, total, format!("未匹配到：{}", s.display())),
            Err(e) => {
                fail += 1;
                progress(i + 1, total, format!("失败：{}（{}）", s.display(), e));
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    (ok, fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str, artist: &str, album: &str, dur: u64) -> Song {
        Song {
            path: PathBuf::from("x.mp3"),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms: dur,
        }
    }

    /// 联网端到端：`cargo test -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn net_end_to_end() {
        let samples = [
            ("晴天", "周杰伦", "叶惠美", 269000u64),
            ("海阔天空", "Beyond", "乐与怒", 326000),
            ("起风了", "买辣椒也用券", "", 325000),
            ("Lemon", "米津玄師", "Lemon", 255000),
            ("Shape of You", "Ed Sheeran", "÷", 233000),
        ];
        let order = default_order();
        let mut hit = 0;
        for (title, artist, album, dur) in samples {
            let s = song(title, artist, album, dur);
            match best_for(&s, &order) {
                Ok(Some(m)) => {
                    hit += 1;
                    println!(
                        "[OK] {:<12} <- {:<10} {:>3} 分  {} - {}  行数={} 翻译={} 逐字={}",
                        title,
                        m.source_label,
                        m.song.score,
                        m.song.name,
                        m.song.artist(),
                        m.lrc.lines().count(),
                        m.translation.is_some(),
                        m.lrc.contains('<')
                    );
                }
                Ok(None) => println!("[--] {:<12} 无可信匹配", title),
                Err(e) => println!("[!!] {:<12} {}", title, e),
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        assert!(hit > 0, "所有样本都没匹配到，检查网络");
    }

    /// 验证 QQ 的「按歌名歌手直取」兜底（不走搜索）：`cargo test qq_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn qq_probe_by_name() {
        let s = song("Lemon", "米津玄師", "Lemon", 255000);
        let src = make("qq").unwrap();
        let probe = RemoteSong {
            source: "qq",
            id: "0".to_string(),
            extra: String::new(),
            name: s.title.clone(),
            artists: score::split_artist_names(&s.artist),
            album: s.album.clone(),
            duration_ms: s.duration_ms,
            score: 78,
        };
        match src.fetch(&probe) {
            Ok(Some(f)) => println!(
                "[qq 直取] OK {} 行, 首行={:?}",
                f.lrc.lines().count(),
                f.lrc.lines().next().unwrap_or("")
            ),
            Ok(None) => println!("[qq 直取] 无歌词"),
            Err(e) => println!("[qq 直取] 出错：{}", e),
        }
    }

    /// 逐源打印搜索/取词细节，排查用：`cargo test debug_sources -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn debug_sources() {
        let s = song("Lemon", "米津玄師", "Lemon", 255000);
        for id in all_source_ids() {
            let Some(src) = make(id) else { continue };
            match src.search(&s, 30) {
                Ok(v) => {
                    println!("[{}] search ok: {} 条", id, v.len());
                    for c in v.iter().take(2) {
                        println!("    id={} extra={} {} - {} ({})", c.id, c.extra, c.name, c.artist(), c.album);
                    }
                    if let Some(first) = v.first() {
                        match src.fetch(first) {
                            Ok(Some(f)) => println!(
                                "    fetch ok: {} 行, 首行={:?}",
                                f.lrc.lines().count(),
                                f.lrc.lines().next().unwrap_or("")
                            ),
                            Ok(None) => println!("    fetch: 无歌词"),
                            Err(e) => println!("    fetch ERR: {}", e),
                        }
                    }
                }
                Err(e) => println!("[{}] search ERR: {}", id, e),
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    /// 每个源单独跑一遍，确认各自的接口/解密链路都通：
    /// `cargo test per_source_smoke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn per_source_smoke() {
        let s = song("Lemon", "米津玄師", "Lemon", 255000);
        for id in all_source_ids() {
            let order = vec![id.to_string()];
            match best_for(&s, &order) {
                Ok(Some(m)) => println!(
                    "[{}] OK {:>3} 分  {} - {}  行数={} 逐字={}",
                    source_label(id),
                    m.song.score,
                    m.song.name,
                    m.song.artist(),
                    m.lrc.lines().count(),
                    m.lrc.contains('<')
                ),
                Ok(None) => println!("[{}] 无结果", source_label(id)),
                Err(e) => println!("[{}] 出错：{}", source_label(id), e),
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}
