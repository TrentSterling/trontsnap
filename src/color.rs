//! Color mathematics ("colormagic"). Vendored 2026-07-11 from Boxel's
//! crates/voxel-app/src/color.rs (same author), itself a complete tested port
//! of TrontColors' color-utils.js. Powers the dynamic theme system.
//!
//! Ported from TrontColors' `color-utils.js`: HSL <-> RGB <-> hex conversions,
//! RGB <-> HSV, OKLCH, WCAG luminance/contrast, the seven harmony rules, the six
//! random palette generators, shade/tint scales, color mixing, an auto-theme
//! deriver, and the premade palette list. The HSL convention matches the JS:
//! hue in 0..360, saturation and lightness in 0..100.
//!
//! This is a deliberately complete port: some conversions (hex<->hsl, OKLCH,
//! contrast ratio, shade/tint scales, color mixing) and a few `AutoTheme` fields
//! are exposed for reuse + covered by tests even where the current UI does not
//! call every one yet, so the allow below keeps the tree warning-clean.
#![allow(dead_code)]

/// A simple 8-bit RGB triple. Mirrors `voxel_core::Color` minus alpha; kept
/// local so the math module has no dependency on the core types.
pub type Rgb = [u8; 3];

/// HSL with hue in 0..360 and saturation/lightness in 0..100 (JS convention).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

impl Hsl {
    pub fn new(h: f32, s: f32, l: f32) -> Self {
        Hsl { h, s, l }
    }
}

// ============ COLOR SPACE CONVERSIONS ============

/// HSL (h 0..360, s/l 0..100) -> RGB. Direct port of `hslToRgb`.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let s = s / 100.0;
    let l = l / 100.0;
    let k = |n: f32| (n + h / 30.0).rem_euclid(12.0);
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        l - a * (-1.0_f32).max((k(n) - 3.0).min((9.0 - k(n)).min(1.0)))
    };
    [
        (f(0.0) * 255.0).round() as u8,
        (f(8.0) * 255.0).round() as u8,
        (f(4.0) * 255.0).round() as u8,
    ]
}

/// RGB -> "#rrggbb" lowercase hex.
pub fn rgb_to_hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// HSL -> "#rrggbb" hex.
pub fn hsl_to_hex(h: f32, s: f32, l: f32) -> String {
    rgb_to_hex(hsl_to_rgb(h, s, l))
}

/// Parse "#rgb" or "#rrggbb" (with or without the leading '#') to RGB. Returns
/// None on malformed input so callers can keep the prior value.
pub fn hex_to_rgb(hex: &str) -> Option<Rgb> {
    let h = hex.trim().trim_start_matches('#');
    let full = match h.len() {
        3 => {
            let b = h.as_bytes();
            format!(
                "{}{}{}{}{}{}",
                b[0] as char, b[0] as char, b[1] as char, b[1] as char, b[2] as char, b[2] as char
            )
        }
        6 => h.to_string(),
        _ => return None,
    };
    let r = u8::from_str_radix(&full[0..2], 16).ok()?;
    let g = u8::from_str_radix(&full[2..4], 16).ok()?;
    let b = u8::from_str_radix(&full[4..6], 16).ok()?;
    Some([r, g, b])
}

/// RGB -> HSL (h 0..360, s/l 0..100), rounded like the JS.
pub fn rgb_to_hsl(rgb: Rgb) -> Hsl {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let mut h = 0.0;
    let mut s = 0.0;
    let l = (max + min) / 2.0;
    if (max - min).abs() > f32::EPSILON {
        let d = max - min;
        s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        h = if max == r {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
    }
    Hsl {
        h: (h * 360.0).round(),
        s: (s * 100.0).round(),
        l: (l * 100.0).round(),
    }
}

/// Hex -> HSL.
pub fn hex_to_hsl(hex: &str) -> Option<Hsl> {
    hex_to_rgb(hex).map(rgb_to_hsl)
}

/// RGB -> HSV (h 0..360, s/v 0..1). Used to seed the Color tab's HSV sliders.
pub fn rgb_to_hsv(rgb: Rgb) -> [f32; 3] {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max <= 0.0 { 0.0 } else { d / max };
    let mut h = 0.0;
    if d > f32::EPSILON {
        h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h *= 60.0;
        if h < 0.0 {
            h += 360.0;
        }
    }
    [h, s, v]
}

/// HSV (h 0..360, s/v 0..1) -> RGB.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let c = v * s;
    let hp = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    ]
}

