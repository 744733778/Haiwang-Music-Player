//! 一键清除本程序在这台电脑上留下的所有数据与缓存。
//!
//! 背景：程序改过名字（rust-music → hwplayer），`Config::dir()` 为了兼容旧版，
//! 只要新目录没有 config.json 就一直沿用 `~/.config/rust-music/`。换机、删过旧
//! AppImage、配置写坏之后，残留的旧配置/曲库缓存会让程序怎么选文件夹都读不到歌。
//! 这里提供彻底的兜底：把程序自己的数据目录全部删掉，回到全新状态。
//!
//! 安全边界：**只删程序专属目录，绝不碰用户的音乐文件、歌词文件，也不卸载程序本体**
//! （deb 安装的本体请用 `apt remove hwplayer`，AppImage 直接删那个文件即可）。

use std::path::PathBuf;

/// 本程序用过的所有目录名：当前名 + 历史名（兼容旧版残留）。
const DATA_NAMES: [&str; 2] = ["hwplayer", "rust-music"];

/// 返回本程序可能写入数据的所有目录（配置 / 缓存 / 本地数据）。
/// 只返回路径、不执行删除，方便在设置界面展示"将要清除什么"，也便于测试。
pub fn data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(base) = dirs::config_dir() {
        for n in DATA_NAMES {
            out.push(base.join(n));
        }
    }
    if let Some(base) = dirs::cache_dir() {
        for n in DATA_NAMES {
            out.push(base.join(n));
        }
    }
    if let Some(base) = dirs::data_local_dir() {
        for n in DATA_NAMES {
            out.push(base.join(n));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 实际存在于磁盘上的程序数据目录。
pub fn existing_data_dirs() -> Vec<PathBuf> {
    data_dirs()
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

/// 删除结果：成功删除的路径 + 失败的路径及原因。
#[derive(Debug, Default)]
pub struct PurgeResult {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// 删除给定的路径（目录递归删、文件直接删）。不存在则跳过，不视为失败。
pub fn purge_paths(paths: &[PathBuf]) -> PurgeResult {
    let mut res = PurgeResult::default();
    for p in paths {
        if !p.exists() {
            continue;
        }
        let r = if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        match r {
            Ok(()) => res.removed.push(p.clone()),
            Err(e) => res.failed.push((p.clone(), e.to_string())),
        }
    }
    res
}

/// 清除本程序所有数据目录。
pub fn purge_all() -> PurgeResult {
    purge_paths(&data_dirs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录清单必须同时覆盖新名和旧名，否则换机残留的旧配置清不掉。
    #[test]
    fn data_dirs_cover_both_names() {
        let d = data_dirs();
        assert!(
            d.iter().any(|p| p.file_name().and_then(|s| s.to_str()) == Some("hwplayer")),
            "必须包含 hwplayer 目录"
        );
        assert!(
            d.iter().any(|p| p.file_name().and_then(|s| s.to_str()) == Some("rust-music")),
            "必须包含旧版 rust-music 目录"
        );
    }

    /// 只删指定路径；无关目录（模拟用户的音乐文件夹）必须原封不动。
    #[test]
    fn purge_removes_only_given_paths() {
        let base = std::env::temp_dir().join(format!("hwcleanup-{}", std::process::id()));
        let app = base.join("hwplayer");
        let music = base.join("我的音乐");
        let _ = std::fs::create_dir_all(&app);
        let _ = std::fs::create_dir_all(&music);
        std::fs::write(app.join("config.json"), "{}").unwrap();
        std::fs::write(app.join("library.json"), "{}").unwrap();
        std::fs::write(music.join("song.mp3"), b"id3").unwrap();

        let r = purge_paths(&[app.clone()]);
        assert_eq!(r.removed, vec![app.clone()]);
        assert!(r.failed.is_empty());
        assert!(!app.exists(), "程序数据目录应被删除");
        assert!(music.join("song.mp3").exists(), "用户音乐文件绝不能被删除");

        let _ = std::fs::remove_dir_all(&base);
    }
}
