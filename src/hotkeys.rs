//! 全局热键：↑/↓ 音量，←/→ 切歌。
//!
//! 做法照搬 `744733778/txt-reader` 在 Linux 上的实现：
//! `global-hotkey` 0.6 在 X11 下用 `XGrabKey` 注册，不需要 root、不用常驻守护进程；
//! 事件通过 `GlobalHotKeyEvent` 的全局 channel 送达，工作线程转成动作队列，
//! 由主线程在 `update()` 里消费（UI 状态只能在主线程改）。
//!
//! 代价要说清楚：不带修饰键的方向键一旦被 grab，运行期间**别的程序收不到方向键**。
//! 这是需求本身决定的（后台控制），设置里给了总开关，关掉就完全退回应用内快捷键。

use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    VolumeUp,
    VolumeDown,
    Prev,
    Next,
}

impl Action {
    /// 去抖槽位
    pub fn slot(self) -> usize {
        match self {
            Action::VolumeUp => 0,
            Action::VolumeDown => 1,
            Action::Prev => 2,
            Action::Next => 3,
        }
    }

    /// 连按去抖：切歌必须比音量更钝，否则按住 ←/→ 的自动重复会连跳十几首。
    pub fn min_gap(self) -> Duration {
        match self {
            Action::Prev | Action::Next => Duration::from_millis(220),
            Action::VolumeUp | Action::VolumeDown => Duration::from_millis(45),
        }
    }
}

/// 固定键位。顺序即注册顺序，方便日志排查。
const BINDINGS: [(Code, Action); 4] = [
    (Code::ArrowUp, Action::VolumeUp),
    (Code::ArrowDown, Action::VolumeDown),
    (Code::ArrowLeft, Action::Prev),
    (Code::ArrowRight, Action::Next),
];

pub const HINT: &str = "↑/↓ 音量 · ←/→ 切歌";

pub struct Hotkeys {
    /// 持有它才算保持注册；Drop 时自动注销
    #[allow(dead_code)]
    manager: Option<GlobalHotKeyManager>,
    queue: Arc<Mutex<Vec<Action>>>,
    /// 给设置界面看的状态
    pub status: String,
    /// 是否真的注册成功（决定还要不要走应用内方向键兜底）
    pub active: bool,
}

impl Hotkeys {
    pub fn inactive(reason: impl Into<String>) -> Self {
        Self {
            manager: None,
            queue: Arc::new(Mutex::new(Vec::new())),
            status: reason.into(),
            active: false,
        }
    }

    pub fn install(ctx: &egui::Context, enabled: bool) -> Self {
        if !enabled {
            return Self::inactive("已关闭（仅窗口内方向键可用）");
        }
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                return Self::inactive(format!("不可用：{e}（Wayland 会话需要 X11 才能全局 grab）"))
            }
        };

        let mut map: HashMap<u32, Action> = HashMap::new();
        let mut failed: Vec<String> = Vec::new();
        for (code, act) in BINDINGS {
            let hk = HotKey::new(None, code);
            match manager.register(hk) {
                Ok(_) => {
                    map.insert(hk.id(), act);
                }
                Err(e) => failed.push(format!("{code:?}: {e}")),
            }
        }
        if map.is_empty() {
            return Self::inactive(format!("注册失败：{}", failed.join("；")));
        }

        let queue = Arc::new(Mutex::new(Vec::new()));
        let q = queue.clone();
        let ctx2 = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("hotkey-evt".to_string())
            .spawn(move || {
                let debug = std::env::var_os("HWPLAYER_DEBUG").is_some();
                // 全局 channel：manager 每触发一次热键就投递一条
                let rx = GlobalHotKeyEvent::receiver().clone();
                while let Ok(ev) = rx.recv() {
                    if ev.state != HotKeyState::Pressed {
                        continue;
                    }
                    if let Some(a) = map.get(&ev.id) {
                        if debug {
                            // 打印在 grab 线程里：即使窗口被最小化/失焦也能证明 X11 抓到了键
                            eprintln!("[hotkey] grab fired: {a:?}");
                        }
                        if let Ok(mut v) = q.lock() {
                            // UI 若长时间不跑帧，别无限堆
                            if v.len() < 64 {
                                v.push(*a);
                            }
                        }
                        ctx2.request_repaint();
                    }
                }
            })
            .is_ok();

        let status = if failed.is_empty() {
            format!("已启用（{HINT}）")
        } else {
            format!("部分键位被占用：{}；其余已启用（{HINT}）", failed.join("；"))
        };
        if !spawned {
            return Self::inactive("事件线程启动失败");
        }
        Self {
            manager: Some(manager),
            queue,
            status,
            active: true,
        }
    }

    pub fn drain(&self) -> Vec<Action> {
        match self.queue.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }
}
