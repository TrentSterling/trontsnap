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
    /// THE PRIMARY, VERBATIM. Exactly the color that was picked, never walked,
    /// never clamped. Pure black stays pure black. This is what the picker shows,
    /// what gets persisted, and what gradient peg 0 IS (not a copy of: the same
    /// value). Do not draw small text in it; use `accent_readable` for that.
    pub accent: Color32,
    /// The primary walked onto the guaranteed ladder (step 9), for ACCENT AS INK:
    /// hyperlinks, warn text, hover strokes, focus edges. Splitting these two is
    /// the whole fix; a single field meant the readability correction had to
    /// overwrite the pick, so the pick could never survive.
    pub accent_readable: Color32,
    pub accent_dim: Color32,
    /// Label color for text sitting ON an `accent` fill, chosen by APCA against
    /// what is actually drawn, so even a midtone primary gets a readable label.
    pub on_accent: Color32,
    pub amber: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub stroke: Color32,
}

/// The house seed: the #5AD1FF cyan TrontSnap already uses in the overlay and
/// gallery. "Cyan" is no longer a hand-tuned token dump; it is simply this seed
/// run through the same ladder as every other theme, which is what makes the
/// system UNIVERSAL. One code path, no privileged default that behaves
/// differently from anything you pick yourself.
pub const HOUSE_SEED: Rgb = [90, 209, 255];

/// The default theme: the house seed through AUTO AWESOME. Kept under the old
/// name so every `unwrap_or_else(cyan_default)` fallback still reads right.
fn cyan_default() -> Tokens {
    auto_theme(&[HOUSE_SEED])
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
    t().on_accent
}

/// The accent in its INK form: guaranteed legible against the panel. Use this
/// for hyperlinks, warn text, hover strokes and focus edges. Use `t().accent`
/// (verbatim) for FILLS, and pair it with `on_accent()` for the label.
pub fn accent_ink() -> Color32 {
    t().accent_readable
}

