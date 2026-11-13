//! 等响度（ReplayGain）：移植自 BYD-AplayerFoliaMix 的
//! `LoudnessAnalyzer.kt` / `LoudnessGainStore.kt` / `ReplayGainController.kt` / `ReplayGainAudioProcessor.kt`。
//!
//! 链路：
//! 1. **测**：把音频解码成单声道 PCM → K 加权（38Hz 高通 + 1.5kHz +4dB 高架）→
//!    按 400ms 窗口算 RMS → 取 95 分位当作整曲代表响度 → gain = 目标 - 实测，夹在 ±20dB。
//! 2. **存**：结果按「路径 + 文件大小 + 修改时间」缓存到 `loudness.json`，文件变了自动重测。
//! 3. **用**：优先用文件里的 ReplayGain 标签；没有就用测出来的；再加上 preamp，
//!    开启峰值保护时用 `min(gain, 1/peak)` 防止削波。最后通过 mpv 的 `af`（volume 滤镜）生效。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::library::Song;
use crate::mpv::Mpv;

/// 目标响度：K 加权 RMS 的 dBFS。−18 dB 是 mp3gain/ReplayGain 社区的常用落点。
pub const DEFAULT_TARGET_DB: f32 = -18.0;
const WINDOW_MS: f32 = 400.0;
const PERCENTILE: f32 = 0.95;
const GAIN_MIN_DB: f32 = -20.0;
const GAIN_MAX_DB: f32 = 20.0;

/// 「只衰减」时的增益上限：−1 dBFS，给下游（EQ / 淡入淡出 / 系统混音）留余量。
/// 对应线性 10^(−1/20) ≈ 0.891。上游 BYD-AplayerFoliaMix 在
/// 「等响度安全增益」修复里用的就是这个值，理由同样是 0 dBFS 已经有削波风险。
pub const HEADROOM_DB: f32 = -1.0;

// ------------------------------------------------------------------ 双二阶滤波

#[derive(Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn new(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// RBJ cookbook 高通（a0 归一化到 1）
    pub fn high_pass(fs: f32, f0: f32, q: f32) -> Self {
        let w0 = std::f32::consts::TAU * f0 / fs;
        let (cw, sw) = (w0.cos(), w0.sin());
        let alpha = sw / (2.0 * q);
        let b0 = (1.0 + cw) / 2.0;
        let b1 = -(1.0 + cw);
        let b2 = (1.0 + cw) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cw;
        let a2 = 1.0 - alpha;
        Self::new(b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
    }

    /// RBJ cookbook 高架（a0 归一化到 1）
    pub fn high_shelf(fs: f32, f0: f32, gain_db: f32, q: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * f0 / fs;
        let (cw, sw) = (w0.cos(), w0.sin());
        let alpha = sw / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * cw + sa);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cw - sa);
        let a0 = (a + 1.0) - (a - 1.0) * cw + sa;
        // 注意：RBJ 高架的 a1 是 2*((A-1) - (A+1)cos)，不能取负。
        // （folia 的 Kotlin 版这里写成了负号，等于把高架滤波器接反了；
        //  取正后 A=1dB 时 b≡a，滤波退化为直通，可以做自检。）
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
        let a2 = (a + 1.0) - (a - 1.0) * cw - sa;
        Self::new(b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

// ------------------------------------------------------------------ 测量

pub struct Report {
    pub gain_db: f32,
    pub peak: f32,
}

/// 对一段单声道浮点采样做测量（幅度范围 ±1）
pub fn analyze_samples(samples: &[f32], sample_rate: u32, target_db: f32) -> Option<Report> {
    if samples.is_empty() || sample_rate == 0 {
        return None;
    }
    let fs = sample_rate as f32;
    let mut hp = Biquad::high_pass(fs, 38.0, 0.5);
    let mut shelf = Biquad::high_shelf(fs, 1500.0, 4.0, 0.707);
    let window = ((fs * WINDOW_MS / 1000.0) as usize).max(1);

    let mut rms: Vec<f32> = Vec::with_capacity(samples.len() / window + 1);
    let mut sum_sq = 0.0f64;
    let mut n = 0usize;
    let mut peak = 0.0f32;

    for &s in samples {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        let y = shelf.process(hp.process(s));
        sum_sq += (y as f64) * (y as f64);
        n += 1;
        if n >= window {
            rms.push(((sum_sq / window as f64).sqrt()) as f32);
            sum_sq = 0.0;
            n = 0;
        }
    }
    if n > 0 && sum_sq > 0.0 {
        rms.push(((sum_sq / n as f64).sqrt()) as f32);
    }
    if rms.is_empty() {
        return None;
    }

    rms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (((rms.len() - 1) as f32 * PERCENTILE).round() as usize).min(rms.len() - 1);
    let rep = rms[idx];
    let gain_db = if rep <= 0.0 {
        0.0
    } else {
        let measured = 20.0 * rep.log10();
        (target_db - measured).clamp(GAIN_MIN_DB, GAIN_MAX_DB)
    };
    Some(Report { gain_db, peak })
}

/// 解码整个文件并测量。用一个独立的 mpv 实例把音频喷成 16bit WAV，
/// 这样格式覆盖面和播放器完全一致（ape/dsf/wv 这些都在内）。
pub fn analyze_file(path: &Path, max_sec: u32, target_db: f32) -> Option<Report> {
    let tmp = tmp_wav_path();
    analyze_with_mpv(path, &tmp, max_sec, target_db)
}

fn tmp_wav_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    let n = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    p.push(format!("rust-music-loud-{}-{}.wav", std::process::id(), n));
    p
}