// ============ OKLCH (simplified, from the JS) ============

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Hex -> OKLCH (L 0..1, C, H 0..360). Nice-to-have for future perceptual UI.
pub fn hex_to_oklch(hex: &str) -> Option<[f32; 3]> {
    let rgb = hex_to_rgb(hex)?;
    let lr = srgb_to_linear(rgb[0] as f32 / 255.0);
    let lg = srgb_to_linear(rgb[1] as f32 / 255.0);
    let lb = srgb_to_linear(rgb[2] as f32 / 255.0);
    let l = 0.412_221_46 * lr + 0.536_332_55 * lg + 0.051_445_995 * lb;
    let m = 0.211_903_5 * lr + 0.680_699_5 * lg + 0.107_396_96 * lb;
    let s = 0.088_302_46 * lr + 0.281_718_85 * lg + 0.629_978_7 * lb;
    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();
    let big_l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let bv = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;
    let c = (a * a + bv * bv).sqrt();
    let mut hdeg = bv.atan2(a) * 180.0 / std::f32::consts::PI;
    if hdeg < 0.0 {
        hdeg += 360.0;
    }
    Some([big_l, c, hdeg])
}

/// OKLCH (L 0..1, C, H 0..360) -> RGB. Inverse of `hex_to_oklch`'s pipeline:
/// OKLCH -> OKLab -> LMS -> linear sRGB -> sRGB, gamut-clamped to [0, 255]. The
/// matrices are the standard inverse of the forward transform above.
pub fn oklch_to_rgb(big_l: f32, c: f32, hdeg: f32) -> Rgb {
    let hrad = hdeg * std::f32::consts::PI / 180.0;
    let a = c * hrad.cos();
    let bv = c * hrad.sin();

    // OKLab -> nonlinear LMS (l_, m_, s_), then cube to get LMS.
    let l_ = big_l + 0.396_337_78 * a + 0.215_803_76 * bv;
    let m_ = big_l - 0.105_561_346 * a - 0.063_854_17 * bv;
    let s_ = big_l - 0.089_484_18 * a - 1.291_485_5 * bv;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    // LMS -> linear sRGB.
    let lr = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let lg = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let lb = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;

    let to_u8 = |lin: f32| (linear_to_srgb(lin).clamp(0.0, 1.0) * 255.0).round() as u8;
    [to_u8(lr), to_u8(lg), to_u8(lb)]
}

/// RGB -> OKLCH (L 0..1, C, H 0..360), the in-memory counterpart of
/// `hex_to_oklch` that skips the hex string. Used to seed the Color tab sliders.
pub fn rgb_to_oklch(rgb: Rgb) -> [f32; 3] {
    // Reuse the hex path's math via a tiny detour to keep one source of truth.
    hex_to_oklch(&rgb_to_hex(rgb)).unwrap_or([0.0, 0.0, 0.0])
}

// ============ LUMINANCE & CONTRAST (WCAG 2.1) ============

