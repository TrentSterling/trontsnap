//! TrontSnap's egui theme — ported from Trent's house style (boxel/tronteq
//! `theme.rs`) instead of default `Visuals::dark()`. One `apply(ctx)` at startup
//! installs the Rajdhani display font + a from-scratch Visuals built off a token
//! palette, so panels have depth (shadows), hover/active states glow toward the
//! accent, and every control shares one height/rounding. Accent = TrontSnap cyan.

use eframe::egui::{self, Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle};

// ---- tokens -------------------------------------------------------------

#[allow(dead_code)] // full palette kept as house-style tokens; not all read yet
pub struct Tokens {
    pub window_bg: Color32,
    pub panel_bg: Color32,
    pub header_strip: Color32,
    pub card_bg: Color32,
    pub widget_bg: Color32,
    pub widget_hover: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub amber: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub stroke: Color32,
}

/// TrontSnap "Cyan" palette: near-black blue-tinted ground, the #5AD1FF cyan
/// accent it already uses in the overlay/gallery, amber for the ShareX archive.
pub const T: Tokens = Tokens {
    window_bg: Color32::from_rgb(9, 13, 19),
    panel_bg: Color32::from_rgb(14, 21, 30),
    header_strip: Color32::from_rgb(19, 28, 40),
    card_bg: Color32::from_rgb(17, 26, 37),
    widget_bg: Color32::from_rgb(24, 35, 47),
    widget_hover: Color32::from_rgb(34, 50, 66),
    accent: Color32::from_rgb(90, 209, 255),
    accent_dim: Color32::from_rgb(44, 110, 138),
    amber: Color32::from_rgb(255, 183, 77),
    text_primary: Color32::from_rgb(216, 240, 248),
    text_muted: Color32::from_rgb(110, 150, 168),
    stroke: Color32::from_rgb(30, 42, 56),
};

// Accessors (so call sites never hardcode a Color32 — matches his house style).
pub fn card_bg() -> Color32 { T.card_bg }
pub fn stroke() -> Color32 { T.stroke }
/// Dark text to sit on the light cyan accent (readable on the accent fill).
/// Wired into `selection.stroke` (selected tab/chip text sits on the cyan fill).
/// Deliberately NOT used for `widgets.active.fg_stroke`, where the pressed-widget
/// label sits over the DARK panel and would turn dark-on-dark — see `apply()`.
pub fn on_accent() -> Color32 { T.window_bg }

pub fn rounding() -> Rounding {
    Rounding::same(6.0)
}
pub fn rounding_lg() -> Rounding {
    Rounding::same(9.0)
}

fn display_family() -> FontFamily {
    FontFamily::Name("display".into())
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "rajdhani".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/Rajdhani-Medium.ttf")),
    );
    fonts.font_data.insert(
        "rajdhani-sb".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/Rajdhani-SemiBold.ttf")),
    );
    // Rajdhani leads Proportional (built-ins stay as fallback for symbols/emoji).
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "rajdhani".to_owned());
    fonts.families.insert(
        FontFamily::Name("display".into()),
        vec!["rajdhani-sb".to_owned(), "rajdhani".to_owned()],
    );
    ctx.set_fonts(fonts);
}

/// Install the font + full Visuals/Style. Call once at startup.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    v.dark_mode = true;

    v.window_fill = T.panel_bg;
    v.panel_fill = T.panel_bg;
    v.faint_bg_color = T.header_strip;
    v.extreme_bg_color = T.window_bg;
    v.code_bg_color = T.widget_bg;

    v.window_rounding = rounding_lg();
    v.window_stroke = Stroke::new(1.0, T.stroke);
    v.menu_rounding = rounding_lg();
    // Layered drop shadows so popups/menus read as raised (flat panels were the
    // biggest "default egui" tell).
    v.popup_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 4.0),
        blur: 18.0,
        spread: 0.0,
        color: Color32::from_black_alpha(110),
    };
    v.window_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 8.0),
        blur: 28.0,
        spread: 1.0,
        color: Color32::from_black_alpha(130),
    };
    v.clip_rect_margin = 3.0;
    v.indent_has_left_vline = false;

    v.selection.bg_fill = T.accent;
    // Text color for a SELECTED SelectableLabel/tab. interact_selectable overrides
    // fg_stroke with this whenever the widget is selected — and a selected chip is
    // painted ON the bright cyan bg_fill, so this text needs to be DARK for contrast
    // (light-on-cyan washes out). This is the opposite of `widgets.active` below,
    // where the pressed-widget label sits over the DARK panel and must stay light.
    v.selection.stroke = Stroke::new(1.0, on_accent());
    v.hyperlink_color = T.accent;
    v.warn_fg_color = T.accent;
    v.error_fg_color = Color32::from_rgb(220, 80, 60);

    let r = rounding();
    let txt = Stroke::new(1.0, T.text_primary);

    let w = &mut v.widgets.noninteractive;
    w.bg_fill = T.panel_bg;
    w.weak_bg_fill = T.panel_bg;
    w.bg_stroke = Stroke::new(1.0, T.stroke);
    w.fg_stroke = txt;
    w.rounding = r;

    let w = &mut v.widgets.inactive;
    w.bg_fill = T.widget_bg;
    w.weak_bg_fill = T.widget_bg;
    w.bg_stroke = Stroke::new(1.0, T.stroke);
    w.fg_stroke = txt;
    w.rounding = r;

    let w = &mut v.widgets.hovered;
    w.bg_fill = T.widget_hover;
    w.weak_bg_fill = T.widget_hover;
    // Full accent (not the dimmed variant) so hover reads as a crisp, deliberate
    // edge on tabs/chips/buttons/menu items — everything sharing this token.
    w.bg_stroke = Stroke::new(1.2, T.accent);
    w.fg_stroke = Stroke::new(1.5, T.text_primary);
    w.rounding = r;
    w.expansion = 1.0;

    let w = &mut v.widgets.active;
    w.bg_fill = T.accent;
    w.weak_bg_fill = T.accent_dim;
    w.bg_stroke = Stroke::new(1.0, T.accent);
    // Was on_accent() (dark) — but egui only fills bg_fill for a handful of
    // widgets (e.g. a pressed Button). Checkbox/SelectableLabel/RadioButton read
    // this same fg_stroke for their label text while pressed, painted straight
    // over the (dark) panel background, not over bg_fill — that dark-on-dark is
    // the invisible-text bug. Light text stays readable in every case.
    w.fg_stroke = Stroke::new(1.0, T.text_primary);
    w.rounding = r;
    w.expansion = 1.0;

    let w = &mut v.widgets.open;
    w.bg_fill = T.widget_bg;
    w.weak_bg_fill = T.widget_bg;
    w.bg_stroke = Stroke::new(1.0, T.accent_dim);
    w.fg_stroke = txt;
    w.rounding = r;

    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 8.0);
    s.button_padding = egui::vec2(10.0, 5.0);
    s.window_margin = Margin::same(10.0);
    s.menu_margin = Margin::same(6.0);
    s.interact_size = egui::vec2(0.0, 26.0);
    s.scroll = egui::style::ScrollStyle {
        bar_width: 8.0,
        floating: false,
        ..egui::style::ScrollStyle::solid()
    };

    // Rajdhani is condensed, so nudge sizes up; headings use the SemiBold cut.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(19.0, display_family())),
        (TextStyle::Body, FontId::new(15.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(15.5, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    ctx.set_style(style);
}