fn analyze_with_mpv(path: &Path, tmp: &Path, max_sec: u32, target_db: f32) -> Option<Report> {
    let end = format!("{}", max_sec.max(5));
    let o = tmp.to_string_lossy().into_owned();
    // 音量必须是 100：这个属性会进音频链，设成 0 会让解出来全是静音
    let mpv = Mpv::with_options(
        100.0,
        &[
            ("o", &o),
            ("no-video", "yes"),
            ("audio-samplerate", "48000"),
            ("audio-channels", "mono"),
            ("end", end.as_str()),
            ("untimed", "yes"),
            ("really-quiet", "yes"),
        ],
    )
    .ok()?;

    let src = path.to_string_lossy().into_owned();
    mpv.load(&src, true);

    // 等它把文件编码完：等 END_FILE/SHUTDOWN 事件，同时兜底看输出文件是否不再长大
    // （encode 模式下核心播完会直接退出，`idle-active` 指望不上）。
    let begin = std::time::Instant::now();
    let mut last_len = 0u64;
    let mut stable = 0;
    while begin.elapsed().as_secs() < 240 {
        if mpv.wait_end_file(0.25) {
            break;
        }
        let len = std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0);
        if len > 44 && len == last_len {
            stable += 1;
            if stable >= 4 {
                break;
            }
        } else {
            stable = 0;
        }
        last_len = len;
    }
    drop(mpv);

    let bytes = std::fs::read(tmp).ok();
    if std::env::var("RM_KEEP_WAV").is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    let (samples, rate, true_peak) = parse_wav(&bytes?)?;
    let mut rep = analyze_samples(&samples, rate, target_db)?;
    // 下混可能相位抵消，真实峰值以逐声道为准
    if true_peak.is_finite() && true_peak > rep.peak {
        rep.peak = true_peak;
    }
    Some(rep)
}

