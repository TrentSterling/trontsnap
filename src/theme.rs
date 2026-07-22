//! TrontSnap's egui theme, ported from Trent's house style (boxel/tronteq
//! `theme.rs`) instead of default `Visuals::dark()`. `apply(ctx)` at startup
//! installs the Rajdhani display font + a from-scratch Visuals built off a
//! token palette, so panels have depth (shadows), hover/active states glow
//! toward the accent, and every control shares one height/rounding.
//!
//! As of the colormagic theme system, the palette is no longer a fixed const:
//! it lives in `CURRENT`, a runtime-swappable snapshot read via `t()`. The
//! original "Cyan" values are preserved verbatim as `cyan_default()` (the
//! default look never changes), and everything else derives from a color list
//! via `crate::color`'s `generate_auto_theme` + WCAG contrast enforcement, so
//! a custom accent, a premade palette, or a random roll all stay readable.

use std::sync::{LazyLock, RwLock};

use eframe::egui::{self, Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle};

use crate::color::{self, Rgb};

// ---- tokens -------------------------------------------------------------

#[derive(Clone, Copy)]
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
/// This is the default theme, verbatim from the original hardcoded `T` const;
/// every other theme derives from a color list instead.
fn cyan_default() -> Tokens {
    Tokens {
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
    }
}

fn c32(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}
fn rgb_of(c: Color32) -> Rgb {
    [c.r(), c.g(), c.b()]
}

/// The live palette. Starts as the classic cyan look; `apply()` resolves the
/// persisted theme into this at startup, and `set_theme()` swaps it live.
static CURRENT: LazyLock<RwLock<Tokens>> = LazyLock::new(|| RwLock::new(cyan_default()));

/// Snapshot of the current theme's tokens. `Tokens` is `Copy`, so call sites
/// keep the familiar `theme::t().field` access pattern (was `theme::T.field`).
pub fn t() -> Tokens {
    *CURRENT.read().unwrap()
}

// Accessors (so call sites never hardcode a Color32 — matches his house style).
pub fn card_bg() -> Color32 {
    t().card_bg
}
pub fn stroke() -> Color32 {
    t().stroke
}
/// Readable color to sit ON the bright accent fill: dark text if the accent is
/// light, light text if the accent is dark. Wired into `selection.stroke`
/// (selected tab/chip text sits on the accent fill) — this is the "smart
/// contrast" that keeps every generated theme readable, not just Cyan.
/// Deliberately NOT used for `widgets.active.fg_stroke`, where the pressed-widget
/// label sits over the DARK panel and would turn dark-on-dark — see `build_visuals()`.
pub fn on_accent() -> Color32 {
    c32(color::contrast_color(rgb_of(t().accent)))
}

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

/// Install the font + full Visuals/Style. Call once at startup. Resolves the
/// persisted theme into `CURRENT` first (`settings::load()` already ran before
/// this in `main()`, so the stored name/source are available), then installs
/// fonts and applies visuals built from that resolved palette.
pub fn apply(ctx: &egui::Context) {
    load_from_settings();

    install_fonts(ctx);
    ctx.set_visuals(build_visuals(t()));

    let mut style = (*ctx.style()).clone();

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

/// Swap the live palette and re-apply egui visuals. Fonts/style don't change
/// per theme, so only the visuals need rebuilding.
pub fn set_theme(ctx: &egui::Context, tk: Tokens) {
    *CURRENT.write().unwrap() = tk;
    ctx.set_visuals(build_visuals(tk));
}

/// egui Visuals derived entirely from the given tokens. Factored out of
/// `apply()` so `set_theme()` can rebuild visuals without re-installing fonts.
/// Reproduces the original contrast rules exactly: `selection.stroke` uses
/// `on_accent()` (readable ON the bright accent fill), while every
/// `fg_stroke` that paints over the dark panel/widget backgrounds
/// (noninteractive/inactive/hovered/active/open) uses `text_primary` (light on
/// dark) — that was a real invisible-text bug fix, not a style choice.
fn build_visuals(tk: Tokens) -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.dark_mode = true;

    v.window_fill = tk.panel_bg;
    v.panel_fill = tk.panel_bg;
    v.faint_bg_color = tk.header_strip;
    v.extreme_bg_color = tk.window_bg;
    v.code_bg_color = tk.widget_bg;

    v.window_rounding = rounding_lg();
    v.window_stroke = Stroke::new(1.0, tk.stroke);
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

    v.selection.bg_fill = tk.accent;
    // Text color for a SELECTED SelectableLabel/tab. interact_selectable overrides
    // fg_stroke with this whenever the widget is selected — and a selected chip is
    // painted ON the bright accent bg_fill, so this text needs to be the contrast
    // color for THAT accent (dark on a light accent, light on a dark one) — the
    // smart-contrast guarantee, not a fixed dark constant.
    v.selection.stroke = Stroke::new(1.0, on_accent());
    v.hyperlink_color = tk.accent;
    v.warn_fg_color = tk.accent;
    v.error_fg_color = Color32::from_rgb(220, 80, 60);

    let r = rounding();
    let txt = Stroke::new(1.0, tk.text_primary);

    let w = &mut v.widgets.noninteractive;
    w.bg_fill = tk.panel_bg;
    w.weak_bg_fill = tk.panel_bg;
    w.bg_stroke = Stroke::new(1.0, tk.stroke);
    w.fg_stroke = txt;
    w.rounding = r;

    let w = &mut v.widgets.inactive;
    w.bg_fill = tk.widget_bg;
    w.weak_bg_fill = tk.widget_bg;
    w.bg_stroke = Stroke::new(1.0, tk.stroke);
    w.fg_stroke = txt;
    w.rounding = r;

    let w = &mut v.widgets.hovered;
    w.bg_fill = tk.widget_hover;
    w.weak_bg_fill = tk.widget_hover;
    // Full accent (not the dimmed variant) so hover reads as a crisp, deliberate
    // edge on tabs/chips/buttons/menu items — everything sharing this token.
    w.bg_stroke = Stroke::new(1.2, tk.accent);
    w.fg_stroke = Stroke::new(1.5, tk.text_primary);
    w.rounding = r;
    w.expansion = 1.0;

    let w = &mut v.widgets.active;
    w.bg_fill = tk.accent;
    w.weak_bg_fill = tk.accent_dim;
    w.bg_stroke = Stroke::new(1.0, tk.accent);
    // Was on_accent() (dark) — but egui only fills bg_fill for a handful of
    // widgets (e.g. a pressed Button). Checkbox/SelectableLabel/RadioButton read
    // this same fg_stroke for their label text while pressed, painted straight
    // over the (dark) panel background, not over bg_fill — that dark-on-dark is
    // the invisible-text bug. Light text stays readable in every case.
    w.fg_stroke = Stroke::new(1.0, tk.text_primary);
    w.rounding = r;
    w.expansion = 1.0;

    let w = &mut v.widgets.open;
    w.bg_fill = tk.widget_bg;
    w.weak_bg_fill = tk.widget_bg;
    w.bg_stroke = Stroke::new(1.0, tk.accent_dim);
    w.fg_stroke = txt;
    w.rounding = r;

    v
}

