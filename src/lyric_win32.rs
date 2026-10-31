//! 桌面歌词窗（Windows 专用）：Win32 分层窗口 + GDI 自绘。
//!
//! 为什么不用 egui 的 `show_viewport_immediate` + `with_transparent(true)`：
//! eframe/glutin 的透明依赖「窗口的 GL 帧缓冲带 alpha 通道，并且 DWM 按像素合成」。
//! 在远程桌面、云端虚拟显卡、部分老驱动上这条链子会断（glutin 只给 WGL 传
//! `WGL_TRANSPARENT_ARB`，真正的 per-pixel alpha 要靠 `cAlphaBits=8` + DWM 合成），
//! 结果就是整窗不透明黑底。
//!
//! 这里改成 `UpdateLayeredWindow`：我们自己把歌词画到一张 32 位 DIB 上，
//! 逐像素给出预乘 alpha，交给 DWM 合成。这条路从 Win2000 起就有，跟 GL 驱动无关，
//! 也不受远程桌面影响，透明是**确定**的。
//!
//! 附带好处：拖动直接用 `WM_NCLBUTTONDOWN(HTCAPTION)` 进入系统的模态拖动循环，
//! 窗口由系统移动，零延迟、完全跟手（不再有「每帧回写位置」的抖动）。

#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDeviceCaps,
    GetTextExtentPoint32W, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HFONT, LOGPIXELSY, TRANSPARENT,
    AC_SRC_ALPHA, AC_SRC_OVER,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, GetWindowLongPtrW,
    GetWindowRect, KillTimer, LoadCursorW, PostQuitMessage, RegisterClassExW, SendMessageW,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
    UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE, GWLP_USERDATA, HWND_TOPMOST,
    IDC_ARROW, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
    ULW_ALPHA, WM_APP, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOVE, WM_NCCREATE,
    WM_NCLBUTTONDOWN, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

const CLASS_NAME: &[u16] = &[
    'R' as u16, 'm' as u16, 'L' as u16, 'y' as u16, 'r' as u16, 'i' as u16, 'c' as u16,
    'W' as u16, 'n' as u16, 'd' as u16, 0,
];
const TIMER_ID: usize = 1;
/// 重绘节拍：33ms ≈ 30fps，逐字高亮够顺滑，CPU 占用可忽略
const TICK_MS: u32 = 33;
const HTCAPTION: isize = 2;

// ---------------------------------------------------------------- 对外数据

/// 一行歌词
#[derive(Clone, Default)]
pub struct Line {
    pub text: String,
    /// 字号（逻辑像素，会按 DPI 缩放）
    pub size: f32,
    pub color: [u8; 3],
    /// 逐字高亮：(已唱颜色, 已唱占比 0..1)
    pub sung: Option<([u8; 3], f32)>,
}

/// 整窗的画面状态，由 UI 线程写入、窗口线程读取
#[derive(Clone, Default)]
pub struct Paint {
    pub lines: Vec<Line>,
    /// 窗口尺寸（逻辑像素）
    pub width: f32,
    pub height: f32,
    /// 背景不透明度 0-255
    pub bg_alpha: u8,
    /// 当前播放位置（毫秒）与是否在播——窗口线程按自己的节拍插值，逐字高亮才顺滑
    pub pos_ms: u64,
    pub playing: bool,
    /// 当前行起止（毫秒），用于插值
    pub line_start_ms: u64,
    pub line_end_ms: u64,
}

struct Shared {
    paint: Mutex<Paint>,
    dirty: AtomicBool,
    quit: AtomicBool,
    hwnd: AtomicIsize,
    /// 窗口被拖动后的新位置（UI 线程取走后写入配置）
    moved: Mutex<Option<(i32, i32)>>,
    locked: AtomicBool,
    visible: AtomicBool,
    /// CreateWindowEx 是否成功
    alive: AtomicBool,
    /// 每次 push 的时间基准，用于插值
    base: Mutex<Option<(Instant, u64)>>,
}

static SHARED: OnceLock<Box<Shared>> = OnceLock::new();

fn shared() -> &'static Shared {
    SHARED.get_or_init(|| {
        Box::new(Shared {
            paint: Mutex::new(Paint::default()),
            dirty: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            moved: Mutex::new(None),
            locked: AtomicBool::new(false),
            visible: AtomicBool::new(false),
            alive: AtomicBool::new(false),
            base: Mutex::new(None),
        })
    })
}

/// UI 线程持有的句柄
pub struct Overlay {
    started: bool,
}