/// 极简 WAV 解析：够用即可（mpv 的 wav 输出是标准 RIFF，pcm_s16le 为主）。
/// 返回 (单声道下混采样, 采样率, 真实峰值)。
/// 峰值必须按**每个声道**取绝对值最大：下混是各声道求和再平均，
/// 反相内容会互相抵消，用下混信号算峰值会低估，峰值保护就会放行削波。
fn parse_wav(b: &[u8]) -> Option<(Vec<f32>, u32, f32)> {
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut rate = 0u32;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut float = false;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let sz = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let body = pos + 8;
        if body + sz > b.len() {
            break;
        }
        match id {
            b"fmt " => {
                if sz >= 16 {
                    channels = u16::from_le_bytes([b[body + 2], b[body + 3]]).max(1);
                    rate = u32::from_le_bytes([b[body + 4], b[body + 5], b[body + 6], b[body + 7]]);
                    bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
                    let fmt = u16::from_le_bytes([b[body], b[body + 1]]);
                    float = fmt == 3;
                }
            }
            b"data" => data = Some(&b[body..body + sz]),
            _ => {}
        }
        pos = body + sz + (sz & 1);
    }
    let data = data?;
    if rate == 0 {
        return None;
    }
    let ch = channels as usize;
    let mut peak = 0.0f32;
    let mut out = Vec::with_capacity(data.len() / 2 / ch);
    match (bits, float) {
        (16, _) => {
            for frame in data.chunks_exact(2 * ch) {
                let mut acc = 0i32;
                for c in 0..ch {
                    let i = c * 2;
                    let v = i16::from_le_bytes([frame[i], frame[i + 1]]) as i32;
                    peak = peak.max((v as f32 / 32768.0).abs());
                    acc += v;
                }
                out.push(acc as f32 / ch as f32 / 32768.0);
            }
        }
        (24, _) => {
            for frame in data.chunks_exact(3 * ch) {
                let mut acc = 0i32;
                for c in 0..ch {
                    let i = c * 3;
                    let v = ((frame[i + 2] as i32) << 16)
                        | ((frame[i + 1] as i32) << 8)
                        | frame[i] as i32;
                    peak = peak.max((v as f32 / 8_388_608.0).abs());
                    acc += (v << 8) >> 8;
                }
                out.push(acc as f32 / ch as f32 / 8_388_608.0);
            }
        }
        (32, true) => {
            for frame in data.chunks_exact(4 * ch) {
                let mut acc = 0f32;
                for c in 0..ch {
                    let i = c * 4;
                    let v = f32::from_le_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]);
                    if v.is_finite() {
                        peak = peak.max(v.abs());
                    }
                    acc += v;
                }
                out.push(acc / ch as f32);
            }
        }
        _ => return None,
    }
    Some((out, rate, peak))
}

// ------------------------------------------------------------------ 标签

