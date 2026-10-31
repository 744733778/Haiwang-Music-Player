use crate::config::Config;
use crate::library::{self, Song};
use crate::lyrics::{self, LyricLine};
use crate::lyrics_online;
use crate::mpv::Mpv;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// 等响度后台线程回报
enum LoudMsg {
    Done(std::path::PathBuf, crate::loudness::Entry),
    Progress(usize, usize, String),
    Finished,
}

/// 桌面歌词窗口的稳定标题（仅 egui 子 viewport 路径使用）
#[cfg(not(windows))]
pub const LYRIC_WINDOW_TITLE: &str = "rust-music-desktop-lyric";

pub struct App {
    cfg: Config,
    mpv: Option<Mpv>,
    mpv_err: Option<String>,

    songs: Vec<Song>,
    current: Option<usize>,

    playing: bool,
    position: f64,
    duration: f64,

    lyrics: Vec<LyricLine>,
    lyric_index: Option<usize>,
    last_scrolled: Option<usize>,

    folder_input: String,
    status: String,

    dl_rx: Option<Receiver<String>>,
    scan_rx: Option<Receiver<Vec<Song>>>,
    /// 等响度测量结果缓存
    loud_store: crate::loudness::Store,
    /// 响度扫描/单曲测量的回报
    loud_rx: Option<Receiver<LoudMsg>>,
    /// 当前曲目实际施加的增益（None = 未启用或未就绪）
    applied_gain: Option<f32>,

    seeking: bool,
    seek_value: f64,
    last_eof: bool,