// ---- theme derivation ("colormagic") -------------------------------------

/// Derive full Tokens from a color list via colormagic: `generate_auto_theme`
/// picks bg/surface/primary, then WCAG contrast rules guarantee readable text
/// and an accent that pops on the panel — randomize all day, stays readable.
/// Mirrors TrontEQ's `Palette::from_colors`, mapped onto TrontSnap's Tokens shape.
fn tokens_from_colors(colors: &[Rgb]) -> Option<Tokens> {
    let auto = color::generate_auto_theme(colors)?;
    let dark = auto.is_dark;
    let bg = auto.bg;
    let surface = auto.surface;
    let toward: Rgb = if dark { [255, 255, 255] } else { [0, 0, 0] };

    // Primary accent must read against the panel; walk lightness until it does
    // (bounded), keeping saturation up so it doesn't wash out to gray.
    let mut accent = auto.primary;
    let mut guard = 0;
    while color::contrast_ratio(accent, surface) < 2.2 && guard < 14 {
        let h = color::rgb_to_hsl(accent);
        let l = if dark { (h.l + 6.0).min(92.0) } else { (h.l - 6.0).max(8.0) };
        accent = color::hsl_to_rgb(h.h, h.s.max(45.0), l);
        guard += 1;
    }
    let accent_dim = color::mix_colors(accent, bg, 0.45);

    // Header strip / card sit a couple of steps above the panel (dark themes)
    // or below it (light themes), header a touch further than the card.
    let header_strip = color::mix_colors(surface, toward, 0.08);
    let card_bg = color::mix_colors(surface, toward, 0.05);
    let widget_bg = color::mix_colors(surface, accent, 0.10);
    let widget_hover = color::mix_colors(widget_bg, accent, 0.14);
    let stroke = auto.border;

    // Text: WCAG 4.5 on the panel or it gets replaced outright.
    let mut text_primary = auto.text;
    if color::contrast_ratio(text_primary, surface) < 4.5 {
        text_primary = color::contrast_color(surface);
    }
    let text_muted = color::mix_colors(text_primary, surface, 0.45);

    // Amber (ShareX legend / warn color) must stay visually distinct from the
    // accent; if the derived warning hue lands too close to it, keep the
    // classic amber instead of two same-looking dots.
    let mut amber = auto.warning;
    let amber_h = color::rgb_to_hsl(amber).h;
    let accent_h = color::rgb_to_hsl(accent).h;
    let hue_gap = (amber_h - accent_h).rem_euclid(360.0);
    let hue_gap = hue_gap.min(360.0 - hue_gap);
    if hue_gap < 30.0 {
        amber = [255, 183, 77];
    }

    Some(Tokens {
        window_bg: c32(bg),
        panel_bg: c32(surface),
        header_strip: c32(header_strip),
        card_bg: c32(card_bg),
        widget_bg: c32(widget_bg),
        widget_hover: c32(widget_hover),
        accent: c32(accent),
        accent_dim: c32(accent_dim),
        amber: c32(amber),
        text_primary: c32(text_primary),
        text_muted: c32(text_muted),
        stroke: c32(stroke),
    })
}