#[derive(Default, Clone, Copy)]
pub struct Tags {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

fn parse_db(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .trim()
        .parse::<f32>()
        .ok()
}

pub fn read_tags(path: &Path) -> Tags {
    use lofty::prelude::{ItemKey, TaggedFileExt};
    let mut t = Tags::default();
    let Ok(f) = lofty::read_from_path(path) else {
        return t;
    };
    for tag in f.tags() {
        for item in tag.items() {
            let k = item.key();
            let Some(v) = item.value().text() else { continue };
            let v = v.to_string();
            match k {
                ItemKey::ReplayGainTrackGain => {
                    t.track_gain = t.track_gain.or(parse_db(&v));
                }
                ItemKey::ReplayGainTrackPeak => {
                    t.track_peak = t.track_peak.or(parse_db(&v));
                }
                ItemKey::ReplayGainAlbumGain => {
                    t.album_gain = t.album_gain.or(parse_db(&v));
                }
                ItemKey::ReplayGainAlbumPeak => {
                    t.album_peak = t.album_peak.or(parse_db(&v));
                }
                _ => {}
            }
        }
    }
    t
}

// ------------------------------------------------------------------ 缓存

/// 缓存结构版本。2 = 峰值改为「逐声道真实峰值」+ 增益改成只衰减语义。
/// 老条目里的 peak 是单声道下混算出来的，会低估真实峰值，必须重测。
pub const CACHE_VERSION: u8 = 2;

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct Entry {
    pub gain_db: f32,
    pub peak: f32,
    pub size: u64,
    pub mtime: i64,
    #[serde(default)]
    pub ver: u8,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Store {
    #[serde(default)]
    pub items: HashMap<String, Entry>,
}

impl Store {
    pub fn path() -> PathBuf {
        crate::config::Config::path()
            .parent()
            .map(|p| p.join("loudness.json"))
            .unwrap_or_else(|| PathBuf::from("loudness.json"))
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(p, s);
        }
    }

    /// 命中且文件没变才算数
    pub fn get_valid(&self, song: &Song) -> Option<&Entry> {
        let key = song.path.to_string_lossy().into_owned();
        let e = self.items.get(&key)?;
        let (size, mtime) = file_sig(&song.path)?;
        if e.size == size && e.mtime == mtime && e.ver == CACHE_VERSION {
            Some(e)
        } else {
            None
        }
    }

    pub fn put(&mut self, path: &Path, e: Entry) {
        self.items.insert(path.to_string_lossy().into_owned(), e);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

pub fn file_sig(path: &Path) -> Option<(u64, i64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((md.len(), mtime))
}

// ------------------------------------------------------------------ 决策

/// 结算最终要施加的增益（dB）。返回 None 表示不做处理。
///
/// 顺序和 folia 一致：ReplayGain 标签 → 本地测量（可现场测）→ 缺省增益。
pub fn resolve_gain(cfg: &crate::config::LoudnessCfg, song: &Song, store: &Store) -> Option<f32> {
    if !cfg.enabled {
        return None;
    }
    let tags = read_tags(&song.path);
    let has_tag = tags.track_gain.is_some() || tags.album_gain.is_some();
    let (gain, peak) = if has_tag {
        if cfg.album_mode {
            (
                tags.album_gain.or(tags.track_gain),
                tags.album_peak.or(tags.track_peak),
            )
        } else {
            match tags.track_gain {
                Some(g) => (Some(g), tags.track_peak),
                None => (tags.album_gain, tags.album_peak),
            }
        }
    } else if let Some(e) = store.get_valid(song) {
        (Some(e.gain_db), Some(e.peak))
    } else if cfg.auto_analyze {
        // 还没测过：先按缺省增益播，后台测完再补上（由调用方处理）
        (None, None)
    } else {
        (None, None)
    };

    let Some(gain) = gain else {
        // 还没测过、也没有标签：走缺省增益，但同样要过安全钳制
        return Some(clamp_gain(cfg, cfg.missing_gain_db, None));
    };
    Some(clamp_gain(cfg, gain + cfg.preamp_db, peak))
}

/// mp3gain 式安全钳制（对齐上游 `a1689638` 「等响度安全增益」）：
///  a) 默认只衰减不放大，上限 −1 dBFS，给下游 EQ / 淡入淡出 / 系统混音留余量；
///  b) 峰值缺失或无效时，绝不把「不知道峰值」当成「可以放大」的许可；
///  c) 只有用户显式打开 `allow_boost`，才恢复完整 ReplayGain 对齐（接受底噪与削波风险）。
fn clamp_gain(cfg: &crate::config::LoudnessCfg, db: f32, peak: Option<f32>) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    let mut ceiling_db = if cfg.allow_boost {
        f32::INFINITY
    } else {
        HEADROOM_DB
    };
    if cfg.peak_protection {
        match peak.filter(|p| p.is_finite() && *p > 0.0) {
            // 用峰值把增益压到不削波（等价于 min(linear, 1/peak)）
            Some(p) => ceiling_db = ceiling_db.min(-20.0 * p.log10()),
            // 没有峰值数据：最多到 0 dBFS，不放大
            None => ceiling_db = ceiling_db.min(0.0),
        }
    }
    db.min(ceiling_db).clamp(GAIN_MIN_DB, GAIN_MAX_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, secs: f32, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                amp * (std::f32::consts::TAU * freq * t).sin()
            })
            .collect()
    }

    #[test]
    fn quieter_signal_gets_more_gain() {
        let loud = sine(1000.0, 0.5, 3.0, 48000);
        let quiet = sine(1000.0, 0.05, 3.0, 48000);
        let a = analyze_samples(&loud, 48000, DEFAULT_TARGET_DB).unwrap();
        let b = analyze_samples(&quiet, 48000, DEFAULT_TARGET_DB).unwrap();
        assert!(a.gain_db < b.gain_db, "{} vs {}", a.gain_db, b.gain_db);
        // 幅度差 20 倍 ≈ 26dB，响亮的那首该被压，轻的那首该被抬
        assert!(a.gain_db < 0.0, "0.5 幅度不该再抬：{}", a.gain_db);
        assert!(b.gain_db > 0.0, "0.05 幅度该被抬：{}", b.gain_db);
        assert!((a.peak - 0.5).abs() < 0.01);
    }

    #[test]
    fn shelf_is_flat_at_0db() {
        // A=1（0dB）时 b 应等于 a，滤波直通
        let f = Biquad::high_shelf(48000.0, 1500.0, 0.0, 0.707);
        let eps = 1e-6;
        assert!((f.b0 - 1.0).abs() < eps, "b0={}", f.b0);
        assert!((f.b1 - f.a1).abs() < eps, "b1={} a1={}", f.b1, f.a1);
        assert!((f.b2 - f.a2).abs() < eps, "b2={} a2={}", f.b2, f.a2);
    }

    #[test]
    fn silence_is_neutral() {
        let s = vec![0.0f32; 48000];
        let r = analyze_samples(&s, 48000, DEFAULT_TARGET_DB).unwrap();
        assert_eq!(r.gain_db, 0.0);
    }

