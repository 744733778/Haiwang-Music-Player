use lofty::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "wma", "ape", "alac", "dsf", "dff", "aiff",
    "aif", "mka", "webm",
];

/// 文件签名：大小 + 修改时间（unix 秒）。增量扫描时用来判断标签是否需要重读。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileSig {
    pub size: u64,
    pub mtime: i64,
}

impl FileSig {
    fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let mtime = m
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some(Self {
            size: m.len(),
            mtime,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Song {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    /// 缓存签名：文件没变就跳过 lofty 读标签
    #[serde(default)]
    pub sig: FileSig,
}

impl Song {
    pub fn display(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.title, self.artist)
        }
    }

    pub fn lrc_path(&self) -> PathBuf {
        self.path.with_extension("lrc")
    }

    pub fn has_lyric_file(&self) -> bool {
        let p = self.lrc_path();
        p.exists() || p.with_extension("LRC").exists()
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            AUDIO_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

/// 递归扫描文件夹，读取标签。读标签失败时退回文件名。
#[allow(dead_code)]
pub fn scan_folder(root: &Path, out: &mut Vec<Song>) {
    for entry in WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_audio(path) {
            continue;
        }
        if let Some(song) = read_song(path) {
            out.push(song);
        }
    }
}

/// 增量扫描：文件签名（大小 + mtime）与旧缓存一致的，直接复用旧 Song 的标签，
/// 只有新增或改动过的文件才调用 lofty 读标签。曲库越大、改动越少，提速越明显。
/// 同时会剔除已经从磁盘上删除的文件（不在 found 集合里就不收进结果）。
pub fn scan_folders_incremental(roots: &[PathBuf], old: &[Song]) -> Vec<Song> {
    let old_by_path: HashMap<&Path, &Song> = old.iter().map(|s| (s.path.as_path(), s)).collect();
    let mut out = Vec::new();
    for root in roots {
        for entry in WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !is_audio(path) {
                continue;
            }
            match FileSig::of(path) {
                Some(sig) => {
                    if let Some(prev) = old_by_path.get(path) {
                        if prev.sig == sig {
                            // 文件没变，整条元数据复用（连签名一起带上）
                            out.push((*prev).clone());
                            continue;
                        }
                    }
                    if let Some(mut song) = read_song(path) {
                        song.sig = sig;
                        out.push(song);
                    }
                }
                None => {
                    // metadata 拿不到就用 read_song 里现算的签名
                    if let Some(song) = read_song(path) {
                        out.push(song);
                    }
                }
            }
        }
    }
    out
}

// ---------------- 播放列表缓存（秒开） ----------------

#[derive(Default, Serialize, Deserialize)]
pub struct LibraryCache {
    #[serde(default)]
    pub version: u32,
    /// 缓存时的文件夹列表；变化了就强制全量重扫
    #[serde(default)]
    pub folders: Vec<PathBuf>,
    #[serde(default)]
    pub songs: Vec<Song>,
}

pub const CACHE_VERSION: u32 = 1;

pub fn cache_path() -> PathBuf {
    crate::config::Config::path()
        .parent()
        .map(|p| p.join("library.json"))
        .unwrap_or_else(|| PathBuf::from("library.json"))
}

pub fn load_cache() -> Option<LibraryCache> {
    let s = std::fs::read_to_string(cache_path()).ok()?;
    let c: LibraryCache = serde_json::from_str(&s).ok()?;
    if c.version != CACHE_VERSION {
        return None;
    }
    Some(c)
}

pub fn save_cache(c: &LibraryCache) {
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(c) {
        // 先写临时文件再 rename，避免中途断电写出半个 JSON 让下次秒开失败
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

pub fn read_song(path: &Path) -> Option<Song> {
    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未知")
        .to_string();

    // 文件名常见形式： "01. 艺术家 - 标题" / "标题 - 艺术家"
    let (mut fn_artist, mut fn_title) = (String::new(), file_stem.clone());
    if let Some((a, t)) = file_stem.split_once(" - ") {
        // 去掉开头的音轨号
        let a = a.trim().trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', '-', ' ', '_'])
            .trim();
        fn_artist = a.to_string();
        fn_title = t.trim().to_string();
    }

    let mut title = String::new();
    let mut artist = String::new();
    let mut album = String::new();
    let mut duration_ms = 0u64;

    if let Ok(tagged) = lofty::read_from_path(path) {
        duration_ms = tagged.properties().duration().as_millis() as u64;
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
        if let Some(tag) = tag {
            title = tag.title().map(|s| s.to_string()).unwrap_or_default();
            artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
            album = tag.album().map(|s| s.to_string()).unwrap_or_default();
        }
    }

    if title.trim().is_empty() {
        title = if fn_title.trim().is_empty() {
            file_stem
        } else {
            fn_title
        };
    }
    if artist.trim().is_empty() {
        artist = fn_artist;
    }

    Some(Song {
        path: path.to_path_buf(),
        title: title.trim().to_string(),
        artist: artist.trim().to_string(),
        album: album.trim().to_string(),
        duration_ms,
        sig: FileSig::of(path).unwrap_or_default(),
    })
}

/// 读取内嵌歌词（ID3 USLT / Vorbis LYRICS 等）
pub fn embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let text = tag.get_string(ItemKey::Lyrics)?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_wav() -> Vec<u8> {
        // 44 字节头 + 极少 PCM，够 lofty 解析出时长即可
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36u32 + 8).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&8000u32.to_le_bytes());
        b.extend_from_slice(&16000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 8]);
        b
    }

    #[test]
    fn cache_roundtrip_keeps_signatures() {
        let sig = FileSig {
            size: 12345,
            mtime: 1700000000,
        };
        let cache = LibraryCache {
            version: CACHE_VERSION,
            folders: vec![PathBuf::from("/music")],
            songs: vec![Song {
                path: PathBuf::from("/music/a.mp3"),
                title: "A".into(),
                artist: "B".into(),
                album: "C".into(),
                duration_ms: 1000,
                sig,
            }],
        };
        let s = serde_json::to_string(&cache).unwrap();
        let back: LibraryCache = serde_json::from_str(&s).unwrap();
        assert_eq!(back.songs.len(), 1);
        assert_eq!(back.songs[0].sig, sig);
        assert_eq!(back.folders, vec![PathBuf::from("/music")]);
    }

    #[test]
    fn incremental_reuses_unchanged_and_drops_deleted() {
        let dir = std::env::temp_dir().join(format!("hwlib-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.wav");
        std::fs::write(&a, tiny_wav()).unwrap();

        // 第一次扫描：全空缓存 → 收到 a
        let first = scan_folders_incremental(&[dir.clone()], &[]);
        assert_eq!(first.len(), 1, "应扫到 a.wav");
        let sig_a = first[0].sig;
        assert!(sig_a.size > 0, "签名应带文件大小");

        // 第二次：文件没变 → 复用（签名一致，仍只有 a）
        let second = scan_folders_incremental(&[dir.clone()], &first);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].sig, sig_a);

        // 删掉 a → 再扫应为空（已删除的文件被剔除）
        std::fs::remove_file(&a).unwrap();
        let third = scan_folders_incremental(&[dir.clone()], &second);
        assert!(third.is_empty(), "文件删除后不应再出现在结果里");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
