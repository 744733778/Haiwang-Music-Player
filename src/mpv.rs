use anyhow::{anyhow, Result};
use libloading::Library;
use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

pub type MpvHandle = *mut c_void;

const FMT_FLAG: c_int = 3;
const FMT_DOUBLE: c_int = 5;

#[repr(C)]
#[allow(dead_code)] // 事件回调保留完整字段，方便后续接「播放结束自动下一首」
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

pub struct MpvApi {
    _lib: Library,
    create: unsafe extern "C" fn() -> MpvHandle,
    initialize: unsafe extern "C" fn(MpvHandle) -> c_int,
    set_option_string: unsafe extern "C" fn(MpvHandle, *const c_char, *const c_char) -> c_int,
    set_property: unsafe extern "C" fn(MpvHandle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(MpvHandle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(MpvHandle, *const c_char) -> *mut c_char,
    command: unsafe extern "C" fn(MpvHandle, *const *const c_char) -> c_int,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    wait_event: unsafe extern "C" fn(MpvHandle, c_double) -> *mut MpvEvent,
    free: unsafe extern "C" fn(*mut c_void),
    terminate_destroy: unsafe extern "C" fn(MpvHandle),
}

impl MpvApi {
    fn load() -> Result<Self> {
        let names: &[&str] = if cfg!(windows) {
            &["libmpv-2.dll", "mpv-2.dll", "libmpv.dll", "mpv.dll"]
        } else {
            &["libmpv.so.2", "libmpv.so.1", "libmpv.so"]
        };

        let mut lib = None;
        let mut last = String::new();
        for n in names {
            match unsafe { Library::new(n) } {
                Ok(l) => {
                    lib = Some(l);
                    break;
                }
                Err(e) => last = e.to_string(),
            }
        }
        let lib = lib.ok_or_else(|| {
            anyhow!(
                "无法加载 libmpv。Windows 请把 libmpv-2.dll 放到 exe 同目录；Linux 请安装 libmpv。({})",
                last
            )
        })?;

        unsafe {
            macro_rules! sym {
                ($name:literal, $ty:ty) => {{
                    let s: libloading::Symbol<$ty> = lib.get($name.as_bytes())?;
                    *s
                }};
            }
            Ok(Self {
                create: sym!("mpv_create", unsafe extern "C" fn() -> MpvHandle),
                initialize: sym!("mpv_initialize", unsafe extern "C" fn(MpvHandle) -> c_int),
                set_option_string: sym!(
                    "mpv_set_option_string",
                    unsafe extern "C" fn(MpvHandle, *const c_char, *const c_char) -> c_int
                ),
                set_property: sym!(
                    "mpv_set_property",
                    unsafe extern "C" fn(MpvHandle, *const c_char, c_int, *mut c_void) -> c_int
                ),
                get_property: sym!(
                    "mpv_get_property",
                    unsafe extern "C" fn(MpvHandle, *const c_char, c_int, *mut c_void) -> c_int
                ),
                get_property_string: sym!(
                    "mpv_get_property_string",
                    unsafe extern "C" fn(MpvHandle, *const c_char) -> *mut c_char
                ),
                command: sym!(
                    "mpv_command",
                    unsafe extern "C" fn(MpvHandle, *const *const c_char) -> c_int
                ),
                error_string: sym!(
                    "mpv_error_string",
                    unsafe extern "C" fn(c_int) -> *const c_char
                ),
                wait_event: sym!(
                    "mpv_wait_event",
                    unsafe extern "C" fn(MpvHandle, c_double) -> *mut MpvEvent
                ),
                free: sym!("mpv_free", unsafe extern "C" fn(*mut c_void)),
                terminate_destroy: sym!(
                    "mpv_terminate_destroy",
                    unsafe extern "C" fn(MpvHandle)
                ),
                _lib: lib,
            })
        }
    }
}

pub struct Mpv {
    api: Arc<MpvApi>,
    ctx: MpvHandle,
    /// 播放结束事件（来自后台事件泵）。只有播放实例才有；
    /// 一次性转码/响度分析的实例走 `wait_end_file`，不能同抢事件队列。
    end_rx: Option<Mutex<Receiver<()>>>,
}

/// mpv 的 ctx 是不透明的裸指针，事件泵线程要用，单独包一层声明 Send
struct CtxPtr(MpvHandle);
unsafe impl Send for CtxPtr {}

impl CtxPtr {
    fn raw(&self) -> MpvHandle {
        self.0
    }
}

unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    /// 播放器实例：启用「播放结束」事件监听
    pub fn new(volume: f64) -> Result<Self> {
        Self::build(volume, &[], true)
    }

    /// 带额外选项创建实例（等响度分析用：`o=<临时 wav>` 等）。
    /// 不启用事件泵 —— 这类实例自己用 `wait_end_file` 阻塞取事件。
    pub fn with_options(volume: f64, extra: &[(&str, &str)]) -> Result<Self> {
        Self::build(volume, extra, false)
    }

    fn build(volume: f64, extra: &[(&str, &str)], with_events: bool) -> Result<Self> {
        let api = Arc::new(MpvApi::load()?);
        let ctx = unsafe { (api.create)() };
        if ctx.is_null() {
            return Err(anyhow!("mpv_create 返回空"));
        }
        let mut m = Self {
            api,
            ctx,
            end_rx: None,
        };

        m.set_option("config", "no");
        for (k, v) in extra {
            m.set_option(k, v);
        }
        m.set_option("vo", "null");
        // 仅用于无声环境的自动测试：HWPLAYER_AO_NULL=1 时用 mpv 内置 null ao
        // （它按实时推进时钟），避免容器里没有声卡导致瞬间“播完”。
        if std::env::var_os("HWPLAYER_AO_NULL").is_some() {
            m.set_option("ao", "null");
        }
        m.set_option("audio-display", "no");
        m.set_option("idle", "yes");
        // 必须 keep-open=yes：否则文件播完会被卸载、核心进入 idle，
        // 属性 eof-reached 随文件一起失效，靠轮询永远看不到“播完了”。
        m.set_option("keep-open", "yes");
        m.set_option("terminal", "no");
        m.set_option("osc", "no");
        m.set_option("input-default-bindings", "no");

        let rc = unsafe { (m.api.initialize)(m.ctx) };
        if rc < 0 {
            return Err(anyhow!("mpv_initialize 失败：{}", m.err_string(rc)));
        }
        m.set_volume(volume);

        if with_events {
            let (tx, rx) = mpsc::channel::<()>();
            m.end_rx = Some(Mutex::new(rx));
            spawn_event_pump(Arc::clone(&m.api), CtxPtr(ctx), tx);
        }
        Ok(m)
    }

    /// 取「上一曲播放结束」事件；一次排空，返回是否至少发生过一次。
    /// 只认 EOF / ERROR，忽略 loadfile(replace) 产生的 STOP，避免切歌把自己再切一次。
    pub fn take_end_file(&self) -> bool {
        let Some(rx) = &self.end_rx else { return false };
        let Ok(guard) = rx.lock() else { return false };
        let mut got = false;
        loop {
            match guard.try_recv() {
                Ok(()) => got = true,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        got
    }

    fn err_string(&self, rc: c_int) -> String {
        unsafe {
            let p = (self.api.error_string)(rc);
            if p.is_null() {
                rc.to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    fn set_option(&self, key: &str, val: &str) {
        if let (Ok(k), Ok(v)) = (CString::new(key), CString::new(val)) {
            unsafe {
                (self.api.set_option_string)(self.ctx, k.as_ptr(), v.as_ptr());
            }
        }
    }

    fn cname(name: &str) -> Option<CString> {
        CString::new(name).ok()
    }

    pub fn load(&self, path: &str, autoplay: bool) {
        let path = path.to_string();
        let args = [
            CString::new("loadfile").unwrap(),
            CString::new(path).unwrap_or_default(),
            CString::new("replace").unwrap(),
        ];
        let ptrs = [
            args[0].as_ptr(),
            args[1].as_ptr(),
            args[2].as_ptr(),
            std::ptr::null(),
        ];
        unsafe {
            (self.api.command)(self.ctx, ptrs.as_ptr());
        }
        if autoplay {
            self.set_pause(false);
        }
    }

    pub fn set_pause(&self, paused: bool) {
        if let Some(k) = Self::cname("pause") {
            let mut v: c_int = if paused { 1 } else { 0 };
            unsafe {
                (self.api.set_property)(
                    self.ctx,
                    k.as_ptr(),
                    FMT_FLAG,
                    &mut v as *mut _ as *mut c_void,
                );
            }
        }
    }

    pub fn paused(&self) -> bool {
        let mut v: c_int = 0;
        if let Some(k) = Self::cname("pause") {
            unsafe {
                (self.api.get_property)(
                    self.ctx,
                    k.as_ptr(),
                    FMT_FLAG,
                    &mut v as *mut _ as *mut c_void,
                );
            }
        }
        v != 0
    }

    #[allow(dead_code)]
    pub fn eof_reached(&self) -> bool {
        self.get_flag("eof-reached")
    }

    /// 核心是否处于 idle（没有已加载的文件）。
    #[allow(dead_code)]
    pub fn idle_active(&self) -> bool {
        self.get_flag("idle-active")
    }

    fn get_flag(&self, name: &str) -> bool {
        let mut v: c_int = 0;
        if let Some(k) = Self::cname(name) {
            unsafe {
                (self.api.get_property)(
                    self.ctx,
                    k.as_ptr(),
                    FMT_FLAG,
                    &mut v as *mut _ as *mut c_void,
                );
            }
        }
        v != 0
    }

    fn get_f64(&self, name: &str) -> f64 {
        let mut v: c_double = 0.0;
        if let Some(k) = Self::cname(name) {
            unsafe {
                (self.api.get_property)(
                    self.ctx,
                    k.as_ptr(),
                    FMT_DOUBLE,
                    &mut v as *mut _ as *mut c_void,
                );
            }
        }
        if v.is_finite() {
            v
        } else {
            0.0
        }
    }

    pub fn time_pos(&self) -> f64 {
        self.get_f64("time-pos")
    }

    pub fn duration(&self) -> f64 {
        self.get_f64("duration")
    }

    pub fn set_volume(&self, volume: f64) {
        if let Some(k) = Self::cname("volume") {
            let mut v = volume.clamp(0.0, 130.0);
            unsafe {
                (self.api.set_property)(
                    self.ctx,
                    k.as_ptr(),
                    FMT_DOUBLE,
                    &mut v as *mut _ as *mut c_void,
                );
            }
        }
    }

    /// 读回 mpv 当前音量（外部改过音量时用来对齐 UI）
    #[allow(dead_code)]
    pub fn volume(&self) -> f64 {
        self.get_f64("volume")
    }

    pub fn seek_abs(&self, secs: f64) {
        let secs = secs.max(0.0);
        let args = [
            CString::new("seek").unwrap(),
            CString::new(format!("{:.3}", secs)).unwrap(),
            CString::new("absolute+exact").unwrap(),
        ];
        let ptrs = [
            args[0].as_ptr(),
            args[1].as_ptr(),
            args[2].as_ptr(),
            std::ptr::null(),
        ];
        unsafe {
            (self.api.command)(self.ctx, ptrs.as_ptr());
        }
    }

    /// 停止播放（退出前清理用，目前由 Drop 负责）
    #[allow(dead_code)]
    pub fn stop(&self) {
        let args = [CString::new("stop").unwrap()];
        let ptrs = [args[0].as_ptr(), std::ptr::null()];
        unsafe {
            (self.api.command)(self.ctx, ptrs.as_ptr());
        }
    }

    /// 阻塞等一个事件，直到「文件播放结束」或「核心退出」。
    /// 用于等一次性转码（`--o=...`）跑完：encode 模式下核心播完就会走 SHUTDOWN，
    /// 光看 `idle-active` 是等不到的。
    pub fn wait_end_file(&self, timeout: f64) -> bool {
        const EV_SHUTDOWN: c_int = 1;
        const EV_END_FILE: c_int = 7;
        unsafe {
            let ev = (self.api.wait_event)(self.ctx, timeout);
            if ev.is_null() {
                return false;
            }
            matches!((*ev).event_id, EV_SHUTDOWN | EV_END_FILE)
        }
    }

    /// 执行一条命令（参数个数可变），返回 mpv 的错误码
    pub fn command(&self, args: &[&str]) -> c_int {
        let owned: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a).unwrap_or_default())
            .collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe { (self.api.command)(self.ctx, ptrs.as_ptr()) }
    }

    /// 读字符串属性；不存在时返回 None
    #[allow(dead_code)]
    pub fn get_string(&self, name: &str) -> Option<String> {
        let k = Self::cname(name)?;
        unsafe {
            let p = (self.api.get_property_string)(self.ctx, k.as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.api.free)(p as *mut c_void);
            Some(s)
        }
    }

    /// 设置音频增益（ReplayGain）：`None` 表示关闭。
    /// 走 mpv 的 `af` 滤镜链（`volume` 滤镜），而不是 `volume` 属性——
    /// 后者上限只有 130%，不足以表达 +20dB。
    pub fn set_gain_db(&self, db: Option<f32>) {
        // 用 `af set <链>` 一次性替换整条链：
        // `af clr` 在 mpv 0.37 上会返回 MPV_ERROR_OPTION_FORMAT（-4），
        // 实测只有带第三个参数的 `af set` 才是可靠的。
        //
        // 增益用线性倍数而不是 "XdB"：负的 dB 以 '-' 开头，会被 mpv 的命令参数
        // 解析当成下一个选项而报错。volume 滤镜的 volume 选项接受线性倍数。
        let chain = match db {
            Some(db) if db.abs() >= 0.05 => {
                let lin = 10f32.powf(db / 20.0).clamp(0.001, 10.0);
                format!("volume=volume={:.5}", lin)
            }
            _ => String::new(),
        };
        self.command(&["af", "set", &chain]);
    }

    #[allow(dead_code)]
    pub fn media_title(&self) -> String {
        if let Some(k) = Self::cname("media-title") {
            unsafe {
                let p = (self.api.get_property_string)(self.ctx, k.as_ptr());
                if p.is_null() {
                    return String::new();
                }
                let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                (self.api.free)(p as *mut c_void);
                s
            }
        } else {
            String::new()
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe {
            (self.api.terminate_destroy)(self.ctx);
        }
    }
}

/// 「播放结束」事件泵。
///
/// 为什么不能靠轮询 `eof-reached`：mpv 播完会把文件卸载/或与 loadfile 的异步过程
/// 交错，属性时而读不到、时而是上一曲的残留值，UI 侧要么永远等不到、要么连环跳歌。
/// 事件是每次播放结束恰好一次，语义干净。
fn spawn_event_pump(api: Arc<MpvApi>, ctx: CtxPtr, tx: mpsc::Sender<()>) {
    std::thread::spawn(move || {
        const EV_NONE: c_int = 0;
        const EV_SHUTDOWN: c_int = 1;
        const EV_END_FILE: c_int = 7;
        const REASON_EOF: c_int = 0;
        const REASON_ERROR: c_int = 4;
        loop {
            let ev = unsafe { (api.wait_event)(ctx.raw(), 0.2) };
            if ev.is_null() {
                continue;
            }
            unsafe {
                let dbg = std::env::var_os("HWPLAYER_DEBUG").is_some();
                match (*ev).event_id {
                    EV_NONE => {}
                    EV_SHUTDOWN => break,
                    EV_END_FILE => {
                        // mpv_event_end_file.reason 是结构体第一个字段
                        let reason = if (*ev).data.is_null() {
                            -1
                        } else {
                            *((*ev).data as *const c_int)
                        };
                        // 只认 EOF(0) 与 ERROR(4)；STOP(2)/QUIT(3)/REDIRECT(5) 忽略，
                        // 否则 loadfile(replace) 自身产生的 STOP 会造成连锁切歌。
                        if dbg {
                            eprintln!("[pump] END_FILE reason={}", reason);
                        }
                        if reason == REASON_EOF || reason == REASON_ERROR {
                            if tx.send(()).is_err() {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });
}