    show_settings: bool,
    /// 桌面歌词窗口位置只在首帧设置，之后交给系统拖动，避免每帧回写导致抖动/不跟手
    #[cfg(not(windows))]
    lyric_pos_init: bool,
    /// 打乱前的原始顺序（退出随机时可恢复，和 folia 的 originalOrder 一致）
    original_order: Option<Vec<PathBuf>>,
    /// Windows：分层窗口自绘的桌面歌词（不依赖 GL 透明通道）
    #[cfg(windows)]
    overlay: crate::lyric_win32::Overlay,
    #[cfg(windows)]
    overlay_started: bool,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let (mpv, mpv_err) = match Mpv::new(cfg.volume) {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(format!("libmpv 未就绪：{}", e))),
        };
        let folder_input = cfg
            .music_folders
            .last()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut app = Self {
            cfg,
            mpv,
            mpv_err,
            songs: Vec::new(),
            current: None,
            playing: false,
            position: 0.0,
            duration: 0.0,
            lyrics: Vec::new(),
            lyric_index: None,
            last_scrolled: None,
            folder_input,
            status: "就绪。选择一个音乐文件夹开始。".to_string(),
            dl_rx: None,
            scan_rx: None,
            loud_store: crate::loudness::Store::load(),
            loud_rx: None,
            applied_gain: None,
            seeking: false,
            seek_value: 0.0,
            last_eof: false,
            show_settings: false,
            original_order: None,
            #[cfg(not(windows))]
            lyric_pos_init: false,
            #[cfg(windows)]
            overlay: crate::lyric_win32::Overlay::new(),
            #[cfg(windows)]
            overlay_started: false,
        };
        if app.cfg.rescan_on_start && !app.cfg.music_folders.is_empty() {
            app.start_scan("正在扫描上次的文件夹…");
        }
        app
    }

    // ---------------- 曲库 ----------------

    /// 后台扫描所有已导入文件夹
    fn start_scan(&mut self, msg: &str) {
        if self.scan_rx.is_some() {
            return;
        }
        let folders = self.cfg.music_folders.clone();
        if folders.is_empty() {
            self.status = "还没有导入任何文件夹。".to_string();
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut all = Vec::new();
            for f in &folders {
                library::scan_folder(f, &mut all);
            }
            sort_songs(&mut all);
            let _ = tx.send(all);
        });
        self.scan_rx = Some(rx);
        self.status = msg.to_string();
    }

    fn poll_scan(&mut self) {
        let received = match &self.scan_rx {
            Some(rx) => match rx.try_recv() {
                Ok(v) => Some(v),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan_rx = None;
                    None
                }
            },
            None => None,
        };
        if let Some(mut songs) = received {
            self.scan_rx = None;
            let cur_path = self.current.and_then(|i| self.songs.get(i).map(|s| s.path.clone()));
            if self.cfg.shuffle {
                shuffle(&mut songs);
            }
            self.songs = songs;
            self.current = cur_path.and_then(|p| self.songs.iter().position(|s| s.path == p));
            if let Some(idx) = self.current {
                let s = self.songs[idx].clone();
                self.load_lyrics_for(&s);
            }
            self.status = format!("扫描完成，曲库共 {} 首。", self.songs.len());
        }
    }

    fn import_folder(&mut self, path: PathBuf) {
        if !path.is_dir() {
            self.status = "路径不是有效文件夹。".to_string();
            return;
        }
        if !self.cfg.music_folders.contains(&path) {
            self.cfg.music_folders.push(path);
            self.cfg.save();
        }
        // 统一走后台重扫，避免大库卡界面
        self.start_scan("正在扫描文件夹…");
    }

    /// 打乱歌单顺序：直接重排列表本身，下一首就是列表里的下一首
    fn shuffle_playlist(&mut self) {
        if self.songs.len() < 2 {
            self.status = "曲目太少，不需要打乱。".to_string();
            return;
        }
        let cur_path = self.current.and_then(|i| self.songs.get(i).map(|s| s.path.clone()));
        // 只在第一次打乱前记一次，后续再打乱不会覆盖原始顺序
        if self.original_order.is_none() {
            self.original_order = Some(self.songs.iter().map(|s| s.path.clone()).collect());
        }
        shuffle(&mut self.songs);
        self.current = cur_path.and_then(|p| self.songs.iter().position(|s| s.path == p));
        let pos = self
            .current
            .map(|i| format!("（当前曲目现在第 {} 位）", i + 1))
            .unwrap_or_default();
        self.status = format!("已打乱歌单顺序{}，左侧列表就是播放顺序。", pos);
    }

    /// 恢复打乱前的原始顺序（按路径找回，期间增删的歌不受影响）
    fn restore_order(&mut self) {
        let Some(order) = self.original_order.take() else {
            self.status = "还没有打乱过歌单。".to_string();
            return;
        };
        let cur_path = self.current.and_then(|i| self.songs.get(i).map(|s| s.path.clone()));
        let mut by_path: std::collections::HashMap<PathBuf, Song> =
            self.songs.drain(..).map(|s| (s.path.clone(), s)).collect();
        for p in &order {
            if let Some(s) = by_path.remove(p) {
                self.songs.push(s);
            }
        }
        // 不在原始顺序里的（打乱后新导入的）保持排在后面
        let mut rest: Vec<Song> = by_path.into_values().collect();
        sort_songs(&mut rest);
        self.songs.extend(rest);
        self.current = cur_path.and_then(|p| self.songs.iter().position(|s| s.path == p));
        self.status = "已恢复原始顺序。".to_string();
    }

    // ---------------- 播放 ----------------

    fn play_index(&mut self, idx: usize) {
        if idx >= self.songs.len() {
            return;
        }
        self.current = Some(idx);
        let song = self.songs[idx].clone();
        // 先定增益再 load，避免开头一瞬间音量跳一下
        self.apply_gain(&song);
        if let Some(m) = &self.mpv {
            m.load(&song.path.to_string_lossy(), true);
        }
        self.duration = if song.duration_ms > 0 {
            song.duration_ms as f64 / 1000.0
        } else {
            0.0
        };
        self.position = 0.0;
        self.playing = true;
        self.last_eof = false;
        self.last_scrolled = None;
        self.load_lyrics_for(&song);
        self.status = format!("正在播放：{}", song.display());
    }

    /// 结算并施加等响度增益；没有数据且允许自动测量时，顺带起个后台测量
    fn apply_gain(&mut self, song: &Song) {
        let cfg = self.cfg.loudness.clone();
        if !cfg.enabled {
            self.applied_gain = None;
            if let Some(m) = &self.mpv {
                m.set_gain_db(None);
            }
            return;
        }
        let gain = crate::loudness::resolve_gain(&cfg, song, &self.loud_store);
        self.applied_gain = gain;
        if let Some(m) = &self.mpv {
            m.set_gain_db(gain);
        }
        // 没有标签、缓存里也没有 → 现场测一首
        let tags = crate::loudness::read_tags(&song.path);
        let has_gain = tags.track_gain.is_some() || tags.album_gain.is_some();
        if !has_gain && cfg.auto_analyze && self.loud_store.get_valid(song).is_none() {
            if self.loud_rx.is_none() {
                let (tx, rx) = mpsc::channel();
                let s2 = song.clone();
                let max_sec = cfg.max_analyze_sec;
                let target = cfg.target_db;
                std::thread::spawn(move || {
                    if let Some(rep) = crate::loudness::analyze_file(&s2.path, max_sec, target) {
                        if let Some((size, mtime)) = crate::loudness::file_sig(&s2.path) {
                            let _ = tx.send(LoudMsg::Done(
                                s2.path.clone(),
                                crate::loudness::Entry {
                                    gain_db: rep.gain_db,
                                    peak: rep.peak,
                                    size,
                                    mtime,
                                },
                            ));
                        }
                    }
                    let _ = tx.send(LoudMsg::Finished);
                });
                self.loud_rx = Some(rx);
                self.status = format!("正在测量响度：{}", song.display());
            }
        }
    }

    /// 全库扫描响度（已有有效缓存的跳过，可中断后续跑）
    fn start_loud_scan(&mut self) {
        if self.loud_rx.is_some() {
            self.status = "响度测量已在运行。".to_string();
            return;
        }
        if self.songs.is_empty() {
            return;
        }
        let songs = self.songs.clone();
        let cfg = self.cfg.loudness.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut store = crate::loudness::Store::load();
            let total = songs.len();
            for (i, s) in songs.iter().enumerate() {
                if store.get_valid(s).is_some() {
                    let _ = tx.send(LoudMsg::Progress(
                        i + 1,
                        total,
                        format!("跳过（已测过）：{}", s.display()),
                    ));
                    continue;
                }
                let _ = tx.send(LoudMsg::Progress(
                    i + 1,
                    total,
                    format!("测量中：{}", s.display()),
                ));
                match crate::loudness::analyze_file(&s.path, cfg.max_analyze_sec, cfg.target_db) {
                    Some(rep) => {
                        if let Some((size, mtime)) = crate::loudness::file_sig(&s.path) {
                            let e = crate::loudness::Entry {
                                gain_db: rep.gain_db,
                                peak: rep.peak,
                                size,
                                mtime,
                            };
                            store.put(&s.path, e.clone());
                            store.save(); // 边测边存，中途关掉也不白测
                            let _ = tx.send(LoudMsg::Done(s.path.clone(), e));
                        }
                    }
                    None => {
                        let _ = tx.send(LoudMsg::Progress(
                            i + 1,
                            total,
                            format!("测量失败：{}", s.display()),
                        ));
                    }
                }
            }
            let _ = tx.send(LoudMsg::Finished);
        });
        self.loud_rx = Some(rx);
        self.status = "开始扫描响度…".to_string();
    }

    fn poll_loudness(&mut self) {
        let mut msgs = Vec::new();
        let mut done = false;
        if let Some(rx) = &self.loud_rx {
            loop {
                match rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }
        let mut last_status = None;
        for m in msgs {
            match m {
                LoudMsg::Done(path, e) => {
                    // 只关心当前曲目的结果，其余进缓存就够
                    let is_current = self
                        .current
                        .and_then(|i| self.songs.get(i))
                        .map(|s| s.path == path)
                        .unwrap_or(false);
                    self.loud_store.put(&path, e);
                    if is_current {
                        if let Some(i) = self.current {
                            let song = self.songs[i].clone();
                            self.apply_gain(&song);
                        }
                    }
                }
                LoudMsg::Progress(i, total, msg) => {
                    last_status = Some(format!("[{}/{}] {}", i, total, msg));
                }
                LoudMsg::Finished => {
                    self.loud_store.save();
                    last_status = Some("响度测量完成。".to_string());
                }
            }
        }
        if let Some(s) = last_status {
            self.status = s;
        }
        if done {
            self.loud_rx = None;
            self.loud_store.save();
        }
    }

    fn toggle_play(&mut self) {
        if self.current.is_none() {
            if !self.songs.is_empty() {
                self.play_index(0);
            }
            return;
        }
        if let Some(m) = &self.mpv {
            if m.paused() {
                m.set_pause(false);
                self.playing = true;
            } else {
                m.set_pause(true);
                self.playing = false;
            }
        }
    }

    fn next(&mut self) {
        if self.songs.is_empty() {
            return;
        }
        let i = match self.current {
            Some(i) => (i + 1) % self.songs.len(),
            None => 0,
        };
        self.play_index(i);
    }

    fn prev(&mut self) {
        if self.songs.is_empty() {
            return;
        }
        let i = match self.current {
            Some(i) => (i + self.songs.len() - 1) % self.songs.len(),
            None => 0,
        };
        self.play_index(i);
    }

    fn poll_mpv(&mut self) {
        let (pos, dur, paused, eof) = match &self.mpv {
            Some(m) => (m.time_pos(), m.duration(), m.paused(), m.eof_reached()),
            None => return,
        };
        self.position = pos;
        if dur > 0.0 {
            self.duration = dur;
        }
        if self.current.is_some() {
            self.playing = !paused;
        }
        if eof && !self.last_eof {
            self.last_eof = true;
            if self.current.is_some() {
                self.next();
            }
        } else if !eof {
            self.last_eof = false;
        }
    }

    // ---------------- 歌词 ----------------

    fn load_lyrics_for(&mut self, song: &Song) {
        self.lyrics.clear();
        self.lyric_index = None;

        let stem = song
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let dir = song.path.parent().map(|p| p.to_path_buf());

        let mut main: Option<Vec<LyricLine>> = None;
        if let Some(dir) = &dir {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if !ext.eq_ignore_ascii_case("lrc") {
                        continue;
                    }
                    let ps = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if ps.to_lowercase().ends_with(".tlyric") {
                        continue;
                    }
                    if ps.eq_ignore_ascii_case(&stem) {
                        if let Some(l) = lyrics::load_lrc_file(&p) {
                            main = Some(l);
                            break;
                        }
                    }
                }
            }
        }

        if main.is_none() {
            if let Some(text) = library::embedded_lyrics(&song.path) {
                let l = lyrics::parse_str(&text);
                if !l.is_empty() {
                    main = Some(l);
                }
            }
        }

        if let Some(mut m) = main {
            if let Some(dir) = &dir {
                let tp = dir.join(format!("{}.tlyric.lrc", stem));
                if let Some(t) = lyrics::load_lrc_file(&tp) {
                    lyrics::merge_translation(&mut m, &t);
                }
            }
            self.lyrics = m;
        }

        let pos_ms = self.pos_ms();
        self.lyric_index = self.current_lyric_index(pos_ms);
    }

    fn pos_ms(&self) -> u64 {
        ((self.position * 1000.0) as i64 + self.cfg.lyric_offset_ms).max(0) as u64
    }

    fn current_lyric_index(&self, pos_ms: u64) -> Option<usize> {
        if self.lyrics.is_empty() {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = self.lyrics.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.lyrics[mid].time() <= pos_ms {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Some(lo.saturating_sub(1))
    }

    fn download_current(&mut self) {
        let Some(idx) = self.current else {
            self.status = "还没有选中歌曲。".to_string();
            return;
        };
        let song = self.songs[idx].clone();
        self.status = format!("正在搜索歌词：{}", song.display());
        match lyrics_online::download_for_song(&song, &self.cfg.source_order) {
            Ok(Some((path, m))) => {
                self.status = format!(
                    "已保存（{}，匹配 {} 分：{} - {}）→ {}",
                    m.source_label,
                    m.song.score,
                    m.song.name,
                    m.song.artist(),
                    path.file_name().and_then(|s| s.to_str()).unwrap_or("")
                );
                self.load_lyrics_for(&song);
            }
            Ok(None) => {
                self.status = format!(
                    "未找到足够可信的歌词（阈值 {} 分）：{}",
                    lyrics_online::MIN_SCORE,
                    song.display()
                )
            }
            Err(e) => self.status = format!("下载失败：{}", e),
        }
    }

    fn download_all(&mut self) {
        if self.dl_rx.is_some() {
            self.status = "批量下载进行中…".to_string();
            return;
        }
        if self.songs.is_empty() {
            return;
        }
        let songs = self.songs.clone();
        let order = self.cfg.source_order.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            lyrics_online::download_missing(&songs, &order, |i, total, msg| {
                let _ = tx.send(format!("[{}/{}] {}", i, total, msg));
            });
            let _ = tx.send("批量下载完成。".to_string());
        });
        self.dl_rx = Some(rx);
        self.status = "开始批量下载歌词…".to_string();
    }

    fn poll_download(&mut self) {
        let mut msgs = Vec::new();
        let mut done = false;
        if let Some(rx) = &self.dl_rx {
            loop {
                match rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }
        if let Some(last) = msgs.pop() {
            self.status = last;
        }
        if done {
            self.dl_rx = None;
            if let Some(i) = self.current {
                let s = self.songs[i].clone();
                self.load_lyrics_for(&s);
            }
        }
    }

    // ---------------- 界面 ----------------

    fn draw_top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("音乐文件夹：");
                ui.add_sized(
                    [420.0, 24.0],
                    egui::TextEdit::singleline(&mut self.folder_input)
                        .hint_text("输入或粘贴文件夹路径"),
                );
                #[cfg(windows)]
                if ui.button("浏览…").clicked() {
                    if let Some(p) = pick_folder() {
                        self.folder_input = p.to_string_lossy().into_owned();
                        self.import_folder(p);
                    }
                }
                if ui.button("导入").clicked() {
                    let p = PathBuf::from(self.folder_input.trim());
                    self.import_folder(p);
                }
                let scanning = self.scan_rx.is_some();
                if ui
                    .add_enabled(!scanning, egui::Button::new("重新扫描"))
                    .clicked()
                {
                    self.start_scan("正在重新扫描…");
                }
                let downloading = self.dl_rx.is_some();
                if ui
                    .add_enabled(!downloading, egui::Button::new("批量下载歌词"))
                    .clicked()
                {
                    self.download_all();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ 设置").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                });
            });
            ui.add_space(6.0);
        });
    }

    fn draw_song_list(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("songs")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("曲库");
                    ui.label(format!("{} 首", self.songs.len()));
                    if self.cfg.shuffle {
                        ui.label(
                            egui::RichText::new("已打乱")
                                .size(12.0)
                                .color(egui::Color32::from_gray(150)),
                        );
                    }
                });
                ui.separator();
                let mut clicked = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.songs.is_empty() {
                            ui.label("（空）导入一个音乐文件夹后，这里会列出歌曲。");
                        }
                        for (i, s) in self.songs.iter().enumerate() {
                            let selected = self.current == Some(i);
                            let mut text = format!("{:>3}. {}", i + 1, s.display());
                            if s.has_lyric_file() {
                                text.push_str("  ♪");
                            }
                            let resp = ui.selectable_label(selected, text);
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                            resp.on_hover_text(s.path.to_string_lossy().to_string());
                        }
                    });
                if let Some(i) = clicked {
                    self.play_index(i);
                }
            });
    }

    fn draw_center(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let (title, artist) = match self.current {
                Some(i) => (
                    self.songs[i].title.clone(),
                    self.songs[i].artist.clone(),
                ),
                None => ("未播放".to_string(), String::new()),
            };

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading(title);
                if !artist.is_empty() {
                    ui.label(egui::RichText::new(artist).size(15.0).color(egui::Color32::GRAY));
                }
            });
            ui.add_space(10.0);

            // 进度条
            let dur = self.duration.max(0.0);
            let mut cur = if self.seeking { self.seek_value } else { self.position };
            ui.horizontal(|ui| {
                ui.label(fmt_time(cur));
                let width = (ui.available_width() - 110.0).max(120.0);
                let resp = ui.add_sized(
                    [width, 22.0],
                    egui::Slider::new(&mut cur, 0.0..=dur.max(1.0)).show_value(false),
                );
                if resp.drag_started() {
                    self.seeking = true;
                }
                if self.seeking {
                    self.seek_value = cur;
                }
                if resp.drag_stopped() {
                    self.seeking = false;
                    if let Some(m) = &self.mpv {
                        m.seek_abs(self.seek_value);
                    }
                }
                ui.label(fmt_time(dur));
            });

            ui.add_space(6.0);

            ui.horizontal_wrapped(|ui| {
                if ui.button("⏮").on_hover_text("上一首").clicked() {
                    self.prev();
                }
                let play = if self.playing { "⏸" } else { "▶" };
                if ui.button(play).on_hover_text("播放/暂停").clicked() {
                    self.toggle_play();
                }
                if ui.button("⏭").on_hover_text("下一首").clicked() {
                    self.next();
                }

                if ui
                    .button("🔀 打乱歌单")
                    .on_hover_text("把左侧列表的顺序打乱（不是随机播放，下一首就是列表里的下一首）")
                    .clicked()
                {
                    self.cfg.shuffle = true;
                    self.shuffle_playlist();
                    self.cfg.save();
                }
                if ui
                    .add_enabled(
                        self.original_order.is_some(),
                        egui::Button::new("↩ 原顺序"),
                    )
                    .on_hover_text("恢复到打乱之前的排列")
                    .clicked()
                {
                    self.cfg.shuffle = false;
                    self.restore_order();
                    self.cfg.save();
                }

                let mut vol = self.cfg.volume;
                if ui
                    .add(egui::Slider::new(&mut vol, 0.0..=130.0).text("音量"))
                    .changed()
                {
                    self.cfg.volume = vol;
                    if let Some(m) = &self.mpv {
                        m.set_volume(vol);
                    }
                }

                if ui
                    .selectable_label(self.cfg.lyric.visible, "桌面歌词")
                    .clicked()
                {
                    self.cfg.lyric.visible = !self.cfg.lyric.visible;
                    // 重新显示时窗口是新建的，位置要重新按配置摆一次，否则会跳回左上角
                    #[cfg(not(windows))]
                    {
                        self.lyric_pos_init = false;
                    }
                    #[cfg(windows)]
                    self.overlay.set_visible(self.cfg.lyric.visible);
                    self.cfg.save();
                }
                if ui
                    .button(if self.cfg.lyric.locked {
                        "解锁歌词"
                    } else {
                        "锁定歌词"
                    })
                    .on_hover_text("锁定后歌词窗鼠标穿透，不会挡住别的窗口")
                    .clicked()
                {
                    self.cfg.lyric.locked = !self.cfg.lyric.locked;
                    #[cfg(not(windows))]
                    if let Some(id) = self.lyric_viewport_id() {
                        ctx.send_viewport_cmd_to(
                            id,
                            egui::ViewportCommand::MousePassthrough(self.cfg.lyric.locked),
                        );
                    }
                    #[cfg(windows)]
                    self.overlay.set_locked(self.cfg.lyric.locked);
                    self.cfg.save();
                }
                if ui.button("下载歌词").clicked() {
                    self.download_current();
                }
            });

            ui.add_space(6.0);
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("歌词偏移：");
                if ui.button("-0.3s").clicked() {
                    self.cfg.lyric_offset_ms -= 300;
                    self.cfg.save();
                }
                ui.label(format!("{} ms", self.cfg.lyric_offset_ms));
                if ui.button("+0.3s").clicked() {
                    self.cfg.lyric_offset_ms += 300;
                    self.cfg.save();
                }
                if let Some(g) = self.applied_gain {
                    ui.separator();
                    ui.label(format!("响度 {:+.1} dB", g));
                }
                if ui.button("重新加载歌词").clicked() {
                    if let Some(i) = self.current {
                        let s = self.songs[i].clone();
                        self.load_lyrics_for(&s);
                    }
                }
            });

            ui.add_space(8.0);

            let pos_ms = self.pos_ms();
            let active = self.current_lyric_index(pos_ms);
            let base = self.cfg.lyric.font_size;
            let active_sung = c32(self.cfg.lyric.sung);
            let active_unsung = c32(self.cfg.lyric.unsung);
            let trans_color = c32(self.cfg.lyric.translation);

            if self.lyrics.is_empty() {
                ui.label("暂无歌词。点“下载歌词”，或把同名 .lrc 放到音频同目录。");
            } else {
                let mut new_scroll: Option<usize> = None;
                let mut scroll = egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, false]);
                if self.last_scrolled.is_none() {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                {
                    let lyrics = &self.lyrics;
                    let last_scrolled = self.last_scrolled;
                    scroll.show(ui, |ui| {
                        for (i, line) in lyrics.iter().enumerate() {
                            let is_active = Some(i) == active;
                            let text = line.text();
                            let resp = if is_active {
                                let end = lyrics
                                    .get(i + 1)
                                    .map(|l| l.time())
                                    .unwrap_or_else(|| line.time() + 5000);
                                let frac = line.sung_fraction(pos_ms, end);
                                ui.label(styled_job(
                                    &text,
                                    frac,
                                    base * 0.66,
                                    active_sung,
                                    active_unsung,
                                ))
                            } else {
                                ui.label(
                                    egui::RichText::new(text)
                                        .size(base * 0.42)
                                        .color(egui::Color32::from_gray(150)),
                                )
                            };
                            // 当前行滚到中间
                            if is_active && last_scrolled != Some(i) {
                                resp.scroll_to_me(Some(egui::Align::Center));
                                new_scroll = Some(i);
                            }
                            if let Some(t) = line.translation() {
                                ui.label(
                                    egui::RichText::new(format!("　{}", t))
                                        .size(base * 0.34)
                                        .color(trans_color),
                                );
                            }
                        }
                    });
                }
                if let Some(i) = new_scroll {
                    self.last_scrolled = Some(i);
                }
            }
        });
    }

    fn draw_settings(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        egui::Window::new("设置")
            .open(&mut open)
            .default_width(420.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("歌词外观");
                {
                    let lw = &mut self.cfg.lyric;
                    ui.add(
                        egui::Slider::new(&mut lw.font_size, 16.0..=72.0)
                            .text("字号")
                            .suffix(" px"),
                    );
                    ui.horizontal(|ui| {
                        ui.label("已唱颜色");
                        let mut c = lw.sung;
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            lw.sung = c;
                        }
                        ui.label("未唱颜色");
                        let mut c = lw.unsung;
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            lw.unsung = c;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("翻译颜色");
                        let mut c = lw.translation;
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            lw.translation = c;
                        }
                    });
                    let mut alpha_pct = lw.background_alpha as f32 / 255.0 * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut alpha_pct, 0.0..=100.0)
                                .text("歌词窗背景")
                                .suffix(" %"),
                        )
                        .changed()
                    {
                        lw.background_alpha = (alpha_pct / 100.0 * 255.0).round() as u8;
                    }
                    ui.checkbox(&mut lw.show_translation, "显示翻译");
                    ui.checkbox(&mut lw.show_next, "显示下一句");
                    ui.add(
                        egui::Slider::new(&mut lw.width, 320.0..=2400.0)
                            .text("歌词窗宽度")
                            .suffix(" px"),
                    );
                    ui.add(
                        egui::Slider::new(&mut lw.height, 70.0..=500.0)
                            .text("歌词窗高度")
                            .suffix(" px"),
                    );
                }

                ui.separator();
                ui.heading("歌词源顺序");
                ui.label(
                    egui::RichText::new("按顺序尝试，命中高分的源就直接用；分数不够再往下找。")
                        .size(12.0)
                        .color(egui::Color32::from_gray(150)),
                );
                let mut swap: Option<(usize, usize)> = None;
                let len = self.cfg.source_order.len();
                for i in 0..len {
                    let id = self.cfg.source_order[i].clone();
                    ui.horizontal(|ui| {
                        ui.label(format!("{}. {}", i + 1, lyrics_online::source_label(&id)));
                        if i > 0 && ui.small_button("↑").clicked() {
                            swap = Some((i, i - 1));
                        }
                        if i + 1 < len && ui.small_button("↓").clicked() {
                            swap = Some((i, i + 1));
                        }
                    });
                }
                if let Some((a, b)) = swap {
                    self.cfg.source_order.swap(a, b);
                    self.cfg.save();
                }

                ui.separator();
                ui.heading("等响度（ReplayGain）");
                ui.label(
                    egui::RichText::new(
                        "把每首歌的响度拉到同一水平。先看文件里的 ReplayGain 标签，没有就本地测：\
                        解码 → K 加权（38Hz 高通 + 1.5kHz +4dB 高架）→ 400ms 窗口 RMS → \
                        95 分位 → 目标响度 − 实测，夹在 ±20dB。",
                    )
                    .size(12.0)
                    .color(egui::Color32::from_gray(150)),
                );
                let mut loud_changed = false;
                {
                    let lc = &mut self.cfg.loudness;
                    loud_changed |= ui.checkbox(&mut lc.enabled, "启用等响度").changed();
                    loud_changed |= ui
                        .add(
                            egui::Slider::new(&mut lc.target_db, -26.0..=-12.0)
                                .text("目标响度")
                                .suffix(" dBFS"),
                        )
                        .changed();
                    loud_changed |= ui
                        .add(
                            egui::Slider::new(&mut lc.preamp_db, -12.0..=12.0)
                                .text("预增益")
                                .suffix(" dB"),
                        )
                        .changed();
                    loud_changed |= ui
                        .add(
                            egui::Slider::new(&mut lc.missing_gain_db, -12.0..=12.0)
                                .text("无数据时的缺省增益")
                                .suffix(" dB"),
                        )
                        .changed();
                    loud_changed |= ui
                        .checkbox(&mut lc.peak_protection, "峰值保护（用测量峰值限制增益，避免削波）")
                        .changed();
                    loud_changed |= ui
                        .checkbox(&mut lc.auto_analyze, "没有标签时自动现场测量（播放时后台跑）")
                        .changed();
                    loud_changed |= ui.checkbox(&mut lc.album_mode, "优先使用专辑增益").changed();
                    loud_changed |= ui
                        .add(
                            egui::Slider::new(&mut lc.max_analyze_sec, 15..=300)
                                .text("每首最多解码")
                                .suffix(" 秒"),
                        )
                        .changed();
                }
                ui.horizontal(|ui| {
                    let running = self.loud_rx.is_some();
                    if ui
                        .add_enabled(!running && !self.songs.is_empty(), egui::Button::new("扫描全库响度"))
                        .on_hover_text("只测还没测过的，边测边存，可以随时关掉下次接着测")
                        .clicked()
                    {
                        self.start_loud_scan();
                    }
                    if ui
                        .add_enabled(!running && !self.loud_store.is_empty(), egui::Button::new("清除测量结果"))
                        .clicked()
                    {
                        self.loud_store.items.clear();
                        self.loud_store.save();
                        self.status = "已清除响度测量结果。".to_string();
                        if let Some(i) = self.current {
                            let s = self.songs[i].clone();
                            self.apply_gain(&s);
                        }
                    }
                    ui.label(format!("已测 {} 首", self.loud_store.len()));
                    if let Some(g) = self.applied_gain {
                        ui.label(format!("当前：{:+.1} dB", g));
                    }
                });
                if loud_changed {
                    self.cfg.save();
                    if let Some(i) = self.current {
                        let s = self.songs[i].clone();
                        self.apply_gain(&s);
                    }
                }

                ui.separator();
                if ui
                    .checkbox(&mut self.cfg.rescan_on_start, "启动时自动重新扫描文件夹")
                    .changed()
                {
                    self.cfg.save();
                }
                if ui.button("保存设置").clicked() {
                    self.cfg.save();
                    self.status = "设置已保存。".to_string();
                }
            });
        self.show_settings = open;
    }

    fn draw_status(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.status.clone());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(err) = &self.mpv_err {
                        ui.colored_label(egui::Color32::LIGHT_RED, err.as_str());
                    } else {
                        ui.label(
                            egui::RichText::new("libmpv ✓")
                                .size(12.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                    }
                });
            });
        });
    }

    #[cfg(not(windows))]
    fn lyric_viewport_id(&self) -> Option<egui::ViewportId> {
        if self.cfg.lyric.visible {
            Some(egui::ViewportId::from_hash_of(LYRIC_WINDOW_TITLE))
        } else {
            None
        }
    }

    fn draw_lyric_window(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let _ = ctx;
            self.update_native_lyric();
        }
        #[cfg(not(windows))]
        self.draw_lyric_viewport(ctx);
    }

    /// Windows 版：把当前画面推给分层窗口线程
    #[cfg(windows)]
    fn update_native_lyric(&mut self) {
        // 拖动结束后窗口线程会回报新位置
        if let Some((x, y)) = self.overlay.take_moved() {
            let (fx, fy) = (x as f32, y as f32);
            if (self.cfg.lyric.x - fx).abs() > 0.5 || (self.cfg.lyric.y - fy).abs() > 0.5 {
                self.cfg.lyric.x = fx;
                self.cfg.lyric.y = fy;
                self.cfg.save();
            }
        }
        self.overlay.set_locked(self.cfg.lyric.locked);
        self.overlay.set_visible(self.cfg.lyric.visible);
        if !self.cfg.lyric.visible {
            return;
        }

        let pos_ms = self.pos_ms();
        let lw = &self.cfg.lyric;
        let mut lines: Vec<crate::lyric_win32::Line> = Vec::new();
        let mut start = 0u64;
        let mut end = 0u64;

        match self.current_lyric_index(pos_ms) {
            Some(idx) => {
                let line = &self.lyrics[idx];
                start = line.time();
                end = self
                    .lyrics
                    .get(idx + 1)
                    .map(|l| l.time())
                    .unwrap_or(start + 5000);
                lines.push(crate::lyric_win32::Line {
                    text: line.text().to_string(),
                    size: lw.font_size,
                    color: lw.unsung,
                    sung: Some((lw.sung, line.sung_fraction(pos_ms, end) as f32)),
                });
                if lw.show_translation {
                    if let Some(t) = line.translation() {
                        lines.push(crate::lyric_win32::Line {
                            text: t.to_string(),
                            size: lw.font_size * 0.5,
                            color: lw.translation,
                            sung: None,
                        });
                    }
                }
                if lw.show_next {
                    if let Some(n) = self.lyrics.get(idx + 1) {
                        lines.push(crate::lyric_win32::Line {
                            text: n.text().to_string(),
                            size: lw.font_size * 0.62,
                            color: [
                                (lw.unsung[0] as f32 * 0.55) as u8,
                                (lw.unsung[1] as f32 * 0.55) as u8,
                                (lw.unsung[2] as f32 * 0.55) as u8,
                            ],
                            sung: None,
                        });
                    }
                }
            }
            None => {
                let title = self
                    .current
                    .and_then(|i| self.songs.get(i))
                    .map(|s| format!("♪ {}", s.title))
                    .unwrap_or_else(|| "♪ Rust Music".to_string());
                lines.push(crate::lyric_win32::Line {
                    text: title,
                    size: lw.font_size,
                    color: lw.unsung,
                    sung: None,
                });
            }
        }

        self.overlay.push(crate::lyric_win32::Paint {
            lines,
            width: lw.width,
            height: lw.height,
            bg_alpha: lw.background_alpha,
            pos_ms,
            playing: self.playing,
            line_start_ms: start,
            line_end_ms: end,
        });
    }

    /// 其它平台：仍用 egui 子 viewport（依赖 GL 帧缓冲 alpha）
    #[cfg(not(windows))]
    fn draw_lyric_viewport(&mut self, ctx: &egui::Context) {
        if !self.cfg.lyric.visible {
            return;
        }

        let pos_ms = self.pos_ms();
        let (cur, frac, trans, next) = match self.current_lyric_index(pos_ms) {
            Some(idx) => {
                let line = &self.lyrics[idx];
                let end = self
                    .lyrics
                    .get(idx + 1)
                    .map(|l| l.time())
                    .unwrap_or_else(|| line.time() + 5000);
                (
                    line.text(),
                    line.sung_fraction(pos_ms, end),
                    line.translation().map(|s| s.to_string()),
                    self.lyrics
                        .get(idx + 1)
                        .map(|l| l.text())
                        .unwrap_or_default(),
                )
            }
            None => {
                let fallback = self
                    .current
                    .and_then(|i| self.songs.get(i))
                    .map(|s| format!("♪ {}", s.title))
                    .unwrap_or_else(|| "♪ Rust Music".to_string());
                (fallback, 0.0, None, String::new())
            }
        };

        let lw = self.cfg.lyric.clone();
        let id = egui::ViewportId::from_hash_of(LYRIC_WINDOW_TITLE);

        let mut builder = egui::ViewportBuilder::default()
            .with_title(LYRIC_WINDOW_TITLE)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(true)
            .with_mouse_passthrough(lw.locked)
            .with_inner_size([lw.width, lw.height]);
        // 位置只在首帧设置：之后交给系统拖动，避免每帧回写导致"拖不动/不跟手"
        if !self.lyric_pos_init {
            builder = builder.with_position([lw.x, lw.y]);
        }

        let sung = c32(lw.sung);
        let unsung = c32(lw.unsung);
        let tr = c32(lw.translation);
        let bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, lw.background_alpha);
        let fs = lw.font_size;

        let mut want_drag = false;
        let mut lock_clicked = false;
        let mut hide_clicked = false;
        let mut pos_after_drag: Option<egui::Pos2> = None;

        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(bg)
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ctx, |ui| {
                    let resp = ui.interact(
                        ui.max_rect(),
                        egui::Id::new("lyric_drag_area"),
                        if lw.locked {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click_and_drag()
                        },
                    );
                    // 按下即交给系统拖动（winit 的 drag_window），比每帧算增量跟手得多
                    if !lw.locked && resp.is_pointer_button_down_on() {
                        if ctx.input(|i| i.pointer.any_pressed()) {
                            want_drag = true;
                        }
                    }

                    ui.vertical_centered(|ui| {
                        ui.add_space(2.0);
                        ui.label(styled_job(&cur, frac, fs, sung, unsung));
                        if lw.show_translation {
                            if let Some(t) = &trans {
                                ui.label(egui::RichText::new(t).size(fs * 0.5).color(tr));
                            }
                        }
                        if lw.show_next && !next.is_empty() {
                            ui.label(
                                egui::RichText::new(&next)
                                    .size(fs * 0.62)
                                    .color(unsung.gamma_multiply(0.55)),
                            );
                        }
                    });

                    if !lw.locked {
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("🔒")
                                .on_hover_text("锁定（鼠标穿透）")
                                .clicked()
                            {
                                lock_clicked = true;
                            }
                            if ui.small_button("✕").on_hover_text("隐藏歌词").clicked() {
                                hide_clicked = true;
                            }
                        });
                    }
                });

            // 拖动结束后把真实位置读回来存进配置
            if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
                pos_after_drag = Some(rect.min);
            }
        });

        if !self.lyric_pos_init {
            self.lyric_pos_init = true;
        }
        if want_drag {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::StartDrag);
        }
        if let Some(p) = pos_after_drag {
            if (self.cfg.lyric.x - p.x).abs() > 0.5 || (self.cfg.lyric.y - p.y).abs() > 0.5 {
                self.cfg.lyric.x = p.x;
                self.cfg.lyric.y = p.y;
                self.cfg.save();
            }
        }
        if lock_clicked {
            self.cfg.lyric.locked = true;
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::MousePassthrough(true));
            self.cfg.save();
        }
        if hide_clicked {
            self.cfg.lyric.visible = false;
            self.cfg.save();
        }
    }

    /// 主窗口透明后必须自己铺一层不透明底，否则会漏出桌面（仅非 Windows：那边歌词窗
    /// 已经换成独立的分层窗口，主窗口不再需要开透明）
    #[cfg(not(windows))]
    fn draw_backdrop(&self, ctx: &egui::Context) {
        let rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(rect, 0.0, ctx.style().visuals.panel_fill);
    }
}