    #[test]
    fn gain_is_clamped() {
        let tiny = sine(1000.0, 0.001, 2.0, 48000);
        let r = analyze_samples(&tiny, 48000, DEFAULT_TARGET_DB).unwrap();
        assert_eq!(r.gain_db, GAIN_MAX_DB);
    }

    #[test]
    fn k_weighting_boosts_2k_more_than_100() {
        // 高架滤波在 1.5k 以上有 +4dB，100Hz 附近基本平坦
        let low = sine(100.0, 0.1, 2.0, 48000);
        let high = sine(4000.0, 0.1, 2.0, 48000);
        let a = analyze_samples(&low, 48000, DEFAULT_TARGET_DB).unwrap();
        let b = analyze_samples(&high, 48000, DEFAULT_TARGET_DB).unwrap();
        assert!(b.gain_db < a.gain_db, "4k 被加权抬高后需要更少增益");
    }

    #[test]
    fn parse_db_handles_suffixes() {
        assert_eq!(parse_db("-7.35 dB"), Some(-7.35));
        assert_eq!(parse_db("+2.10dB"), Some(2.1));
        assert_eq!(parse_db("0.98"), Some(0.98));
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use std::path::Path;

    /// 真实解码路径（需要本机有 libmpv）：
    /// cargo test loud_analyze_real_file -- --ignored --nocapture
    #[test]
    #[ignore]
    fn loud_analyze_real_file() {
        let p = Path::new("/tmp/test1k.wav");
        if !p.exists() {
            eprintln!("跳过了：{} 不存在", p.display());
            return;
        }
        let r = analyze_file(p, 90, DEFAULT_TARGET_DB).expect("应该能测出结果");
        eprintln!(
            "gain={:+.2} dB  peak={:.3}",
            r.gain_db, r.peak
        );
        // 0.1 幅度 1kHz 正弦：RMS≈-23dBFS，K 加权后约 -19dBFS，目标 -18 → 增益应在 0 附近小幅
        assert!(r.gain_db > -8.0 && r.gain_db < 8.0, "增益异常：{}", r.gain_db);
        // 单声道下混有增益，峰值会比源幅度(0.1)高一些，这里给宽一点
        assert!(r.peak > 0.05 && r.peak < 0.25, "峰值异常：{}", r.peak);
    }

    /// af 链真的被 mpv 接受了（语法 `af add volume=volume=XdB`）
    #[test]
    #[ignore]
    fn gain_filter_is_accepted_by_mpv() {
        let m = Mpv::new(100.0).expect("需要本机 libmpv");
        // 正负增益都要能被 mpv 接受（rc==0），且清得掉
        m.set_gain_db(Some(6.0));
        eprintln!("af(+6) -> {:?}", m.get_string("af"));
        m.set_gain_db(Some(-4.5));
        eprintln!("af(-4.5) -> {:?}", m.get_string("af"));
        let chain = m.get_string("af").unwrap_or_default();
        assert!(chain.contains("volume"), "增益没进 af 链：{:?}", chain);
        m.set_gain_db(None);
        let chain = m.get_string("af").unwrap_or_default();
        assert!(
            chain.is_empty() || chain == "(empty)",
            "af 没清掉：{:?}",
            chain
        );
        // 0dB 不该挂滤镜
        m.set_gain_db(Some(0.0));
        let chain = m.get_string("af").unwrap_or_default();
        assert!(chain.is_empty() || chain == "(empty)", "0dB 还挂了滤镜：{:?}", chain);
    }

    /// gain 解析 + 峰值保护
    #[test]
    fn resolve_applies_preamp_and_peak_limit() {
        let mut cfg = crate::config::LoudnessCfg::default();
        cfg.enabled = true;
        cfg.preamp_db = 3.0;
        cfg.peak_protection = true;
        let song = song_with_store_gain(-6.0, 0.9);
        let g = resolve_gain(&cfg, &song.0, &song.1).unwrap();
        // -6 + 3 = -3dB，峰值 0.9 的上限是 -20log10(0.9) ≈ +0.92dB → 不触发限制
        assert!((g - (-3.0)).abs() < 0.01, "{}", g);

        let song = song_with_store_gain(-1.0, 0.9);
        let g = resolve_gain(&cfg, &song.0, &song.1).unwrap();
        // +2dB 会被峰值压到 ≈0.92dB
        assert!(g < 1.0, "峰值保护没生效：{}", g);
    }

    #[test]
    fn default_gain_only_attenuates_with_headroom() {
        let cfg = crate::config::LoudnessCfg::default();
        // 响的照压（峰值上限只封顶正向增益，不会把 -8dB 再往下拽）
        assert_eq!(clamp_gain(&cfg, -8.0, Some(0.9)), -8.0);
        assert_eq!(clamp_gain(&cfg, 6.0, Some(1.0)), HEADROOM_DB);
        // 安静的：默认不抬，且统一留 1dB 余量
        assert_eq!(clamp_gain(&cfg, 6.0, Some(0.05)), HEADROOM_DB);
        // 峰值缺失更不是放大的理由
        assert_eq!(clamp_gain(&cfg, 6.0, None), HEADROOM_DB);
        // 关掉峰值保护也一样不会变成放大
        let mut no_peak_prot = cfg.clone();
        no_peak_prot.peak_protection = false;
        assert_eq!(clamp_gain(&no_peak_prot, 5.0, None), HEADROOM_DB);
    }

    #[test]
    fn allow_boost_restores_full_replaygain() {
        let mut cfg = crate::config::LoudnessCfg::default();
        cfg.allow_boost = true;
        // 峰值 0.9 → 上限 ≈ +0.92dB，抬不到 +12
        let g = clamp_gain(&cfg, 12.0, Some(0.9));
        assert!(g < 1.0 && g > 0.8, "峰值保护应生效: {g}");
        // 峰值 0.05 有 26dB 余量，+12dB 可以放行
        assert_eq!(clamp_gain(&cfg, 12.0, Some(0.05)), 12.0);
        // 开了正向放大但完全没有峰值数据：仍然不放大
        assert_eq!(clamp_gain(&cfg, 12.0, None), 0.0);
    }

    #[test]
    fn out_of_phase_stereo_peak_is_per_channel() {
        // 左右反相且幅度相同：单声道下混几乎为零，但真实峰值是 0.8。
        // 旧实现用下混算峰值 → peak≈0 → 1/peak 无穷大，峰值保护形同虚设。
        let mut data = Vec::new();
        for i in 0..2000 {
            let v: i16 = if i % 2 == 0 { 26214 } else { -26214 };
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&(-v).to_le_bytes());
        }
        let wav = tiny_wav(&data, 2, 16, 48000);
        let (samples, rate, peak) = parse_wav(&wav).expect("应能解析");
        assert_eq!(rate, 48000);
        assert!(samples.iter().all(|s| s.abs() < 0.01), "下混应抵消");
        assert!((peak - 0.8).abs() < 0.01, "峰值必须按声道取: {peak}");
    }