fn channel_luminance(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance of an RGB color per WCAG 2.1.
pub fn luminance(rgb: Rgb) -> f32 {
    0.2126 * channel_luminance(rgb[0])
        + 0.7152 * channel_luminance(rgb[1])
        + 0.0722 * channel_luminance(rgb[2])
}

/// WCAG contrast ratio between two colors (1.0 .. 21.0).
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let l1 = luminance(a);
    let l2 = luminance(b);
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

/// Pick a readable foreground (near-black or near-white) for a background.
pub fn contrast_color(bg: Rgb) -> Rgb {
    if luminance(bg) > 0.179 {
        [0x0f, 0x17, 0x2a]
    } else {
        [0xf8, 0xfa, 0xfc]
    }
}

// ============ HARMONY GENERATION ============

fn clamp_hsl(h: f32, s: f32, l: f32) -> Hsl {
    Hsl {
        h: h.rem_euclid(360.0),
        s: s.clamp(0.0, 100.0),
        l: l.clamp(0.0, 100.0),
    }
}

/// The seven harmony rules, in the order surfaced by the UI.
pub const HARMONY_RULES: [&str; 7] = [
    "Analogous",
    "Monochromatic",
    "Triadic",
    "Complementary",
    "Split Comp.",
    "Square",
    "Tetradic",
];

/// Generate a harmony palette from a base HSL color. Mirrors `generateHarmony`;
/// every known rule returns five colors, an unknown rule returns just the base.
pub fn generate_harmony(base: Hsl, rule: &str) -> Vec<Hsl> {
    let Hsl { h, s, l } = base;
    match rule {
        "Analogous" => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h - 30.0, s, l + 5.0),
            clamp_hsl(h + 30.0, s, l + 5.0),
            clamp_hsl(h - 45.0, s - 10.0, l + 10.0),
            clamp_hsl(h + 45.0, s - 10.0, l + 10.0),
        ],
        "Monochromatic" => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h, s - 20.0, l + 20.0),
            clamp_hsl(h, s + 10.0, l - 15.0),
            clamp_hsl(h, s - 30.0, l + 30.0),
            clamp_hsl(h, s + 20.0, l - 25.0),
        ],
        "Complementary" => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h + 180.0, s, l),
            clamp_hsl(h, s - 15.0, l + 20.0),
            clamp_hsl(h + 180.0, s - 15.0, l + 20.0),
            clamp_hsl(h + 180.0, s + 10.0, l - 10.0),
        ],
        "Split Comp." => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h + 150.0, s, l),
            clamp_hsl(h + 210.0, s, l),
            clamp_hsl(h, s - 20.0, l + 15.0),
            clamp_hsl(h + 180.0, s - 10.0, l + 10.0),
        ],
        "Triadic" => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h + 120.0, s, l),
            clamp_hsl(h + 240.0, s, l),
            clamp_hsl(h + 120.0, s - 15.0, l + 15.0),
            clamp_hsl(h + 240.0, s - 15.0, l + 15.0),
        ],
        "Square" => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h + 90.0, s, l),
            clamp_hsl(h + 180.0, s, l),
            clamp_hsl(h + 270.0, s, l),
            clamp_hsl(h + 45.0, s - 10.0, l + 10.0),
        ],
        "Tetradic" => vec![
            clamp_hsl(h, s, l),
            clamp_hsl(h + 60.0, s, l),
            clamp_hsl(h + 180.0, s, l),
            clamp_hsl(h + 240.0, s, l),
            clamp_hsl(h + 120.0, s - 10.0, l + 10.0),
        ],
        _ => vec![clamp_hsl(h, s, l)],
    }
}

// ============ RANDOM PALETTE GENERATORS ============

/// Minimal xorshift RNG so the generators need no rand dependency. Seed it from
/// the system clock at the call site for fresh palettes each click.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Avoid the zero fixed-point.
        Rng(seed | 1)
    }
    /// Seed from the wall clock (nanoseconds).
    pub fn from_clock() -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Rng::new(n)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Inclusive integer range [min, max], like the JS `randRange`.
    pub fn range(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (max - min + 1) as u64;
        min + (self.next_u64() % span) as i32
    }
}

/// The six named random-palette flavors, with their (saturation, lightness)
/// bands from the JS generators.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteKind {
    Pastel,
    Neon,
    Vintage,
    Dark,
    Earthy,
    Jewel,
}

impl PaletteKind {
    pub fn label(self) -> &'static str {
        match self {
            PaletteKind::Pastel => "Pastel",
            PaletteKind::Neon => "Neon",
            PaletteKind::Vintage => "Vintage",
            PaletteKind::Dark => "Dark",
            PaletteKind::Earthy => "Earthy",
            PaletteKind::Jewel => "Jewel",
        }
    }
    /// (s_min, s_max, l_min, l_max) band for this flavor.
    fn band(self) -> (i32, i32, i32, i32) {
        match self {
            PaletteKind::Pastel => (40, 80, 70, 90),
            PaletteKind::Neon => (85, 100, 50, 65),
            PaletteKind::Vintage => (20, 50, 40, 75),
            PaletteKind::Dark => (10, 60, 5, 25),
            PaletteKind::Earthy => (25, 55, 25, 55),
            PaletteKind::Jewel => (60, 90, 25, 45),
        }
    }
    pub const ALL: [PaletteKind; 6] = [
        PaletteKind::Pastel,
        PaletteKind::Neon,
        PaletteKind::Vintage,
        PaletteKind::Dark,
        PaletteKind::Earthy,
        PaletteKind::Jewel,
    ];
}