impl eframe::App for App {
    /// 主窗口开了透明（为了给桌面歌词窗提供 alpha 通道），所以这里清成全透明，
    /// 主窗口由各 panel 自己填不透明背景。
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        #[cfg(not(windows))]
        {
            let _ = visuals;
            [0.0, 0.0, 0.0, 0.0]
        }
        #[cfg(windows)]
        visuals.window_fill.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_mpv();
        self.poll_download();
        self.poll_loudness();
        self.poll_scan();
        self.lyric_index = self.current_lyric_index(self.pos_ms());

        #[cfg(not(windows))]
        self.draw_backdrop(ctx);
        #[cfg(windows)]
        if !self.overlay_started {
            self.overlay.spawn();
            self.overlay_started = true;
        }
        self.draw_top(ctx);
        self.draw_status(ctx);
        self.draw_song_list(ctx);
        self.draw_center(ctx);
        self.draw_settings(ctx);
        self.draw_lyric_window(ctx);

        // 播放中把重绘提到 20fps：桌面歌词窗靠插值走逐字高亮，
        // 但换行、进度、状态还是要靠这里推
        let ms = if self.playing { 50 } else { 100 };
        ctx.request_repaint_after(Duration::from_millis(ms));
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.cfg.save();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.cfg.save();
    }
}

// ---------------- 工具函数 ----------------

fn c32(c: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn sort_songs(v: &mut [Song]) {
    v.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then(a.artist.cmp(&b.artist))
    });
}

/// 生成「已唱部分高亮 + 未唱部分」的富文本
fn styled_job(
    text: &str,
    frac: f64,
    size: f32,
    sung: egui::Color32,
    unsung: egui::Color32,
) -> egui::text::LayoutJob {
    let chars: Vec<char> = text.chars().collect();
    let n = ((chars.len() as f64) * frac.clamp(0.0, 1.0)).round() as usize;
    let a: String = chars[..n].iter().collect();
    let b: String = chars[n..].iter().collect();
    let font = egui::FontId::new(size, egui::FontFamily::Proportional);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &a,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: sung,
            ..Default::default()
        },
    );
    job.append(
        &b,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: unsung,
            ..Default::default()
        },
    );
    job
}

fn shuffle<T>(v: &mut [T]) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        | 1;
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed as usize) % (i + 1);
        v.swap(i, j);
    }
}

#[cfg(windows)]
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}
