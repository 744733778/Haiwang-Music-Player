use crate::lyrics_online;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// 容器级 default：旧版配置里缺的新字段（字号/颜色/背景不透明度…）逐字段回退到
// LyricWin::default()，而不是让整份 config.json 反序列化失败把音乐文件夹一起丢掉。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricWin {
    /// 外层窗口位置（屏幕坐标，点）
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// 歌词字号（主窗口与桌面歌词共用基准，桌面歌词按此值渲染）
    pub font_size: f32,
    /// 锁定后鼠标穿透，不能拖动
    pub locked: bool,
    pub visible: bool,
    pub show_translation: bool,
    pub show_next: bool,
    pub sung: [u8; 3],
    pub unsung: [u8; 3],
    pub translation: [u8; 3],
    /// 背景不透明度 0-255（0 = 完全透明）
    pub background_alpha: u8,
}

impl Default for LyricWin {
    fn default() -> Self {
        Self {
            x: 240.0,
            y: 140.0,
            width: 940.0,
            height: 190.0,
            font_size: 36.0,
            locked: false,
            visible: true,
            show_translation: true,
            show_next: true,
            sung: [0x3b, 0x82, 0xf6],
            unsung: [0xf2, 0xf2, 0xf2],
            translation: [0xc8, 0xc8, 0xc8],
            background_alpha: 0,
        }
    }
}

/// 等响度（ReplayGain）配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessCfg {
    pub enabled: bool,
    /// 优先用专辑增益（整张碟统一音量），否则用单曲增益
    pub album_mode: bool,
    /// 预增益（dB）：整体抬/压一点
    pub preamp_db: f32,
    /// 峰值保护：用测量到的峰值把增益压到不削波
    pub peak_protection: bool,
    /// 允许正向放大（默认关）。
    /// 关：mp3gain 式「只衰减、不放大」，并且整体留 1 dB 余量给下游（EQ/淡入淡出）。
    /// 开：恢复成完整 ReplayGain 对齐，安静曲目会被抬到目标响度，但底噪同步抬高、
    /// 遇到峰值数据不准时容易削波，是否接受由用户自己决定。
    pub allow_boost: bool,
    /// 没有标签、也没测过时用的缺省增益
    pub missing_gain_db: f32,
    /// 没有 ReplayGain 标签时自动现场测
    pub auto_analyze: bool,
    /// 目标响度（K 加权 RMS 的 dBFS）
    pub target_db: f32,
    /// 单曲最多解码多少秒来测量
    pub max_analyze_sec: u32,
}

impl Default for LoudnessCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            album_mode: false,
            preamp_db: 0.0,
            peak_protection: true,
            allow_boost: false,
            missing_gain_db: 0.0,
            auto_analyze: true,
            target_db: crate::loudness::DEFAULT_TARGET_DB,
            max_analyze_sec: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub music_folders: Vec<PathBuf>,
    pub volume: f64,
    /// 打乱歌单顺序（不是播放时随机取，见 App::shuffle_playlist）
    pub shuffle: bool,
    pub lyric: LyricWin,
    pub lyric_offset_ms: i64,
    #[serde(default)]
    pub auto_download_lyric: bool,
    /// 歌词源优先顺序
    #[serde(default = "lyrics_online::default_order")]
    pub source_order: Vec<String>,
    /// 启动时自动重新扫描已导入的文件夹
    #[serde(default = "default_true")]
    pub rescan_on_start: bool,
    /// 等响度
    pub loudness: LoudnessCfg,
    /// 全局热键（↑↓ 音量、←→ 切歌）。X11 下用 XGrabKey 注册，窗口不在前台也生效。
    /// 注意：会独占方向键——运行期间别的程序收不到方向键，不需要时在设置里关掉。
    #[serde(default = "default_true")]
    pub hotkey_enabled: bool,
    /// 每按一次 ↑/↓ 调整多少音量
    #[serde(default = "default_volume_step")]
    pub hotkey_volume_step: f32,
    /// 配置版本：老版本缺字段时的迁移用
    #[serde(default)]
    pub version: u32,
}

fn default_true() -> bool {
    true
}

fn default_volume_step() -> f32 {
    5.0
}