impl Overlay {
    pub fn new() -> Self {
        Self { started: false }
    }

    /// 创建窗口线程（只做一次）
    pub fn spawn(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let sh = shared();
        std::thread::Builder::new()
            .name("lyric-window".into())
            .spawn(move || unsafe { run_window_thread(sh) })
            .ok();
    }

    /// 推一帧画面（UI 线程每帧调用即可，内部只在数据变化时标脏）
    pub fn push(&self, p: Paint) {
        let sh = shared();
        if !sh.alive.load(Ordering::Relaxed) {
            // 窗口线程还没起来（或创建失败）：先存着，起来后第一次 tick 会画出来
            *sh.paint.lock().unwrap() = p;
            return;
        }
        {
            let mut base = sh.base.lock().unwrap();
            *base = Some((Instant::now(), p.pos_ms));
        }
        *sh.paint.lock().unwrap() = p;
        sh.dirty.store(true, Ordering::Relaxed);
    }

    pub fn set_visible(&self, v: bool) {
        let sh = shared();
        if sh.visible.swap(v, Ordering::Relaxed) == v {
            return;
        }
        let hwnd = sh.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                ShowWindow(hwnd as HWND, if v { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        sh.dirty.store(true, Ordering::Relaxed);
    }

    pub fn set_locked(&self, locked: bool) {
        let sh = shared();
        if sh.locked.swap(locked, Ordering::Relaxed) == locked {
            return;
        }
        let hwnd = sh.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                let h = hwnd as HWND;
                let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
                let ex = if locked {
                    ex | WS_EX_TRANSPARENT
                } else {
                    ex & !WS_EX_TRANSPARENT
                };
                SetWindowLongPtrW(h, GWL_EXSTYLE, ex as isize);
            }
        }
        sh.dirty.store(true, Ordering::Relaxed);
    }

    /// 取走「窗口被拖动后的新位置」
    pub fn take_moved(&self) -> Option<(i32, i32)> {
        shared().moved.lock().unwrap().take()
    }

    pub fn shutdown(&self) {
        let sh = shared();
        sh.quit.store(true, Ordering::Relaxed);
        let hwnd = sh.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd as HWND,
                    WM_APP + 1,
                    0,
                    0,
                );
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------- 窗口线程

unsafe fn run_window_thread(sh: &'static Shared) -> i32 {
    let hinst = GetModuleHandleW(std::ptr::null());

    let cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: std::ptr::null_mut(),
        hCursor: cursor,
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: CLASS_NAME.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    if RegisterClassExW(&wc) == 0 {
        // 已经注册过（重复创建）也算正常
    }

    let (x, y, w, h) = {
        let p = sh.paint.lock().unwrap();
        (40, 40, p.width.max(200.0) as i32, p.height.max(60.0) as i32)
    };

    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        CLASS_NAME.as_ptr(),
        CLASS_NAME.as_ptr(),
        WS_POPUP,
        x,
        y,
        w,
        h,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinst,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        return -1;
    }
    sh.hwnd.store(hwnd as isize, Ordering::Relaxed);
    sh.alive.store(true, Ordering::Relaxed);

