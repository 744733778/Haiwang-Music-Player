//! 全局视觉主题：深色炭灰 + 薄荷绿强调（LX Music Fluent Dark 风格）。
//!
//! 这里只定义「颜色 / 圆角 / 间距 / 字号」等视觉参数，不含任何业务逻辑。
//! 中文 CJK 字体加载仍在 main.rs::setup_fonts 中，与本模块无关。

use eframe::egui;

// ---- 强调色（薄荷绿）----
pub const MINT: egui::Color32 = egui::Color32::from_rgb(74, 210, 149); // #4AD295
pub const MINT_DIM: egui::Color32 = egui::Color32::from_rgb(48, 150, 108); // #30966C
pub const MINT_FAINT: egui::Color32 = egui::Color32::from_rgb(38, 84, 65); // 选中行底色

// ---- 背景层次 ----
pub const BG: egui::Color32 = egui::Color32::from_rgb(22, 24, 28); // #16181C 主背景
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(30, 34, 40); // #1E2228 面板
pub const ELEVATED: egui::Color32 = egui::Color32::from_rgb(40, 45, 53); // #282D35 控件/悬浮

// ---- 文字 ----
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(232, 237, 240); // 主文字
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(146, 156, 166); // 次级文字
pub const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(108, 118, 128); // 更弱的提示

/// 应用全局主题。在 eframe 创建 Context 后调用一次即可。
pub fn apply(ctx: &egui::Context) {
    // ---- 布局间距与字号 ----
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(14);
    style.spacing.indent = 16.0;
    style.spacing.interact_size = egui::vec2(32.0, 26.0);
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(21.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(14.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
    );
    ctx.set_style(style);

    // ---- 颜色 / 圆角 ----
    let mut visuals = ctx.style().visuals.clone();
    visuals.dark_mode = true;
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = ELEVATED;
    visuals.hyperlink_color = MINT;
    // 注意：不要设置 override_text_color——桌面歌词窗的逐字高亮颜色是用户可在设置里
    // 自定义的（styled_job / RichText::color 显式指定）。全局 override 会覆盖它们，
    // 导致歌词颜色失效。普通文本亮度改由 noninteractive.fg_stroke 控制（见下）。

    visuals.selection.bg_fill = MINT_FAINT;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, MINT);

    let radius = egui::CornerRadius::same(8);
    let small_radius = egui::CornerRadius::same(6);
    let no_stroke = egui::Stroke::NONE;

    // 不可交互（纯文本容器等）
    let w = &mut visuals.widgets.noninteractive;
    w.bg_fill = PANEL;
    w.weak_bg_fill = PANEL;
    w.bg_stroke = no_stroke;
    w.fg_stroke = egui::Stroke::new(1.0_f32, TEXT); // 普通标签用亮色，保证可读性
    w.corner_radius = small_radius;

    // 静止态按钮
    let w = &mut visuals.widgets.inactive;
    w.bg_fill = ELEVATED;
    w.weak_bg_fill = ELEVATED;
    w.bg_stroke = no_stroke;
    w.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    w.corner_radius = radius;

    // 鼠标悬浮
    let w = &mut visuals.widgets.hovered;
    w.bg_fill = MINT_DIM;
    w.weak_bg_fill = MINT_DIM;
    w.bg_stroke = no_stroke;
    w.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    w.expansion = 1.0;
    w.corner_radius = radius;

    // 按下
    let w = &mut visuals.widgets.active;
    w.bg_fill = MINT;
    w.weak_bg_fill = MINT;
    w.bg_stroke = no_stroke;
    w.fg_stroke = egui::Stroke::new(1.0_f32, BG);
    w.expansion = 1.0;
    w.corner_radius = radius;

    // 选中态（如 selectable_label 的当前项、标签页高亮）
    let w = &mut visuals.widgets.open;
    w.bg_fill = MINT_FAINT;
    w.weak_bg_fill = MINT_FAINT;
    w.bg_stroke = egui::Stroke::new(1.0_f32, MINT);
    w.fg_stroke = egui::Stroke::new(1.0_f32, MINT);
    w.corner_radius = radius;

    visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(120),
    };

    ctx.set_visuals(visuals);
}