/// 当前配置版本。v2：桌面歌词窗改成 Win32 分层窗口，旧版存下的
/// `background_alpha=255`（当时没得选）要重置成 0（全透明），否则升级后还是黑底。
const CONFIG_VERSION: u32 = 2;

impl Default for Config {
    fn default() -> Self {
        Self {
            music_folders: Vec::new(),
            volume: 85.0,
            shuffle: false,
            lyric: LyricWin::default(),
            lyric_offset_ms: 0,
            auto_download_lyric: false,
            source_order: lyrics_online::default_order(),
            rescan_on_start: true,
            loudness: LoudnessCfg::default(),
            hotkey_enabled: true,
            hotkey_volume_step: 5.0,
            version: CONFIG_VERSION,
        }
    }
}

impl Config {
    /// 配置目录。程序更名为 hwplayer 后默认写 ~/.config/hwplayer；
    /// 若新目录还没有配置、而旧版（rust-music）的配置存在，则继续沿用旧文件，
    /// 避免改名导致用户丢失已导入的音乐文件夹、音量等设置。
    fn dir() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let new_dir = base.join("hwplayer");
        let old_file = base.join("rust-music").join("config.json");
        if !new_dir.join("config.json").exists() && old_file.exists() {
            return base.join("rust-music");
        }
        new_dir
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.json")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => {
                let mut cfg: Self = serde_json::from_str(&s).unwrap_or_default();
                cfg.normalize();
                cfg
            }
            Err(_) => Self::default(),
        }
    }

    /// 补齐/清理可能不完整的字段（旧配置、手改配置）
    fn normalize(&mut self) {
        if self.source_order.is_empty() {
            self.source_order = lyrics_online::default_order();
        }
        // 只保留已知源，且不重复
        let known = lyrics_online::all_source_ids();
        let mut seen: Vec<String> = Vec::new();
        self.source_order.retain(|s| {
            if known.contains(&s.as_str()) && !seen.contains(s) {
                seen.push(s.clone());
                true
            } else {
                false
            }
        });
        for id in known {
            if !self.source_order.iter().any(|s| s == id) {
                self.source_order.push(id.to_string());
            }
        }
        if self.version < 2 {
            self.lyric.background_alpha = 0;
            self.version = CONFIG_VERSION;
        }
        self.lyric.font_size = self.lyric.font_size.clamp(12.0, 96.0);
        self.lyric.width = self.lyric.width.clamp(240.0, 3000.0);
        self.lyric.height = self.lyric.height.clamp(60.0, 800.0);
        if !self.hotkey_volume_step.is_finite() {
            self.hotkey_volume_step = 5.0;
        }
        self.hotkey_volume_step = self.hotkey_volume_step.clamp(1.0, 20.0);
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上一版（没有字号/颜色/背景/源顺序这些字段）的 config.json 必须还能读，
    /// 否则用户升级后音乐文件夹会丢，还得重新导入一遍。
    #[test]
    fn old_config_still_loads() {
        let old = r#"{
            "music_folders": ["D:\\Music"],
            "volume": 70.0,
            "shuffle": false,
            "lyric": {
                "x": 240.0, "y": 140.0, "width": 940.0, "height": 190.0,
                "locked": false, "visible": true,
                "show_translation": true, "show_next": true
            },
            "lyric_offset_ms": 0,
            "auto_download_lyric": false
        }"#;
        let mut cfg: Config = serde_json::from_str(old).expect("旧配置必须能解析");
        cfg.normalize();
        assert_eq!(cfg.music_folders.len(), 1);
        assert_eq!(cfg.lyric.font_size, LyricWin::default().font_size);
        assert!(cfg.rescan_on_start);
        assert_eq!(cfg.source_order, lyrics_online::default_order());
        assert_eq!(cfg.lyric.background_alpha, 0);
    }

    /// 源顺序被手改坏（未知源 / 重复 / 缺项）时要能自动修复
    #[test]
    fn normalize_repairs_source_order() {
        let mut cfg = Config::default();
        cfg.source_order = vec!["kugou".into(), "kugou".into(), "bogus".into()];
        cfg.normalize();
        assert_eq!(cfg.source_order[0], "kugou");
        assert_eq!(cfg.source_order.len(), 4);
        assert!(!cfg.source_order.iter().any(|s| s == "bogus"));
    }
}
