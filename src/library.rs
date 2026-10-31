use lofty::prelude::*;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "wma", "ape", "alac", "dsf", "dff", "aiff",
    "aif", "mka", "webm",
];

#[derive(Clone, Debug)]
pub struct Song {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
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