    /// 造一个最小可解析的 PCM WAV
    fn tiny_wav(data: &[u8], channels: u16, bits: u16, rate: u32) -> Vec<u8> {
        let block = channels * bits / 8;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * block as u32).to_le_bytes());
        b.extend_from_slice(&block.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    /// 造一个真实存在的临时 WAV + 与之签名匹配的缓存条目。
    /// 以前依赖 /tmp/test1k.wav，机器上没有这个文件时用例会静默走「无数据」分支，
    /// 变成永远测不到峰值保护，所以在测试里自己生成。
    fn song_with_store_gain(gain: f32, peak: f32) -> (Song, Store) {
        let mut data = Vec::new();
        for i in 0..4800 {
            let v: i16 = if i % 2 == 0 { 26214 } else { -26214 };
            data.extend_from_slice(&v.to_le_bytes());
        }
        let wav = tiny_wav(&data, 1, 16, 48000);
        let real = std::env::temp_dir().join(format!(
            "hwplayer-loud-test-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&real, &wav).expect("写测试 WAV");
        let (size, mtime) = file_sig(&real).expect("文件刚写完，签名必须拿得到");

        let song = Song {
            path: real.clone(),
            title: "t".into(),
            artist: "a".into(),
            album: "b".into(),
            duration_ms: 1000,
        };
        let mut store = Store::default();
        store.put(
            &real,
            Entry {
                gain_db: gain,
                peak,
                size,
                mtime,
                ver: CACHE_VERSION,
            },
        );
        (song, store)
    }
}