/// "Your accent color on the standard dark UI": start from `cyan_default()`
/// (the proven dark ground) and only swap the accent-derived fields. Text
/// stays put since it's already readable on that dark ground.
pub fn from_accent(accent: Rgb) -> Tokens {
    let base = cyan_default();
    let panel = rgb_of(base.panel_bg);

    let mut chosen = accent;
    let mut guard = 0;
    while color::contrast_ratio(chosen, panel) < 2.2 && guard < 14 {
        let h = color::rgb_to_hsl(chosen);
        // The standard ground is dark, so lightening is what gains contrast.
        let l = (h.l + 6.0).min(92.0);
        chosen = color::hsl_to_rgb(h.h, h.s.max(45.0), l);
        guard += 1;
    }

    let accent_dim = color::mix_colors(chosen, rgb_of(base.window_bg), 0.45);
    let widget_hover = color::mix_colors(rgb_of(base.widget_bg), chosen, 0.14);

    Tokens { accent: c32(chosen), accent_dim: c32(accent_dim), widget_hover: c32(widget_hover), ..base }
}

/// Roll a new theme: random flavor palette, random harmony spread, or a
/// random premade — all funneled through the same contrast-safe deriver.
/// Returns the tokens plus a display name and the source hex list, so the
/// caller can persist both via `settings::set_theme`.
pub fn randomize() -> (Tokens, String, Vec<String>) {
    let mut rng = color::Rng::from_clock();
    let pick = rng.range(0, 2);
    let (name, cols): (String, Vec<Rgb>) = match pick {
        0 => {
            let kind = color::PaletteKind::ALL[rng.range(0, 5) as usize];
            let hsl_cols = color::generate_random_palette(kind, 5, &mut rng);
            let rgb: Vec<Rgb> = hsl_cols.iter().map(|h| color::hsl_to_rgb(h.h, h.s, h.l)).collect();
            (format!("Random {}", kind.label()), rgb)
        }
        1 => {
            let base = color::Hsl::new(
                rng.range(0, 359) as f32,
                rng.range(55, 95) as f32,
                rng.range(28, 62) as f32,
            );
            let rule = color::HARMONY_RULES[rng.range(0, 6) as usize];
            let hsl_cols = color::generate_harmony(base, rule);
            let rgb: Vec<Rgb> = hsl_cols.iter().map(|h| color::hsl_to_rgb(h.h, h.s, h.l)).collect();
            (format!("Random {rule}"), rgb)
        }
        _ => {
            let n = color::PREMADE_PALETTES.len() as i32;
            let p = &color::PREMADE_PALETTES[rng.range(0, n - 1) as usize];
            let rgb: Vec<Rgb> = p.colors.iter().filter_map(|h| color::hex_to_rgb(h)).collect();
            (p.name.to_string(), rgb)
        }
    };
    let source: Vec<String> = cols.iter().map(|&c| color::rgb_to_hex(c)).collect();
    let tokens = tokens_from_colors(&cols).unwrap_or_else(cyan_default);
    (tokens, name, source)
}

/// Look up a premade palette by name and derive tokens from it, returning the
/// source hex list too (for persistence).
pub fn premade_tokens(name: &str) -> Option<(Tokens, Vec<String>)> {
    let p = color::PREMADE_PALETTES.iter().find(|p| p.name == name)?;
    let rgb: Vec<Rgb> = p.colors.iter().filter_map(|h| color::hex_to_rgb(h)).collect();
    let tokens = tokens_from_colors(&rgb)?;
    let source: Vec<String> = p.colors.iter().map(|s| s.to_string()).collect();
    Some((tokens, source))
}

/// Resolve a persisted theme: "Cyan" is the hardcoded built-in, a single
/// source color is the accent-on-dark-ground path, two or more source colors
/// rebuild via colormagic, and no source at all falls back to the premade
/// list by name (or Cyan if that name is unknown too).
pub fn resolve(name: &str, source: &[String]) -> Tokens {
    if name == "Cyan" {
        return cyan_default();
    }
    if source.len() == 1 {
        if let Some(rgb) = color::hex_to_rgb(&source[0]) {
            return from_accent(rgb);
        }
    } else if source.len() >= 2 {
        let rgb: Vec<Rgb> = source.iter().filter_map(|h| color::hex_to_rgb(h)).collect();
        return tokens_from_colors(&rgb).unwrap_or_else(cyan_default);
    }
    premade_tokens(name).map(|(tk, _)| tk).unwrap_or_else(cyan_default)
}

/// Resolve the persisted theme name/source (written by `settings::set_theme`)
/// into `CURRENT`. Called once at the top of `apply()`, after `settings::load()`
/// already ran in `main()`.
fn load_from_settings() {
    let name = crate::settings::theme_name();
    let source = crate::settings::theme_source();
    *CURRENT.write().unwrap() = resolve(&name, &source);
}