/// Generate `count` random colors in the given flavor's bands, as HSL. Mirrors
/// `generateRandomPalette`: a single base hue, each color jittered +/-60deg.
pub fn generate_random_palette(kind: PaletteKind, count: usize, rng: &mut Rng) -> Vec<Hsl> {
    let (smin, smax, lmin, lmax) = kind.band();
    let base_h = rng.range(0, 359);
    (0..count)
        .map(|_| {
            let h = (base_h + rng.range(-60, 60)).rem_euclid(360) as f32;
            Hsl::new(h, rng.range(smin, smax) as f32, rng.range(lmin, lmax) as f32)
        })
        .collect()
}

// ============ SHADE / TINT SCALE ============

/// Darker variants of a color toward black (lightness ramp down). Mirrors
/// `generateShades`.
pub fn generate_shades(rgb: Rgb, steps: usize) -> Vec<Rgb> {
    let Hsl { h, s, l } = rgb_to_hsl(rgb);
    (0..steps)
        .map(|i| {
            let new_l = (l / (steps as f32 - 1.0) * (steps as f32 - 1.0 - i as f32)).round();
            hsl_to_rgb(h, s, new_l)
        })
        .collect()
}

/// Lighter variants of a color toward white (lightness ramp up). Mirrors
/// `generateTints`.
pub fn generate_tints(rgb: Rgb, steps: usize) -> Vec<Rgb> {
    let Hsl { h, s, l } = rgb_to_hsl(rgb);
    (0..steps)
        .map(|i| {
            let new_l = (l + (100.0 - l) / (steps as f32 - 1.0) * i as f32).round();
            hsl_to_rgb(h, s, new_l)
        })
        .collect()
}

// ============ COLOR MIXER ============