/// The color the title-bar theme swatch paints: the USER'S pick, never the
/// readability-corrected token. `from_accent` / `enforce_readability` walk a
/// low-contrast pick toward the panel-readable version, so painting
/// `t().accent` next to a picker holding the raw value makes the two disagree
/// (TrontEQ shipped that: a yellow pick displayed as brown). Resolution
/// mirrors `resolve()`: no source hexes = the built-in look (the token IS the
/// truth), one = the exact custom pick, several = the most saturated stop,
/// which is what a palette reads as at a glance.
pub fn swatch_seed() -> Color32 {
    // Under AUTO AWESOME `accent` IS the verbatim primary: seed 0, unwalked and
    // unclamped. So the swatch, the picker and gradient peg 0 can all just read
    // it. This used to reach back into persisted settings to dig the raw pick
    // out from under a correction that had already overwritten the token; there
    // is no longer a correction to route around.
    t().accent
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
/// THE READABILITY GUARANTEE. Every token set that reaches the screen goes
/// through here, whatever produced it: a built-in, a premade, a derived accent,
/// Random, or a gradient preset that re-themed the app.
///
/// Text roles are walked (hue preserved) until APCA says they clear their floor
/// against the surfaces they actually sit on, with pure black/white as the
/// unconditional backstop. `amber` is a SEMANTIC colour (the ShareX-archive
/// marker) so it keeps its meaning and only its lightness moves.
/// AMBER ONLY, and that is the point.
///
/// This used to walk `text_primary`, `text_muted` AND `accent`. Under AUTO
/// AWESOME the first three come off the guaranteed ladder already: text is
/// PLACED at a readable tone rather than derived and hoped for, and the accent's
/// ink form is `accent_readable` (step 9). Walking them again did nothing except
/// destroy `accent`, which is exactly how a pure-red pick turned into #ff9999
/// and then overwrote itself.
///
/// `amber` is different: it is a hardcoded SEMANTIC literal (the ShareX-archive
/// marker) that never passes through the ladder, so it is precisely the kind of
/// value that survives every theme change and then vanishes on the one ground
/// that happens to match it. Hardcoded literals still need the walk.
fn enforce_readability(mut tk: Tokens) -> Tokens {
    tk.amber = c32(color::readable_against(
        rgb_of(tk.amber),
        rgb_of(tk.panel_bg),
        color::LC_MUTED,
    ));
    tk
}

pub fn set_theme(ctx: &egui::Context, tk: Tokens) {
    let tk = enforce_readability(tk);
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

    // Discord-style background wash (see `paint_gradient` below): when it's on,
    // panel/window fills go translucent by FROST (a user slider, default 0.85)
    // so the wash reads through the chrome instead of being hidden under a
    // fully opaque panel. Mirrors SpaceView's `build_visuals` dark-mode branch
    // (`(255.0 * f) as u8`) — TrontSnap has no light mode, so only that one
    // branch is needed.
    let panel_alpha: u8 =
        if crate::settings::gradient() { (255.0 * frost()) as u8 } else { 255 };
    let panel = Color32::from_rgba_unmultiplied(tk.panel_bg.r(), tk.panel_bg.g(), tk.panel_bg.b(), panel_alpha);

    // MENUS AND POPUPS STAY OPAQUE. `window_fill` is what `Frame::popup`,
    // `Frame::menu` and `egui::Window` paint with, so tying it to frost meant a
    // 0% frost setting produced alpha 0 -- context menus rendered as floating text
    // with no background at all. Panels still follow frost; overlays never do.
    v.window_fill = tk.panel_bg;
    v.panel_fill = panel;
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
    // FILL vs INK, the distinction the single-accent model could not express.
    // `selection.bg_fill` above is the VERBATIM pick (a fill), so its label is
    // `on_accent`, chosen by APCA against that exact fill. Hyperlinks and warn
    // text are INK on the panel, so they take the ladder's step-9 form.
    v.selection.stroke = Stroke::new(1.0, tk.on_accent);
    v.hyperlink_color = tk.accent_readable;
    v.warn_fg_color = tk.accent_readable;
    v.error_fg_color = Color32::from_rgb(220, 80, 60);

    let r = rounding();
    let txt = Stroke::new(1.0, tk.text_primary);

    let w = &mut v.widgets.noninteractive;
    w.bg_fill = panel;
    w.weak_bg_fill = panel;
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
    w.bg_stroke = Stroke::new(1.2, tk.accent_readable);
    w.fg_stroke = Stroke::new(1.5, tk.text_primary);
    w.rounding = r;
    w.expansion = 1.0;

    let w = &mut v.widgets.active;
    w.bg_fill = tk.accent;
    w.weak_bg_fill = tk.accent_dim;
    w.bg_stroke = Stroke::new(1.0, tk.accent_readable);
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
/// AUTO AWESOME: build the complete token set from 1..=4 seed colors.
///
/// **SEED 0 OWNS THE CHROME.** Every ground, border and text tone is a step on
/// the ladder `color::scale_from_seed` lays down from it, so picking a color
/// re-tints the WHOLE app instead of swapping three fields on a frozen navy.
/// That frozen ground is what made every custom accent look "just blue": the
/// old `from_accent` returned `..cyan_default()` and only replaced `accent`,
/// `accent_dim` and `widget_hover`.
///
/// Seeds 1.. feed the gradient ramp (see `gradient_pegs_raw`), which is why one
/// pick and four picks both produce a coherent theme.
///
/// TrontSnap is always-dark, so `dark` is pinned true here; the ladder itself
/// carries both tables, so a light mode stays a drop-in rather than a rewrite.
pub fn auto_theme(seeds: &[Rgb]) -> Tokens {
    let seed = seeds.first().copied().unwrap_or(HOUSE_SEED);
    let sc = color::scale_from_seed(seed, true);

    // Radix roles, mapped onto TrontSnap's surface names. The original hand-tuned
    // ordering (window < panel < card/header < widget < hover) is preserved, it
    // is just tones off the ladder now instead of twelve magic literals.
    let tk = Tokens {
        window_bg: c32(sc.step(1)),
        panel_bg: c32(sc.step(2)),
        card_bg: c32(sc.step(3)),
        header_strip: c32(sc.step(3)),
        widget_bg: c32(sc.step(4)),
        widget_hover: c32(sc.step(5)),
        stroke: c32(sc.step(6)),
        // THE PICK STAYS VERBATIM. Black is allowed to be black.
        accent: c32(seed),
        // Step 9 is the SOLID step: the ladder guarantees it can HOST a label,
        // which is not the same as being legible AS a label on the panel. A deep
        // purple seed lands its step 9 at Lc ~29 against step 2, and TrontSnap
        // draws real text in this token (section headers, the title bar, gallery
        // source labels), so it gets walked onto the muted-text floor. Hue is
        // preserved, so the theme still reads as itself.
        accent_readable: c32(color::readable_against(
            sc.step(9),
            sc.step(2),
            color::LC_MUTED,
        )),
        accent_dim: c32(color::mix_colors(sc.step(9), sc.step(1), 0.45)),
        on_accent: c32(color::on_color(seed, &sc)),
        text_primary: c32(sc.step(12)),
        text_muted: c32(sc.step(11)),
        // Semantic, not derived: the ShareX-archive marker keeps its meaning.
        // `enforce_readability` walks only its lightness onto this ground.
        amber: c32([255, 183, 77]),
    };
    enforce_readability(tk)
}

/// Derive tokens from a color list. Now just AUTO AWESOME: seed 0 owns the
/// chrome, the rest are extra gradient stops. Kept as an `Option` so the
/// existing `unwrap_or_else(cyan_default)` call sites are unchanged.
fn tokens_from_colors(colors: &[Rgb]) -> Option<Tokens> {
    if colors.is_empty() {
        return None;
    }
    Some(auto_theme(colors))
}

/// Single-primary entry point (the picker, premades, presets and Random all
/// funnel here). Thin wrapper so every source shares one guaranteed path.
pub fn from_accent(accent: Rgb) -> Tokens {
    auto_theme(&[accent])
}

/// Pick the most saturated color in a list — the one swatch a palette (or a
/// generated harmony/preset spread) would read as its "accent" at a glance.
/// Used by the Gradient editor's preset-sync: picking a preset also rethemes
/// the app off this swatch, so the ground and the wash never fight.
pub fn most_saturated(colors: &[Rgb]) -> Option<Rgb> {
    colors.iter().copied().max_by(|a, b| {
        color::rgb_to_hsl(*a).s.partial_cmp(&color::rgb_to_hsl(*b).s).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Reorder a palette so its most saturated swatch leads, then trim to the four
/// the gradient can hold.
///
/// SEED 0 OWNS THE CHROME now, so which color leads decides what the entire app
/// looks like. Premade palettes are authored background-first (Dracula opens on
/// #282a36, Tokyo Night on #1a1b26), and seeding on that would derive a whole
/// theme from a near-black: no hue to spread, no identity, every palette landing
/// on the same charcoal. The most saturated stop is what a palette reads as at a
/// glance, so that is the primary; the rest follow as gradient stops.
fn seeds_primary_first(colors: &[Rgb]) -> Vec<Rgb> {
    let Some(primary) = most_saturated(colors) else {
        return Vec::new();
    };
    let mut out = vec![primary];
    // Compare by value, not index: a palette may legitimately repeat a color,
    // and dropping every copy would silently shorten the ramp.
    let mut skipped = false;
    for &c in colors {
        if !skipped && c == primary {
            skipped = true;
            continue;
        }
        out.push(c);
    }
    out.truncate(4);
    out
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
    // Same rule as the premades: the most saturated roll leads, so a Random
    // theme actually LOOKS like the color it advertises instead of seeding the
    // whole app on whichever stop happened to come out darkest.
    let cols = seeds_primary_first(&cols);
    let source: Vec<String> = cols.iter().map(|&c| color::rgb_to_hex(c)).collect();
    let tokens = tokens_from_colors(&cols).unwrap_or_else(cyan_default);
    (tokens, name, source)
}

/// Look up a premade palette by name and derive tokens from it, returning the
/// source hex list too (for persistence).
pub fn premade_tokens(name: &str) -> Option<(Tokens, Vec<String>)> {
    let p = color::PREMADE_PALETTES.iter().find(|p| p.name == name)?;
    let rgb: Vec<Rgb> = p.colors.iter().filter_map(|h| color::hex_to_rgb(h)).collect();
    let seeds = seeds_primary_first(&rgb);
    let tokens = tokens_from_colors(&seeds)?;
    // Persist in SEED ORDER, not authoring order, so `resolve` rebuilds the same
    // theme next launch instead of re-seeding on whatever color came first.
    let source: Vec<String> = seeds.iter().map(|&c| color::rgb_to_hex(c)).collect();
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
    // Startup path installs a palette directly, so it needs the same guarantee
    // `set_theme` applies — otherwise the very first frame can ship unreadable
    // text and only fix itself once the user touches a theme control.
    *CURRENT.write().unwrap() = enforce_readability(resolve(&name, &source));

    // Gradient v2 knobs + frost — settings::load() already ran in main()
    // before apply(), so these reads see whatever was persisted last session.
    *GRAD_CFG.write().unwrap() = GradientCfg {
        angle_deg: crate::settings::gradient_angle(),
        intensity: crate::settings::gradient_intensity(),
        pegs: crate::settings::gradient_pegs_count(),
        harmony: crate::settings::gradient_harmony(),
        preset: crate::settings::gradient_preset(),
        custom: crate::settings::gradient_custom(),
    };
    *FROST.write().unwrap() = crate::settings::gradient_frost();
}

// ---- background gradient v2 (Discord parity) --------------------------------
//
// Ported verbatim from SpaceView's `theme.rs` (the reference implementation).
// v1 mixed every corner toward bg as one quad; v2 is a true multi-stop ramp,
// Discord-style:
//   - 2..=4 PEGS derived from the accent via colormagic harmony rules, so the
//     stops can NEVER clash (Discord's trick, our engine).
//   - DIRECTION: any angle, like Discord's Gradient Direction dial.
//   - INTENSITY: 0..1 like Discord's Color Intensity. At 1.0 the background IS
//     the pure peg ramp (panels float on top); at low values it fades to bg.
//   - END-HOLD easing: the ramp saturates to pure first/last peg over the
//     outer ~12% so the extremes read as their color instead of a blend.
//
// TrontSnap is always-dark (no light theme), so unlike SpaceView this module
// has no `dark: bool` branching anywhere below — every clamp/lift is the dark
// variant only. The function shapes (frost as its own get/set pair, presets
// used verbatim rather than lifted toward white, etc.) still mirror SpaceView
// 1:1 so a future light mode is a drop-in, not a rewrite.

#[derive(Clone, Copy, PartialEq)]
pub struct GradientCfg {
    /// Degrees; 0 = left->right, 90 = top->bottom, 135 = TL->BR diagonal.
    pub angle_deg: f32,
    /// 0..1. Discord "Color Intensity". 1.0 = pure peg colors as the ground.
    pub intensity: f32,
    /// 2..=4 color stops (harmony mode).
    pub pegs: u8,
    /// Index into color::HARMONY_RULES used to derive the pegs from the accent.
    pub harmony: u8,
    /// >= 0: index into GRADIENT_PRESETS (curated named ramps, accent ignored).
    /// -1: harmony mode (pegs derived from the live accent).
    /// -2: custom mode (the `custom` pegs below, user-picked, used verbatim).
    pub preset: i16,
    /// Manual pegs for custom mode (first `pegs` entries used).
    pub custom: [Rgb; 4],
}

impl Default for GradientCfg {
    fn default() -> Self {
        GradientCfg {
            angle_deg: 135.0,
            intensity: 0.45,
            pegs: 3,
            harmony: 0,
            preset: -1,
            custom: [[86, 204, 255], [153, 14, 165], [253, 79, 80], [37, 223, 196]],
        }
    }
}

/// Curated, named multi-stop ramps — the "Gradients of the Galaxy / chrome
/// sunset" shelf. Hand-picked hex stops (2-3 per ramp), used verbatim as pegs
/// (dark-only here; SpaceView additionally lifts these toward white for its
/// light mode, which TrontSnap doesn't have). Creative names are load-bearing;
/// nobody rolls "Preset 7".
pub const GRADIENT_PRESETS: &[(&str, &[&str])] = &[
    ("Galaxy Punch", &["#FD4F50", "#990EA5"]),
    ("Nebula Rush", &["#E71B7B", "#8324FB"]),
    ("Ultraviolet", &["#B501AA", "#FD37C8"]),
    ("Solar Flare", &["#FC4D1D", "#F1358A"]),
    ("Chrome Sunset", &["#C0C6CC", "#FFB88C", "#DE4313"]),
    ("Vaporwave", &["#FF6FD8", "#3813C2"]),
    ("Synthwave Drive", &["#DC28B2", "#2A41D2"]),
    ("Deep Space", &["#4D153C", "#B30F40"]),
    ("Golden Hour", &["#FEF528", "#B93B41"]),
    ("Blue Hour", &["#2AA9E9", "#005AFF"]),
    ("Tide Pool", &["#0DBEBA", "#00FFFB"]),
    ("Aurora Sky", &["#00C9FF", "#92FE9D"]),
    ("Toxic Slime", &["#25DFC4", "#E4E518"]),
    ("Matrix Rain", &["#00F032", "#00A0EA"]),
    ("Cherry Cola", &["#EB3349", "#F45C43"]),
    ("Berry Smoothie", &["#FF1B6B", "#45CAFF"]),
    ("Miami Nights", &["#FF0080", "#7928CA", "#4A00E0"]),
    ("Ember Fade", &["#F83600", "#F9D423"]),
    ("Concrete", &["#3A3D42", "#95989E"]),
    ("Princess", &["#FF9A9E", "#FAD0C4", "#A18CD1"]),
    ("Ocean Floor", &["#0F2027", "#2C5364", "#00B4DB"]),
    ("Firewatch", &["#CB2D3E", "#EF473A", "#2C3E50"]),
    ("Mint Chip", &["#00B09B", "#96C93D"]),
    ("Bubblegum", &["#FC5C7D", "#6A82FB"]),
    ("Night Drive", &["#0F0C29", "#302B63", "#24243E"]),
    ("Sunburn", &["#FF512F", "#F09819"]),
    ("Glacier", &["#83A4D4", "#B6FBFF"]),
];

static GRAD_CFG: LazyLock<RwLock<GradientCfg>> = LazyLock::new(|| RwLock::new(GradientCfg::default()));

/// FROST: panel opacity over the wash (0.0 = panels vanish, the background IS
/// the raw ramp, WYSIWYG with the editor preview; 1.0 = solid panels, wash
/// hidden). TrontSnap is always-dark, so this is a single value — SpaceView's
/// dark/light pair collapses to just the dark default (0.85) here.
static FROST: LazyLock<RwLock<f32>> = LazyLock::new(|| RwLock::new(0.85));

pub fn frost() -> f32 {
    *FROST.read().unwrap()
}
pub fn set_frost(v: f32) {
    *FROST.write().unwrap() = v.clamp(0.0, 1.0);
}

pub fn gradient_cfg() -> GradientCfg {
    *GRAD_CFG.read().unwrap()
}
pub fn set_gradient_cfg(cfg: GradientCfg) {
    *GRAD_CFG.write().unwrap() = cfg;
}

/// The peg colors: accent -> harmony spread. WCAG isn't a factor here (no text
/// sits on the raw ramp; panels carry the text).
/// Make the WASH itself safe to put text on.
///
/// At frost 0 and intensity 1 the panels vanish and text sits directly on the raw
/// peg ramp, which is exactly where labels were disappearing. The guarantee held
/// for panels but never covered that case.
///
/// It is not a hue problem. One ink can clear a full rainbow; what breaks it is
/// LIGHTNESS SPREAD — a light peg beside a dark one creates a midtone crossing
/// where neither white nor black reaches the floor. And because `ramp` mixes
/// linearly between pegs, mixing two dark colours can never yield a light one, so
/// clearing every PEG clears the entire ramp.
///
/// Push the pegs away from the ink until the worst one passes. Hues untouched;
/// only lightness compresses. Demand scales with how exposed the wash actually is
/// (`intensity * (1 - frost)`), so a frosted wash keeps its full colour range and
/// only a bare one gets constrained.
///
/// Polarity comes from the INK, not a dark-mode flag: "push away from whatever is
/// being drawn on top" is the real rule and needs no per-app plumbing.
fn enforce_wash_readability(pegs: Vec<Rgb>, exposure: f32, ink: Rgb) -> Vec<Rgb> {
    if exposure <= 0.05 {
        return pegs; // panels still cover the wash; leave the colours alone
    }
    let target = color::LC_TEXT_MIN * exposure.clamp(0.0, 1.0);
    // A light ink wants dark pegs beneath it, and vice versa.
    let ink_is_light = color::apca_abs(ink, [0, 0, 0]) >= color::apca_abs(ink, [255, 255, 255]);

    let mut out = pegs;
    for _ in 0..40 {
        let worst = out
            .iter()
            .map(|p| color::apca_abs(ink, *p))
            .fold(f32::MAX, f32::min);
        if worst >= target {
            break;
        }
        out = out
            .into_iter()
            .map(|p| {
                let [l, c, h] = color::rgb_to_oklch(p);
                let nl = if ink_is_light { (l - 0.025).max(0.04) } else { (l + 0.025).min(0.97) };
                color::oklch_to_rgb(nl, c, h)
            })
            .collect();
    }
    out
}

/// THE CHOKE POINT for peg colours: every source (custom slots, curated preset,
/// harmony spread) resolves through `gradient_pegs_raw` and then through
/// `enforce_wash_readability`, so no mode can produce a wash text cannot sit on.
pub fn gradient_pegs(tk: &Tokens) -> Vec<Rgb> {
    let exposure = gradient_cfg().intensity.clamp(0.0, 1.0) * (1.0 - frost());
    enforce_wash_readability(gradient_pegs_raw(tk), exposure, rgb_of(tk.text_primary))
}

fn gradient_pegs_raw(tk: &Tokens) -> Vec<Rgb> {
    let cfg = gradient_cfg();

    // Custom mode: SLOT 0 IS THE ACCENT (linked, always participates — the
    // smart-slot rule: the primary color can never be missing from the ramp).
    // Slots 1..N are the user's exact colors, no adult supervision.
    if cfg.preset == -2 {
        let n = cfg.pegs.clamp(1, 4) as usize;
        let mut pegs: Vec<Rgb> = Vec::with_capacity(n.max(2));
        pegs.push(rgb_of(tk.accent));
        if n > 1 {
            pegs.extend_from_slice(&cfg.custom[1..n]);
        }
        return mono_partner(pegs);
    }
    // NOTE for both branches below: `tk.accent` is the VERBATIM primary now, so
    // "peg 0 is the accent" finally means what it says. It used to be the
    // readability-walked token, which is why slot 0 showed a pale salmon for a
    // pure-red theme and why editing it could never give you red back.

    // Curated preset: designed stops used verbatim.
    if cfg.preset >= 0 {
        if let Some((_, hexes)) = GRADIENT_PRESETS.get(cfg.preset as usize) {
            return hexes.iter().filter_map(|h| color::hex_to_rgb(h)).collect();
        }
    }

    // Harmony mode: pegs derived from the live accent, clash-proof by rule.
    // Harmony mode: peg 0 IS the primary, verbatim; pegs 1.. are derived from it
    // by the rule and shaped to sit deep and rich on a dark ground.
    //
    // The shaping used to be applied to the WHOLE spread including its first
    // entry, which is the base itself. That silently clamped the primary into
    // L 20..42, so peg 0 disagreed with both the accent picker and the title-bar
    // swatch and a bright pick showed up muddy in its own gradient. Only the
    // DERIVED stops get shaped; the one the user actually chose is passed
    // through untouched.
    let rule = color::HARMONY_RULES[(cfg.harmony as usize) % color::HARMONY_RULES.len()];
    let base = color::rgb_to_hsl(rgb_of(tk.accent));
    let spread = color::generate_harmony(base, rule);
    let n = cfg.pegs.clamp(1, 4) as usize;
    let mut derived: Vec<Rgb> = Vec::with_capacity(n);
    derived.push(rgb_of(tk.accent));
    for h in spread.into_iter().skip(1).take(n.saturating_sub(1)) {
        // Saturation is only capped, never forced UP: a gray/black primary
        // legitimately yields a monochrome ramp (go nuts).
        let l = h.l.clamp(20.0, 42.0);
        let s = h.s.min(90.0);
        derived.push(color::hsl_to_rgb(h.h, s, l));
    }
    mono_partner(derived)
}

/// ONE-COLOR THEME MODE: a single peg gets an auto-derived deep partner so the
/// ramp is a monochrome sweep instead of a flat fill — smart slots means 1 peg
/// is a real, good-looking choice.
fn mono_partner(pegs: Vec<Rgb>) -> Vec<Rgb> {
    // ONE PEG MEANS ONE COLOUR. This used to derive a second stop so a single peg
    // still swept, but that silently turns "1 peg" into two, which shows up as an
    // unexpected extra swatch in the editor. A gradient with one peg IS a solid
    // colour choice: `ramp` returns that colour at every t, so intensity alone
    // decides how strongly it covers the ground.
    pegs
}

/// Sample the peg ramp at t in [0,1], with end-hold easing so the outer ~12%
/// on each side sits at the pure first/last peg.
fn ramp(pegs: &[Rgb], t: f32) -> Rgb {
    let t = ((t - 0.5) * 1.28 + 0.5).clamp(0.0, 1.0);
    let n = pegs.len();
    if n == 1 {
        return pegs[0];
    }
    let scaled = t * (n - 1) as f32;
    let i = (scaled.floor() as usize).min(n - 2);
    let frac = scaled - i as f32;
    color::mix_colors(pegs[i], pegs[i + 1], frac)
}

/// Paint the gradient as a fine vertex-colored grid into the background layer.
/// A grid (not one quad) because the ramp is multi-stop and runs at an
/// arbitrary angle; 16x16 vertices is still a trivially cheap single mesh.
/// Call every frame (`app.rs`'s `update()`, before `title_bar()`) while
/// `settings::gradient()` is on; the caller owns that check so a disabled
/// gradient costs nothing.
pub fn paint_gradient(ctx: &egui::Context, tk: &Tokens) {
    let rect = ctx.screen_rect();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let cfg = gradient_cfg();
    let pegs = gradient_pegs(tk);
    let bg = rgb_of(tk.window_bg);

    let a = cfg.angle_deg.to_radians();
    let (dx, dy) = (a.cos(), a.sin());
    let c = rect.center();
    // Projection half-extent of the rect onto the gradient axis.
    let half = (rect.width() * 0.5 * dx.abs()) + (rect.height() * 0.5 * dy.abs());
    let half = half.max(1.0);

    const N: usize = 16;
    let mut mesh = egui::Mesh::default();
    for gy in 0..=N {
        for gx in 0..=N {
            let p = egui::pos2(
                rect.left() + rect.width() * gx as f32 / N as f32,
                rect.top() + rect.height() * gy as f32 / N as f32,
            );
            let t = (((p.x - c.x) * dx + (p.y - c.y) * dy) / half) * 0.5 + 0.5;
            let col = color::mix_colors(bg, ramp(&pegs, t), cfg.intensity.clamp(0.0, 1.0));
            mesh.colored_vertex(p, c32(col));
        }
    }
    let w = (N + 1) as u32;
    for gy in 0..N as u32 {
        for gx in 0..N as u32 {
            let i = gy * w + gx;
            mesh.add_triangle(i, i + 1, i + w);
            mesh.add_triangle(i + 1, i + w + 1, i + w);
        }
    }

    ctx.layer_painter(egui::LayerId::background()).add(egui::Shape::mesh(mesh));
}

/// Sample the final composited ramp (pegs + end-hold easing + intensity mix
/// toward bg) at t in [0,1] — powers the editor's live preview bar.
pub fn ramp_sample(tk: &Tokens, t: f32) -> Color32 {
    let cfg = gradient_cfg();
    let pegs = gradient_pegs(tk);
    c32(color::mix_colors(rgb_of(tk.window_bg), ramp(&pegs, t), cfg.intensity.clamp(0.0, 1.0)))
}

/// The wash as actually PERCEIVED through the current frost (panel compositing
/// included) — so the editor preview can show reality, not just the raw ramp.
pub fn ramp_sample_frosted(tk: &Tokens, t: f32) -> Color32 {
    let wash = ramp_sample(tk, t);
    let f = frost();
    c32(color::mix_colors(rgb_of(wash), rgb_of(tk.panel_bg), f))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spread that covers the cases that actually broke: pure primaries, the
    /// achromatic extremes, a bright hue whose cusp is far from the ladder's
    /// solid step (yellow), and a muted near-gray.
    const SEEDS: [Rgb; 9] = [
        [255, 0, 0],
        [0, 0, 0],
        [255, 255, 255],
        [128, 128, 128],
        [255, 255, 0],
        [90, 209, 255],
        [18, 34, 30],
        [140, 60, 200],
        [0, 255, 0],
    ];

    /// THE INTENT RULE. The primary is stored and rendered EXACTLY as picked, for
    /// every seed, with no exceptions. Pure red stays pure red; pure black stays
    /// pure black. This is the guarantee the old single-accent model could not
    /// make, because its readability walk had nowhere to put the result except on
    /// top of the pick itself.
    #[test]
    fn primary_is_always_verbatim() {
        for seed in SEEDS {
            assert_eq!(
                rgb_of(from_accent(seed).accent),
                seed,
                "the primary was altered for {seed:?}"
            );
        }
    }

    /// THE "IT'S JUST BLUE" GUARD. Seed 0 owns the chrome, so two different picks
    /// must produce visibly different grounds. The old `from_accent` returned
    /// `..cyan_default()` and swapped three fields, so every theme in the app had
    /// an identical navy background no matter what was chosen.
    #[test]
    fn the_chrome_follows_the_seed() {
        let red = from_accent([255, 0, 0]);
        let cyan = from_accent([90, 209, 255]);
        for (label, a, b) in [
            ("window_bg", red.window_bg, cyan.window_bg),
            ("panel_bg", red.panel_bg, cyan.panel_bg),
            ("widget_bg", red.widget_bg, cyan.widget_bg),
        ] {
            let d = color::delta_e(rgb_of(a), rgb_of(b));
            assert!(d > 8.0, "{label} barely moved between seeds (delta_e {d:.1})");
        }
    }

    /// Grounds are TINTED, not painted: even a screaming seed yields a background
    /// you can stare at for hours. Guards the ground chroma ceiling.
    #[test]
    fn grounds_stay_calm_for_every_seed() {
        for seed in SEEDS {
            let tk = from_accent(seed);
            for (label, g) in [("window_bg", tk.window_bg), ("panel_bg", tk.panel_bg)] {
                let [_, c, _] = color::rgb_to_oklch(rgb_of(g));
                assert!(c <= 0.06, "{label} is painted, not tinted, for {seed:?} (chroma {c:.3})");
            }
        }
    }

    /// Body text is PLACED at a readable tone against every ground it can land
    /// on, for every seed. This is the ladder's core promise.
    #[test]
    fn text_reads_on_every_ground() {
        for seed in SEEDS {
            let tk = from_accent(seed);
            for (label, g) in [
                ("window_bg", tk.window_bg),
                ("panel_bg", tk.panel_bg),
                ("card_bg", tk.card_bg),
                ("widget_bg", tk.widget_bg),
            ] {
                let lc = color::apca_abs(rgb_of(tk.text_primary), rgb_of(g));
                assert!(lc >= color::LC_TEXT_MIN, "text on {label} is Lc {lc:.1} for {seed:?}");
            }
            let lc = color::apca_abs(rgb_of(tk.text_muted), rgb_of(tk.panel_bg));
            assert!(lc >= color::LC_MUTED, "muted text is Lc {lc:.1} for {seed:?}");
        }
    }

    /// The accent's INK form clears its floor on the panel even when the verbatim
    /// pick could not possibly (pure black on a near-black ground). Splitting the
    /// two is what lets the pick stay verbatim without anything going invisible.
    #[test]
    fn accent_ink_reads_where_the_pick_cannot() {
        for seed in SEEDS {
            let tk = from_accent(seed);
            let lc = color::apca_abs(rgb_of(tk.accent_readable), rgb_of(tk.panel_bg));
            assert!(lc >= color::LC_MUTED, "accent ink is Lc {lc:.1} for {seed:?}");
        }
    }

    /// A label drawn ON the verbatim accent fill stays readable, which is what
    /// makes it safe to keep using the raw pick as a fill.
    #[test]
    fn labels_read_on_the_verbatim_fill() {
        for seed in SEEDS {
            let tk = from_accent(seed);
            let lc = color::apca_abs(rgb_of(tk.on_accent), rgb_of(tk.accent));
            assert!(lc >= 45.0, "on_accent is Lc {lc:.1} against the fill for {seed:?}");
        }
    }

    /// PEG 0 IS THE PRIMARY, in both peg modes. Not linked to it, not a corrected
    /// copy of it: the same color. Harmony mode used to shape the whole spread
    /// including its first entry (which is the base itself), clamping the primary
    /// into L 20..42 so it disagreed with its own picker.
    #[test]
    fn peg_zero_is_the_primary() {
        let tk = from_accent([255, 0, 0]);
        let mut cfg = GradientCfg { pegs: 3, ..GradientCfg::default() };

        cfg.preset = -2; // custom
        set_gradient_cfg(cfg);
        assert_eq!(gradient_pegs_raw(&tk)[0], [255, 0, 0], "custom peg 0 is not the primary");

        cfg.preset = -1; // harmony
        set_gradient_cfg(cfg);
        assert_eq!(gradient_pegs_raw(&tk)[0], [255, 0, 0], "harmony peg 0 is not the primary");

        set_gradient_cfg(GradientCfg::default());
    }

    /// Premades seed on their most saturated stop, not their first. Authored
    /// background-first, they would otherwise all derive from a near-black and
    /// land on the same identity-free charcoal.
    #[test]
    fn premades_seed_on_their_identity() {
        // Dracula opens on #282a36 (near-black) and carries #ff79c6 / #bd93f9.
        let (tk, source) = premade_tokens("Dracula").expect("Dracula exists");
        let seed = rgb_of(tk.accent);
        assert_ne!(seed, [0x28, 0x2a, 0x36], "seeded on the background, not the identity");
        assert_eq!(
            color::hex_to_rgb(&source[0]),
            Some(seed),
            "persisted source must lead with the seed so resolve() rebuilds it"
        );
        assert!(color::rgb_to_hsl(seed).s > 50.0, "the seed should be the saturated stop");
    }
}
