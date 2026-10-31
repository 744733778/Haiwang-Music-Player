#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod library;
mod loudness;
mod lyrics;
mod lyrics_online;
#[cfg(windows)]
mod lyric_win32;
mod mpv;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let cfg = config::Config::load();

    #[allow(unused_mut)]
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Rust Music")
        .with_inner_size([1120.0, 720.0])
        .with_min_inner_size([900.0, 560.0]);
    // 只有非 Windows 才需要主窗口透明：那边桌面歌词还是 egui 子 viewport，
    // glutin 的 framebuffer alpha 只由主窗口的 GL config 决定。
    // Windows 上歌词窗已经换成独立的 Win32 分层窗口，主窗口保持普通不透明更稳。
    #[cfg(not(windows))]
    {
        viewport = viewport.with_transparent(true);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Rust Music",
        options,
        Box::new(move |cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cfg)))
        }),
    )
}

/// 加载一个中文字体，否则歌词/歌名会显示成方块。
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let candidates: &[&str] = if cfg!(windows) {
        &[
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/msyhbd.ttc",
            "C:/Windows/Fonts/simhei.ttf",
            "C:/Windows/Fonts/simsun.ttc",
            "C:/Windows/Fonts/Deng.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        ]
    };

    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, "cjk".to_owned());
            }
            break;
        }
    }

    ctx.set_fonts(fonts);
}