    if sh.visible.load(Ordering::Relaxed) {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    SetTimer(hwnd, TIMER_ID, TICK_MS, None);

    let mut msg: MSG = std::mem::zeroed();
    loop {
        let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
        if r == 0 || r == -1 {
            break;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if sh.quit.load(Ordering::Relaxed) {
            break;
        }
    }
    KillTimer(hwnd, TIMER_ID);
    0
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let sh = shared();
    match msg {
        WM_NCCREATE => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_TIMER => {
            if sh.dirty.swap(false, Ordering::Relaxed) {
                render(hwnd, sh);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !sh.locked.load(Ordering::Relaxed) {
                // 交给系统的模态拖动循环：鼠标怎么动窗口怎么动，零延迟
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as WPARAM, 0);
            }
            0
        }
        WM_LBUTTONUP => {
            report_position(hwnd, sh);
            0
        }
        WM_MOVE => {
            sh.dirty.store(true, Ordering::Relaxed);
            report_position(hwnd, sh);
            0
        }
        WM_DESTROY => {
            sh.alive.store(false, Ordering::Relaxed);
            sh.hwnd.store(0, Ordering::Relaxed);
            PostQuitMessage(0);
            0
        }
        _ if msg == WM_APP + 1 => {
            // 注意：DefWindowProc(WM_DESTROY) 只是默认处理，不会销毁窗口
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn report_position(hwnd: HWND, sh: &Shared) {
    let mut rc: RECT = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut rc) != 0 {
        *sh.moved.lock().unwrap() = Some((rc.left, rc.top));
    }
}

type GetDpiForWindowFn = unsafe extern "system" fn(HWND) -> u32;

static GET_DPI_FOR_WINDOW: OnceLock<Option<GetDpiForWindowFn>> = OnceLock::new();

/// 当前 DPI 缩放（1.0 = 96dpi）
unsafe fn dpi_scale(hwnd: HWND) -> f32 {
    // Win10 1607+ 才有 GetDpiForWindow（只解析一次），Win7 上退回 GetDeviceCaps
    let f = GET_DPI_FOR_WINDOW.get_or_init(|| {
        let user32 = LoadLibraryW(encode("user32.dll").as_ptr());
        if user32.is_null() {
            return None;
        }
        let p = GetProcAddress(user32, c"GetDpiForWindow".as_ptr() as *const u8)?;
        Some(std::mem::transmute::<_, GetDpiForWindowFn>(p))
    });
    if let Some(f) = f {
        let dpi = f(hwnd);
        if dpi > 0 {
            return dpi as f32 / 96.0;
        }
    }
    let dc = GetDC(std::ptr::null_mut());
    let dpi = GetDeviceCaps(dc, LOGPIXELSY as i32) as f32;
    ReleaseDC(std::ptr::null_mut(), dc);
    if dpi > 0.0 {
        dpi / 96.0
    } else {
        1.0
    }
}

fn encode(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------- 绘制

struct Fonts {
    map: HashMap<i32, HFONT>,
}

impl Fonts {
    fn get(&mut self, px: i32) -> HFONT {
        if let Some(f) = self.map.get(&px) {
            return *f;
        }
        let face = encode("Microsoft YaHei");
        let f = unsafe {
            windows_sys::Win32::Graphics::Gdi::CreateFontW(
                -px,
                0,
                0,
                0,
                500, // medium
                0,
                0,
                0,
                windows_sys::Win32::Graphics::Gdi::DEFAULT_CHARSET as u32,
                windows_sys::Win32::Graphics::Gdi::OUT_TT_PRECIS as u32,
                windows_sys::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS as u32,
                // 灰度抗锯齿：不能要 ClearType，否则子像素颜色会被当成 alpha
                windows_sys::Win32::Graphics::Gdi::ANTIALIASED_QUALITY as u32,
                windows_sys::Win32::Graphics::Gdi::DEFAULT_PITCH as u32,
                face.as_ptr(),
            )
        };
        self.map.insert(px, f);
        f
    }
}

unsafe fn render(hwnd: HWND, sh: &Shared) {
    let mut p = sh.paint.lock().unwrap().clone();
    // 逐字高亮：UI 线程只在 push 时给基准，这里按自己的节拍插值，30fps 平滑
    if p.playing {
        if let Some((t0, pos0)) = *sh.base.lock().unwrap() {
            let dt = t0.elapsed().as_millis() as u64;
            let cur = pos0 + dt;
            let span = p.line_end_ms.saturating_sub(p.line_start_ms);
            if span > 0 && cur >= p.line_start_ms && cur < p.line_end_ms {
                let frac = (cur - p.line_start_ms) as f32 / span as f32;
                for l in p.lines.iter_mut() {
                    if let Some((c, _)) = l.sung {
                        l.sung = Some((c, frac.clamp(0.0, 1.0)));
                    }
                }
            } else if cur >= p.line_end_ms {
                for l in p.lines.iter_mut() {
                    if let Some((c, _)) = l.sung {
                        l.sung = Some((c, 1.0));
                    }
                }
            }
        }
    }

    let scale = dpi_scale(hwnd);
    let w = ((p.width * scale).round() as i32).max(120);
    let h = ((p.height * scale).round() as i32).max(40);

    // 窗口尺寸跟着配置走（不改则不动，避免每帧 SetWindowPos）
    let mut rc: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut rc);
    if (rc.right - rc.left) != w || (rc.bottom - rc.top) != h {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOACTIVATE);
    }
    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let (cw, ch) = (client.right, client.bottom);

    let screen_dc = GetDC(std::ptr::null_mut());
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: cw,
            biHeight: -ch, // 负值 = 自上而下，行序和像素索引一致
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [std::mem::zeroed()],
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if dib.is_null() || bits.is_null() {
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        return;
    }
    let old_bmp = SelectObject(mem_dc, dib);

    let px = bits as *mut u8;
    let n = (cw * ch) as usize;
    let bg_a = p.bg_alpha;
    // 背景：预乘 alpha 的纯色（黑）。哪怕配置成 0%，也至少给 1/255：
    // 分层窗口的命中测试按 alpha 做，全 0 的区域点不到，就没法拖着走了。
    let solid_a = bg_a.max(1);
    for i in 0..n {
        let d = px.add(i * 4);
        *d = 0;
        *d.add(1) = 0;
        *d.add(2) = 0;
        *d.add(3) = solid_a;
    }

    // 画字：白字画在透明底上，GDI 只写 RGB 不写 alpha，
    // 之后把亮度当 alpha、按行决定颜色，逐像素预乘 —— 这就是分层窗口的标准做法。
    SetBkMode(mem_dc, TRANSPARENT as i32);
    SetTextColor(mem_dc, 0x00FF_FFFF);

    let mut fonts = Fonts { map: HashMap::new() };
    // 竖直居中：先算总高
    let gap = (4.0 * scale).round() as i32;
    let mut total_h = 0;
    let mut heights = Vec::new();
    for l in &p.lines {
        let lh = (l.size * scale * 1.3).round() as i32;
        heights.push(lh);
        total_h += lh + gap;
    }
    let mut y = ((ch - total_h) / 2).max(2);

    // 每行的排版信息，给像素着色阶段用
    let mut boxes: Vec<(i32, i32, i32, i32, [u8; 3], [u8; 3])> = Vec::new(); // y0,y1,x0,sung_w,unsung,sung

    for (i, l) in p.lines.iter().enumerate() {
        if l.text.trim().is_empty() {
            y += heights[i] + gap;
            continue;
        }
        let fpx = (l.size * scale).round() as i32;
        let font = fonts.get(fpx.max(8));
        let old_font = SelectObject(mem_dc, font);
        let utf16: Vec<u16> = l.text.encode_utf16().collect();
        let mut sz: SIZE = std::mem::zeroed();
        GetTextExtentPoint32W(mem_dc, utf16.as_ptr(), utf16.len() as i32, &mut sz);
        if y + heights[i] > ch {
            SelectObject(mem_dc, old_font);
            break;
        }
        let x0 = ((cw - sz.cx) / 2).max(2);
        TextOutW(mem_dc, x0, y, utf16.as_ptr(), utf16.len() as i32);
        SelectObject(mem_dc, old_font);

        let (unsung, sung, frac) = match l.sung {
            Some((sc, f)) => (l.color, sc, f),
            None => (l.color, l.color, 1.0),
        };
        let sung_w = (sz.cx as f32 * frac.clamp(0.0, 1.0)).round() as i32;
        boxes.push((y, y + heights[i], x0, sung_w, unsung, sung));
        y += heights[i] + gap;
    }

    // 覆盖率 → 预乘像素
    let surface = std::slice::from_raw_parts_mut(px, n * 4);
    for (y0, y1, x0, sung_w, unsung, sung) in boxes {
        let ys = y0.max(0);
        let ye = y1.min(ch);
        let xs = x0.max(0);
        let xe = cw;
        for yy in ys..ye {
            let row = yy as usize * cw as usize;
            for xx in xs..xe {
                let i = (row + xx as usize) * 4;
                let a = surface[i].max(surface[i + 1]).max(surface[i + 2]);
                if a == 0 {
                    continue;
                }
                let c = if xx - x0 < sung_w { sung } else { unsung };
                // 文字 over 背景（背景是黑色，预乘后 rgb 全是 0，只剩 alpha）
                let sa = a as u32;
                let ba = solid_a as u32;
                let out_a = sa + ba * (255 - sa) / 255;
                surface[i] = (c[0] as u32 * sa / 255) as u8;
                surface[i + 1] = (c[1] as u32 * sa / 255) as u8;
                surface[i + 2] = (c[2] as u32 * sa / 255) as u8;
                surface[i + 3] = out_a.min(255) as u8;
            }
        }
    }

    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let size = SIZE { cx: cw, cy: ch };
    let src = POINT { x: 0, y: 0 };
    let dst = POINT { x: rc.left, y: rc.top };
    UpdateLayeredWindow(
        hwnd,
        screen_dc,
        &dst,
        &size,
        mem_dc,
        &src,
        0 as COLORREF,
        &blend,
        ULW_ALPHA,
    );

    SelectObject(mem_dc, old_bmp);
    DeleteObject(dib);
    DeleteDC(mem_dc);
    ReleaseDC(std::ptr::null_mut(), screen_dc);
    for (_, f) in fonts.map.drain() {
        DeleteObject(f);
    }
}