/// Linearly mix two colors in sRGB (`ratio` 0 = a, 1 = b). Mirrors `mixColors`.
pub fn mix_colors(a: Rgb, b: Rgb, ratio: f32) -> Rgb {
    let mix = |x: u8, y: u8| (x as f32 * (1.0 - ratio) + y as f32 * ratio).round() as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

// ============ PERCEPTUAL DISTANCE ============

/// redMean-weighted perceptual RGB distance. Approximate human-perception units;
/// far cheaper than full CIEDE2000, much better than raw Euclidean (which
/// under-weights green vs human vision). Returns a distance (not squared); the
/// ordering is the same either way, so it can stand in anywhere a "nearest color"
/// comparison was using squared Euclidean.
///
/// HARD-WON: the sibling `delta_e2` in `voxel-core/src/modifier.rs` MUST mirror
/// this formula -- that crate can't depend on voxel-app, so the math is duplicated
/// there for `nearest_slot`. Keep them in sync.
pub fn delta_e(a: Rgb, b: Rgb) -> f32 {
    let rmean = (a[0] as f32 + b[0] as f32) * 0.5;
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    (((2.0 + rmean / 256.0) * dr * dr)
        + 4.0 * dg * dg
        + ((2.0 + (255.0 - rmean) / 256.0) * db * db))
        .sqrt()
}

// ============ AUTO-THEME GENERATION ============

/// A derived UI theme: backgrounds, text, and accents pulled from a palette.
/// Colors are RGB; mirrors the shape of `generateAutoTheme`'s return object.
#[derive(Clone, Debug)]
pub struct AutoTheme {
    pub is_dark: bool,
    pub bg: Rgb,
    pub surface: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub border: Rgb,
    pub primary: Rgb,
    pub primary_fg: Rgb,
    pub secondary: Rgb,
    pub secondary_fg: Rgb,
    pub accent: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub error: Rgb,
}

/// Derive a theme from a palette (needs at least two colors). Ported from
/// `generateAutoTheme`: sorts by lightness for bg/surface, by saturation for
/// primary/secondary/accent, and picks status colors by hue range with
/// defaults. Returns None for palettes shorter than two.
pub fn generate_auto_theme(colors: &[Rgb]) -> Option<AutoTheme> {
    if colors.len() < 2 {
        return None;
    }
    let hsl: Vec<(Rgb, Hsl)> = colors.iter().map(|&c| (c, rgb_to_hsl(c))).collect();

    let mut by_l = hsl.clone();
    by_l.sort_by(|a, b| a.1.l.partial_cmp(&b.1.l).unwrap_or(std::cmp::Ordering::Equal));
    let darkest = by_l[0];
    let lightest = *by_l.last().unwrap();

    let mut by_sat = hsl.clone();
    by_sat.sort_by(|a, b| b.1.s.partial_cmp(&a.1.s).unwrap_or(std::cmp::Ordering::Equal));
    let primary = by_sat[0];
    let secondary = if by_sat.len() > 1 { by_sat[1] } else { primary };
    let accent = if by_sat.len() > 2 { by_sat[2] } else { primary };

    let avg_l = hsl.iter().map(|(_, h)| h.l).sum::<f32>() / hsl.len() as f32;
    let is_dark = avg_l < 50.0;

    let bg = if is_dark { darkest.0 } else { lightest.0 };
    let surface = if is_dark {
        hsl_to_rgb(
            darkest.1.h,
            darkest.1.s.min(15.0),
            (darkest.1.l + 8.0).min(20.0),
        )
    } else {
        hsl_to_rgb(
            lightest.1.h,
            lightest.1.s.min(10.0),
            (lightest.1.l - 5.0).max(90.0),
        )
    };
    let text = if is_dark {
        [0xf0, 0xf0, 0xf5]
    } else {
        [0x11, 0x11, 0x18]
    };
    let muted = if is_dark {
        [0x8b, 0x8f, 0xa3]
    } else {
        [0x6b, 0x72, 0x80]
    };
    let border = if is_dark {
        hsl_to_rgb(darkest.1.h, 8.0, darkest.1.l + 15.0)
    } else {
        hsl_to_rgb(lightest.1.h, 8.0, lightest.1.l - 12.0)
    };

    let find_hue = |min: f32, max: f32| {
        hsl.iter()
            .find(|(_, h)| h.h >= min && h.h <= max && h.s > 30.0)
            .map(|(c, _)| *c)
    };
    let success = find_hue(90.0, 170.0).unwrap_or([0x22, 0xc5, 0x5e]);
    let warning = find_hue(30.0, 55.0).unwrap_or([0xf5, 0x9e, 0x0b]);
    let error = find_hue(340.0, 360.0)
        .or_else(|| find_hue(0.0, 15.0))
        .unwrap_or([0xef, 0x44, 0x44]);

    Some(AutoTheme {
        is_dark,
        bg,
        surface,
        text,
        muted,
        border,
        primary: primary.0,
        primary_fg: contrast_color(primary.0),
        secondary: secondary.0,
        secondary_fg: contrast_color(secondary.0),
        accent: accent.0,
        success,
        warning,
        error,
    })
}

// ============ PREMADE PALETTES ============

/// A named premade palette: a label and its hex color list. Surfaced in the
/// Color tab's palette dropdown; ported from the JS `PREMADE_PALETTES`.
pub struct PremadePalette {
    pub name: &'static str,
    pub colors: &'static [&'static str],
}

/// All 32 premade palettes (Nordic Frost through Sepia Vintage).
pub const PREMADE_PALETTES: &[PremadePalette] = &[
    PremadePalette { name: "Nordic Frost", colors: &["#2e3440", "#3b4252", "#4c566a", "#d8dee9", "#88c0d0", "#81a1c1"] },
    PremadePalette { name: "Sakura Season", colors: &["#ffe4e1", "#ffb7b2", "#ff9e9d", "#ffdac1", "#e2f0cb", "#b5ead7"] },
    PremadePalette { name: "Sunset Boulevard", colors: &["#ff7b7b", "#ffb88c", "#ffdca2", "#fff4bd", "#85a1c1", "#3f4d63"] },
    PremadePalette { name: "Deep Ocean", colors: &["#051937", "#004d7a", "#008793", "#00bf72", "#a8eb12"] },
    PremadePalette { name: "Neon Cyber", colors: &["#ff00ff", "#00ffff", "#ffff00", "#00ff00", "#120458"] },
    PremadePalette { name: "Earthy Clay", colors: &["#582f0e", "#7f4f24", "#936639", "#a68a64", "#b6ad90", "#656d4a"] },
    PremadePalette { name: "Retro Pop", colors: &["#ef476f", "#ffd166", "#06d6a0", "#118ab2", "#073b4c"] },
    PremadePalette { name: "Cotton Candy", colors: &["#ff99c8", "#fcf6bd", "#d0f4de", "#a9def9", "#e4c1f9"] },
    PremadePalette { name: "Matcha Latte", colors: &["#d6e2e9", "#f0efeb", "#b7e4c7", "#74c69d", "#52b788", "#2d6a4f"] },
    PremadePalette { name: "Berry Smoothie", colors: &["#cdb4db", "#ffc8dd", "#ffafcc", "#bde0fe", "#a2d2ff", "#7209b7"] },
    PremadePalette { name: "Desert Sands", colors: &["#e63946", "#f1faee", "#a8dadc", "#457b9d", "#1d3557"] },
    PremadePalette { name: "Lavender Dreams", colors: &["#e6e6fa", "#d8bfd8", "#dda0dd", "#da70d6", "#9932cc", "#9400d3"] },
    PremadePalette { name: "Golden Hour", colors: &["#ffbf69", "#ff9f1c", "#cbf3f0", "#2ec4b6", "#f8f9fa"] },
    PremadePalette { name: "Morning Mist", colors: &["#e0fbfc", "#c2dfe3", "#9db4c0", "#5c6b73", "#253237"] },
    PremadePalette { name: "Soft Succulent", colors: &["#cad2c5", "#84a98c", "#52796f", "#354f52", "#2f3e46"] },
    PremadePalette { name: "Peach Fuzz", colors: &["#ffbe98", "#ffcb9a", "#fecea0", "#e07a5f", "#f2cc8f", "#81b29a"] },
    PremadePalette { name: "Vintage Pastel", colors: &["#f4c2c2", "#f9ebc7", "#d4e09b", "#9cbfa7", "#a9d0d3"] },
    PremadePalette { name: "Colorful Winter", colors: &["#a8dae1", "#cbd4e8", "#e4d4e6", "#f1e4e6", "#87aeb4"] },
    PremadePalette { name: "Hades Fire", colors: &["#1a0a0a", "#8b0000", "#ff4500", "#ff8c00", "#ffd700", "#fff8dc"] },
    PremadePalette { name: "Hollow Knight", colors: &["#0f1923", "#1f3044", "#4a7a8c", "#a0c4d0", "#e8f0f2", "#f5a623"] },
    PremadePalette { name: "Celeste Sky", colors: &["#1b2838", "#2d4a6f", "#5b8fb9", "#87ceeb", "#e0f0ff", "#ff6b8a"] },
    PremadePalette { name: "Solarized Dark", colors: &["#002b36", "#073642", "#586e75", "#839496", "#93a1a1", "#268bd2"] },
    PremadePalette { name: "Dracula", colors: &["#282a36", "#44475a", "#6272a4", "#bd93f9", "#ff79c6", "#f8f8f2"] },
    PremadePalette { name: "Catppuccin Mocha", colors: &["#1e1e2e", "#313244", "#585b70", "#cdd6f4", "#f38ba8", "#a6e3a1"] },
    PremadePalette { name: "Gruvbox", colors: &["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#ebdbb2"] },
    PremadePalette { name: "Tokyo Night", colors: &["#1a1b26", "#24283b", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"] },
    PremadePalette { name: "Autumn Forest", colors: &["#2d1b0e", "#5e3a1a", "#a0522d", "#cd853f", "#daa520", "#6b8e23"] },
    PremadePalette { name: "Coral Reef", colors: &["#ff6f61", "#ffb347", "#fdfd96", "#77dd77", "#40e0d0", "#1e90ff"] },
    PremadePalette { name: "Volcanic", colors: &["#0d0d0d", "#3d0000", "#8b0000", "#ff4500", "#ffa500", "#ffe066"] },
    PremadePalette { name: "Arctic Aurora", colors: &["#011627", "#0b3d5c", "#1fa8a8", "#5eead4", "#a7f3d0", "#c7f9cc"] },
    PremadePalette { name: "Candy Shop", colors: &["#ff5d8f", "#ff8fab", "#ffb3c6", "#a0e7e5", "#b4f8c8", "#fbe7c6"] },
    PremadePalette { name: "Sepia Vintage", colors: &["#2b1d0e", "#4a3520", "#7d5a3c", "#a98467", "#d4b996", "#ede0d4"] },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rgb_roundtrip() {
        for hex in ["#000000", "#ffffff", "#ff8c00", "#268bd2", "#1e1e2e"] {
            let rgb = hex_to_rgb(hex).unwrap();
            assert_eq!(rgb_to_hex(rgb), hex);
        }
    }

    #[test]
    fn delta_e_identity_is_zero() {
        assert_eq!(delta_e([123, 45, 200], [123, 45, 200]), 0.0);
        assert_eq!(delta_e([0, 0, 0], [0, 0, 0]), 0.0);
    }

    #[test]
    fn delta_e_weights_green_over_blue() {
        // Same magnitude channel delta: green should register as a larger
        // perceptual distance than blue (the whole point of redMean weighting).
        let g = delta_e([0, 0, 0], [0, 60, 0]);
        let b = delta_e([0, 0, 0], [0, 0, 60]);
        assert!(g > b, "green delta {g} should exceed blue delta {b}");
    }

    #[test]
    fn short_hex_expands() {
        assert_eq!(hex_to_rgb("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(hex_to_rgb("fff"), Some([255, 255, 255]));
    }

    #[test]
    fn bad_hex_is_none() {
        assert_eq!(hex_to_rgb("#zzz"), None);
        assert_eq!(hex_to_rgb("#12"), None);
        assert_eq!(hex_to_rgb("nonsense"), None);
    }

    #[test]
    fn hex_hsl_roundtrip_is_close() {
        // HSL rounds to integers, so allow a small tolerance per channel after
        // a full hex -> hsl -> hex round trip.
        for hex in ["#ff8c00", "#268bd2", "#52b788", "#bd93f9", "#1d3557"] {
            let orig = hex_to_rgb(hex).unwrap();
            let hsl = rgb_to_hsl(orig);
            let back = hsl_to_rgb(hsl.h, hsl.s, hsl.l);
            for i in 0..3 {
                let d = (orig[i] as i32 - back[i] as i32).abs();
                assert!(d <= 4, "channel {i} drifted by {d} for {hex}");
            }
        }
    }

    #[test]
    fn hsv_rgb_roundtrip_is_close() {
        for rgb in [[255u8, 140, 0], [38, 139, 210], [82, 183, 136], [0, 0, 0], [255, 255, 255]] {
            let hsv = rgb_to_hsv(rgb);
            let back = hsv_to_rgb(hsv[0], hsv[1], hsv[2]);
            for i in 0..3 {
                let d = (rgb[i] as i32 - back[i] as i32).abs();
                assert!(d <= 2, "channel {i} drifted by {d}");
            }
        }
    }

    #[test]
    fn each_harmony_returns_five() {
        let base = rgb_to_hsl([240, 110, 70]);
        for rule in HARMONY_RULES {
            assert_eq!(generate_harmony(base, rule).len(), 5, "rule {rule}");
        }
        // Unknown rule falls back to just the base color.
        assert_eq!(generate_harmony(base, "Nope").len(), 1);
    }

    #[test]
    fn harmony_colors_are_in_range() {
        let base = rgb_to_hsl([200, 50, 90]);
        for rule in HARMONY_RULES {
            for c in generate_harmony(base, rule) {
                assert!((0.0..360.0).contains(&c.h));
                assert!((0.0..=100.0).contains(&c.s));
                assert!((0.0..=100.0).contains(&c.l));
            }
        }
    }

    #[test]
    fn random_palettes_have_count_and_bands() {
        let mut rng = Rng::new(12345);
        for kind in PaletteKind::ALL {
            let pal = generate_random_palette(kind, 5, &mut rng);
            assert_eq!(pal.len(), 5);
            let (smin, smax, lmin, lmax) = kind.band();
            for c in pal {
                assert!(c.s >= smin as f32 && c.s <= smax as f32, "{:?} sat", kind);
                assert!(c.l >= lmin as f32 && c.l <= lmax as f32, "{:?} light", kind);
            }
        }
    }

    #[test]
    fn shades_darken_tints_lighten() {
        let base = [120u8, 80, 200];
        let shades = generate_shades(base, 10);
        let tints = generate_tints(base, 10);
        assert_eq!(shades.len(), 10);
        assert_eq!(tints.len(), 10);
        // First shade is the base lightness, last is darkest (lowest luminance).
        assert!(luminance(*shades.last().unwrap()) <= luminance(shades[0]));
        // Tints ramp up to near-white.
        assert!(luminance(*tints.last().unwrap()) >= luminance(tints[0]));
    }

    #[test]
    fn mix_is_endpoints_and_midpoint() {
        let a = [0u8, 0, 0];
        let b = [255u8, 255, 255];
        assert_eq!(mix_colors(a, b, 0.0), a);
        assert_eq!(mix_colors(a, b, 1.0), b);
        assert_eq!(mix_colors(a, b, 0.5), [128, 128, 128]);
    }

    #[test]
    fn contrast_black_white_is_max() {
        let r = contrast_ratio([0, 0, 0], [255, 255, 255]);
        assert!((r - 21.0).abs() < 0.01, "got {r}");
        // Foreground picks dark on light bg, light on dark bg.
        assert_eq!(contrast_color([255, 255, 255]), [0x0f, 0x17, 0x2a]);
        assert_eq!(contrast_color([0, 0, 0]), [0xf8, 0xfa, 0xfc]);
    }

    #[test]
    fn premade_palettes_all_parse() {
        assert_eq!(PREMADE_PALETTES.len(), 32);
        for p in PREMADE_PALETTES {
            assert!(!p.colors.is_empty(), "{} empty", p.name);
            for hex in p.colors {
                assert!(hex_to_rgb(hex).is_some(), "{} bad hex {hex}", p.name);
            }
        }
    }

    #[test]
    fn auto_theme_needs_two_colors() {
        assert!(generate_auto_theme(&[[1, 2, 3]]).is_none());
        let theme = generate_auto_theme(&[[20, 20, 30], [240, 110, 70], [200, 220, 255]]).unwrap();
        // Average lightness here is low-ish; bg should be one of the inputs.
        assert!(theme.primary == [240, 110, 70] || theme.primary == [200, 220, 255]);
    }

    #[test]
    fn oklch_runs_and_hue_in_range() {
        let [_l, _c, h] = hex_to_oklch("#268bd2").unwrap();
        assert!((0.0..360.0).contains(&h));
        assert!(hex_to_oklch("xyzzy").is_none());
    }

    #[test]
    fn rgb_oklch_roundtrip_is_close() {
        // A spread of hues, neutrals, and the gray axis. The OKLCH transform
        // goes through cube roots + matrix products, so allow a few levels of
        // drift per channel after a full rgb -> oklch -> rgb round trip.
        for rgb in [
            [255u8, 140, 0],
            [38, 139, 210],
            [82, 183, 136],
            [189, 147, 249],
            [29, 53, 87],
            [0, 0, 0],
            [255, 255, 255],
            [128, 128, 128],
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
        ] {
            let [l, c, h] = rgb_to_oklch(rgb);
            let back = oklch_to_rgb(l, c, h);
            for i in 0..3 {
                let d = (rgb[i] as i32 - back[i] as i32).abs();
                assert!(d <= 2, "channel {i} drifted by {d} for {rgb:?} -> back {back:?}");
            }
        }
    }

    #[test]
    fn oklch_gamut_clamps_to_bytes() {
        // An impossibly saturated request must still produce a valid in-gamut
        // color (the u8 type guarantees the range; this asserts the clamp does
        // not panic and that an out-of-gamut chroma saturates a channel rather
        // than wrapping). Red at huge chroma should pin the red channel high.
        let rgb = oklch_to_rgb(0.628, 0.9, 29.23);
        assert_eq!(rgb[0], 255, "over-saturated red should clamp red to max");
    }
}
