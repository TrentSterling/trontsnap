/*!
 * TRONT SYSTEM - guaranteed-readable theming for tront.xyz
 *
 * Plain script, no modules, no build step: works from file:// and from Pages.
 * Attaches window.Tront. Does NOT touch the existing assets/js/tront-theme.js.
 *
 * Ported from C:\Github\SpaceView\src\color.rs, the same engine vendored across
 * TrontEQ, TrontSnap, Boxel and SpaceView.
 *
 *     THE THING CONTROLS HUE. THE SYSTEM CONTROLS LIGHTNESS.
 *
 * Contrast is APCA, not WCAG 2.1. WCAG is known-wrong in dark mode: it passes
 * light-grey-on-black that reads as mud. APCA is polarity aware.
 *
 * 12 functional steps (Radix vocabulary), each with exactly one job:
 *   1 app background     5 active UI fill    9  solid (the accent)
 *   2 subtle background  6 subtle border     10 solid hover
 *   3 UI element fill    7 border            11 low-contrast text
 *   4 hover UI fill      8 border hover      12 high-contrast text
 */
(function (root) {
  'use strict';

  var clamp = function (v, lo, hi) { return Math.min(hi, Math.max(lo, v)); };
  var s2l = function (c) { return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); };
  var l2s = function (c) { return c <= 0.0031308 ? c * 12.92 : 1.055 * Math.pow(c, 1 / 2.4) - 0.055; };

  // ---------------------------------------------------------------- OKLCH

  function rgbToOklch(rgb) {
    var lr = s2l(rgb[0] / 255), lg = s2l(rgb[1] / 255), lb = s2l(rgb[2] / 255);
    var l = 0.41222146 * lr + 0.53633255 * lg + 0.051445995 * lb;
    var m = 0.2119035 * lr + 0.6806995 * lg + 0.10739696 * lb;
    var s = 0.08830246 * lr + 0.28171885 * lg + 0.6299787 * lb;
    var l_ = Math.cbrt(l), m_ = Math.cbrt(m), s_ = Math.cbrt(s);
    var L = 0.21045426 * l_ + 0.7936178 * m_ - 0.004072047 * s_;
    var a = 1.9779985 * l_ - 2.4285922 * m_ + 0.4505937 * s_;
    var b = 0.025904037 * l_ + 0.78277177 * m_ - 0.80867577 * s_;
    var H = Math.atan2(b, a) * 180 / Math.PI;
    if (H < 0) H += 360;
    return [L, Math.sqrt(a * a + b * b), H];
  }

  function oklchToRgb(L, C, Hdeg) {
    var h = Hdeg * Math.PI / 180, a = C * Math.cos(h), b = C * Math.sin(h);
    var l_ = L + 0.39633778 * a + 0.21580376 * b;
    var m_ = L - 0.105561346 * a - 0.06385417 * b;
    var s_ = L - 0.08948418 * a - 1.2914855 * b;
    var l = l_ * l_ * l_, m = m_ * m_ * m_, s = s_ * s_ * s_;
    var lr = 4.0767417 * l - 3.3077116 * m + 0.23096994 * s;
    var lg = -1.268438 * l + 2.6097574 * m - 0.34131938 * s;
    var lb = -0.0041960863 * l - 0.7034186 * m + 1.7076147 * s;
    var u8 = function (x) { return Math.round(clamp(l2s(x), 0, 1) * 255); };
    return [u8(lr), u8(lg), u8(lb)];
  }

  // ---------------------------------------------------------------- HSL

  function rgbToHsl(rgb) {
    var r = rgb[0] / 255, g = rgb[1] / 255, b = rgb[2] / 255;
    var mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2;
    if (mx === mn) return [0, 0, l * 100];
    var d = mx - mn, s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn), h;
    if (mx === r) h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
    else if (mx === g) h = ((b - r) / d + 2) / 6;
    else h = ((r - g) / d + 4) / 6;
    return [h * 360, s * 100, l * 100];
  }

  function hslToRgb(h, s, l) {
    h = ((h % 360) + 360) % 360 / 360;
    s = clamp(s, 0, 100) / 100; l = clamp(l, 0, 100) / 100;
    if (s === 0) { var v = Math.round(l * 255); return [v, v, v]; }
    var q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
    var hue = function (t) {
      if (t < 0) t += 1; if (t > 1) t -= 1;
      if (t < 1 / 6) return p + (q - p) * 6 * t;
      if (t < 1 / 2) return q;
      if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
      return p;
    };
    return [Math.round(hue(h + 1 / 3) * 255), Math.round(hue(h) * 255), Math.round(hue(h - 1 / 3) * 255)];
  }

  // ---------------------------------------------------------------- APCA (SA98G)

  function apcaY(rgb) {
    var f = function (c) { return Math.pow(c / 255, 2.4); };
    return 0.2126729 * f(rgb[0]) + 0.7151522 * f(rgb[1]) + 0.071275 * f(rgb[2]);
  }

  function apcaLc(text, bg) {
    var soft = function (y) { return y < 0.022 ? y + Math.pow(0.022 - y, 1.414) : y; };
    var yt = soft(apcaY(text)), yb = soft(apcaY(bg));
    if (Math.abs(yb - yt) < 0.0005) return 0;
    var sapc;
    if (yb > yt) {
      sapc = (Math.pow(yb, 0.56) - Math.pow(yt, 0.57)) * 1.14;
      return sapc < 0.1 ? 0 : (sapc - 0.027) * 100;
    }
    sapc = (Math.pow(yb, 0.65) - Math.pow(yt, 0.62)) * 1.14;
    return sapc > -0.1 ? 0 : (sapc + 0.027) * 100;
  }

  var apcaAbs = function (t, b) { return Math.abs(apcaLc(t, b)); };

  var LC_TEXT = 90, LC_TEXT_MIN = 75, LC_MUTED = 60, LC_NONTEXT = 30;

  // ---------------------------------------------------------------- the ladder

  var L_DARK = [0.17, 0.21, 0.25, 0.29, 0.33, 0.38, 0.44, 0.53, 0.62, 0.68, 0.77, 0.95];
  var L_LIGHT = [0.99, 0.975, 0.95, 0.925, 0.9, 0.87, 0.83, 0.76, 0.62, 0.56, 0.52, 0.24];
  var C_DARK = [0.35, 0.42, 0.5, 0.55, 0.58, 0.55, 0.52, 0.55, 1.0, 1.0, 0.3, 0.16];
  var C_LIGHT = [0.3, 0.38, 0.45, 0.5, 0.55, 0.55, 0.55, 0.6, 1.0, 1.0, 0.35, 0.22];
  var C_GROUND_CEIL = 0.045;
  /** Ground ceiling when the gradient is off and the ground is the only colour. */
  var C_GROUND_CEIL_FLAT = 0.11;
  var C_CEIL = 0.33;

  /** Is this OKLCH triple inside sRGB without clipping? */
  function inGamut(L, C, hdeg) {
    var rad = hdeg * Math.PI / 180, a = C * Math.cos(rad), b = C * Math.sin(rad);
    var l_ = L + 0.39633778 * a + 0.21580376 * b;
    var m_ = L - 0.105561346 * a - 0.06385417 * b;
    var s_ = L - 0.08948418 * a - 1.2914855 * b;
    var l = l_ * l_ * l_, m = m_ * m_ * m_, s = s_ * s_ * s_;
    var lr = 4.0767417 * l - 3.3077116 * m + 0.23096994 * s;
    var lg = -1.268438 * l + 2.6097574 * m - 0.34131938 * s;
    var lb = -0.0041960863 * l - 0.7034186 * m + 1.7076147 * s;
    var e = 0.001;
    return lr >= -e && lr <= 1 + e && lg >= -e && lg <= 1 + e && lb >= -e && lb <= 1 + e;
  }

  /**
   * The lightness at which a hue reaches its maximum chroma: the gamut cusp.
   *
   * This is why yellow used to come out brown. The ladder puts the solid step at
   * L 0.62, which is near blue's cusp but far below yellow's (~0.86). Yellow
   * forced to 0.62 IS brown; no amount of chroma rescues it. Bright hues have to
   * be allowed to sit where they actually live.
   *
   * The ladder still owns structure: only the two SOLID steps follow the cusp,
   * and their label is chosen by APCA afterwards, so a bright yellow button
   * simply gets dark text the way a real one does.
   */
  var cuspCache = {};
  function cuspLightness(hdeg) {
    var key = Math.round(hdeg);
    if (cuspCache[key] !== undefined) return cuspCache[key];
    var bestL = 0.62, bestC = -1;
    for (var L = 0.30; L <= 0.96; L += 0.02) {
      var lo = 0, hi = 0.4;
      for (var i = 0; i < 14; i++) {
        var mid = (lo + hi) / 2;
        if (inGamut(L, mid, hdeg)) lo = mid; else hi = mid;
      }
      if (lo > bestC) { bestC = lo; bestL = L; }
    }
    cuspCache[key] = bestL;
    return bestL;
  }

  function walkToTarget(startL, startC, h, bg, target, floor, dark) {
    var l = startL, c = startC, best = oklchToRgb(l, c, h), bestLc = apcaAbs(best, bg);
    if (bestLc >= target) return best;
    for (var i = 0; i < 48; i++) {
      l = dark ? Math.min(l + 0.015, 1) : Math.max(l - 0.015, 0);
      c *= 0.94;
      var cand = oklchToRgb(l, c, h), lc = apcaAbs(cand, bg);
      if (lc > bestLc) { bestLc = lc; best = cand; }
      if (lc >= target) return cand;
    }
    if (bestLc < floor) {
      var ex = dark ? [255, 255, 255] : [0, 0, 0];
      if (apcaAbs(ex, bg) > bestLc) return ex;
    }
    return best;
  }

  function scaleFromSeed(seed, dark) {
    var o = rgbToOklch(seed), cSeed = o[1], h = o[2];
    var ls = dark ? L_DARK : L_LIGHT, cs = dark ? C_DARK : C_LIGHT;
    var steps = [], i;

    // With the gradient ON the wash carries the colour, so grounds stay a whisper.
    // With it OFF there is nothing else to carry it, and a yellow theme reads as
    // "dark grey with a yellow button". Let the ground hold real hue instead.
    var flat = (typeof gradCfg !== 'undefined' && gradCfg && !gradCfg.enabled);
    var groundCeil = flat ? C_GROUND_CEIL_FLAT : C_GROUND_CEIL;

    // Raising the ceiling alone does nothing: at L 0.17 yellow cannot HOLD much
    // chroma, so the ground stayed near-black. Colour lives near the hue's cusp,
    // so the ground has to move lightness toward it to carry any hue at all.
    // A small nudge only, because this is still a ground you stare at for hours.
    var cuspG = flat ? cuspLightness(h) : 0;

    for (i = 0; i < 12; i++) {
      var c = cSeed * cs[i];
      var lStep = ls[i];
      if (i < 3) {
        c = Math.min(c, groundCeil);
        if (flat) lStep = ls[i] + (cuspG - ls[i]) * 0.14;
      }
      steps.push(oklchToRgb(lStep, Math.min(c, C_CEIL), h));
    }

    // 9/10 are FILLS that host labels: keep them out of the murderous midtones
    // Pull the solid steps toward this hue's own chroma peak first. Without
    // this, every hue is judged by blue's ladder and the bright ones (yellow,
    // lime, cyan, orange) come out muddy: yellow at L 0.62 is brown.
    var cusp = cuspLightness(h);
    [8, 9].forEach(function (k) {
      var cc = Math.min(cSeed * cs[k], C_CEIL);
      var l = ls[k] + (cusp - ls[k]) * 0.72;
      // step 10 stays a shade off step 9 so hover is still visible
      if (k === 9) l = dark ? Math.min(l + 0.06, 0.97) : Math.max(l - 0.06, 0.12);
      for (var n = 0; n < 24; n++) {
        var cand = oklchToRgb(l, cc, h);
        if (Math.max(apcaAbs([255, 255, 255], cand), apcaAbs([0, 0, 0], cand)) >= LC_MUTED) break;
        l = dark ? Math.max(l - 0.02, 0.3) : Math.min(l + 0.02, 0.8);
      }
      steps[k] = oklchToRgb(l, cc, h);
    });

    var ground = steps[1], worst = steps[4], t, m, now;

    t = walkToTarget(ls[11], Math.min(cSeed * cs[11], C_CEIL), h, ground, LC_TEXT, LC_TEXT_MIN, dark);
    if (apcaAbs(t, worst) < LC_TEXT_MIN) {
      now = rgbToOklch(t);
      t = walkToTarget(now[0], now[1], h, worst, LC_TEXT_MIN, LC_TEXT_MIN, dark);
    }
    steps[11] = t;

    m = walkToTarget(ls[10], Math.min(cSeed * cs[10], C_CEIL), h, ground, LC_MUTED, LC_MUTED, dark);
    if (apcaAbs(m, worst) < LC_MUTED) {
      now = rgbToOklch(m);
      m = walkToTarget(now[0], now[1], h, worst, LC_MUTED, LC_MUTED, dark);
    }
    steps[10] = m;

    return { steps: steps, dark: dark };
  }

  /**
   * Make ANY colour safe as ink on a ground, hue preserved. Hardcoded literals
   * (a semantic red, a fixed amber) never pass through the ladder, so they are
   * exactly the values that survive every theme change and then vanish.
   */
  function readableAgainst(fg, bg, target) {
    var best = fg, bestLc = apcaAbs(fg, bg);
    if (bestLc >= target) return fg;
    // direction chosen by APCA, not a luminance threshold: on a midtone ground
    // the threshold guesses wrong and walks toward the WORSE extreme
    var darkWins = apcaAbs([0, 0, 0], bg) >= apcaAbs([255, 255, 255], bg);
    var hsl = rgbToHsl(fg), l = hsl[2];
    for (var i = 0; i < 44; i++) {
      l = darkWins ? Math.max(l - 2.5, 0) : Math.min(l + 2.5, 100);
      var cand = hslToRgb(hsl[0], hsl[1], l), lc = apcaAbs(cand, bg);
      if (lc > bestLc) { bestLc = lc; best = cand; }
      if (lc >= target) return cand;
    }
    if (bestLc < target) {
      var ex = darkWins ? [0, 0, 0] : [255, 255, 255];
      if (apcaAbs(ex, bg) > bestLc) return ex;
    }
    return best;
  }

  /** Text to draw ON an arbitrary fill. This is what fixes the gradient button. */
  function onColor(fill, scale) {
    var hi = scale.steps[11], lo = scale.steps[0];
    var cand = apcaAbs(hi, fill) >= apcaAbs(lo, fill) ? hi : lo;
    if (apcaAbs(cand, fill) >= LC_TEXT_MIN) return cand;
    return apcaAbs([255, 255, 255], fill) >= apcaAbs([0, 0, 0], fill) ? [255, 255, 255] : [0, 0, 0];
  }

  // ---------------------------------------------------------------- css

  var toHex = function (rgb) {
    return '#' + rgb.map(function (c) {
      return clamp(Math.round(c), 0, 255).toString(16).padStart(2, '0');
    }).join('');
  };

  function hexToRgb(hex) {
    var h = String(hex).trim().replace(/^#/, '');
    if (h.length === 3) h = h.split('').map(function (c) { return c + c; }).join('');
    if (!/^[0-9a-fA-F]{6}$/.test(h)) return null;
    return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
  }

  var STEP_VARS = ['--bg', '--panel', '--elem', '--elem-hover', '--elem-active',
    '--line-subtle', '--line', '--line-hover', '--solid', '--solid-hover', '--muted', '--text'];

  var SEMANTIC = { '--danger': [220, 80, 60], '--warn': [235, 170, 60], '--ok': [70, 190, 120] };

  /**
   * The single choke point. Nothing else may set these properties, which is
   * exactly why no theme can produce unreadable text.
   */
  function applyTheme(seed, dark, el) {
    var target = el || document.documentElement;
    var rgb = (typeof seed === 'string' ? hexToRgb(seed) : seed) || [86, 204, 255];
    var scale = scaleFromSeed(rgb, dark);
    var st = scale.steps;

    STEP_VARS.forEach(function (name, i) { target.style.setProperty(name, toHex(st[i])); });

    // ink vs fill is a real distinction: --solid is a FILL that hosts labels,
    // --accent is the same hue used as INK on the panel
    target.style.setProperty('--on-solid', toHex(onColor(st[8], scale)));
    target.style.setProperty('--accent', toHex(readableAgainst(st[8], st[1], LC_MUTED)));
    target.style.setProperty('--accent-on-bg', toHex(readableAgainst(st[8], st[0], LC_MUTED)));
    target.style.setProperty('--seed', toHex(rgb));

    Object.keys(SEMANTIC).forEach(function (k) {
      target.style.setProperty(k, toHex(readableAgainst(SEMANTIC[k], st[1], LC_MUTED)));
    });

    target.setAttribute('data-mode', dark ? 'dark' : 'light');
    root.Tront.scale = scale;
    return scale;
  }

  /**
   * Per-item hue, the games-page DNA. Give an element its own colour and get
   * back a guaranteed-readable ink, border and fill for it.
   */
  function applyItemHue(el, hue, scale) {
    var sc = scale || root.Tront.scale;
    var rgb = (typeof hue === 'string' ? hexToRgb(hue) : hue);
    if (!rgb || !sc) return;
    el.style.setProperty('--item', toHex(rgb));
    el.style.setProperty('--item-ink', toHex(readableAgainst(rgb, sc.steps[1], LC_MUTED)));
    el.style.setProperty('--item-line', toHex(readableAgainst(rgb, sc.steps[1], LC_NONTEXT)));
    el.style.setProperty('--item-on', toHex(onColor(rgb, sc)));
  }

  // ================================================================ GRADIENT v2
  //
  // Faithful port of SpaceView src/theme.rs. Same knobs as the egui fleet:
  //   PEGS       1..4 stops, derived from the accent by harmony rule so they
  //              can never clash. Slot 0 is ALWAYS the accent (smart slots:
  //              the primary colour can never be missing from the ramp).
  //   HARMONY    which spread rule derives the other pegs.
  //   DIRECTION  any angle. 0 = left to right, 135 = top-left to bottom-right.
  //   INTENSITY  0..1. At 1.0 the ground IS the peg ramp; at low values it
  //              fades back toward --bg.
  //   FROST      panel opacity over the wash, asymmetric per mode because white
  //              bleaches colour and dark preserves it.
  //   END-HOLD   the ramp saturates to the pure first/last peg over the outer
  //              ~12%, so the extremes read as their colour instead of a blend.

  var HARMONY_RULES = ['Analogous','Monochromatic','Triadic','Complementary',
                       'Split Comp.','Square','Tetradic'];

  function clampHsl(h, s, l) {
    return [((h % 360) + 360) % 360, clamp(s, 0, 100), clamp(l, 0, 100)];
  }

  function generateHarmony(base, rule) {
    var h = base[0], s = base[1], l = base[2];
    switch (rule) {
      case 'Analogous': return [clampHsl(h,s,l), clampHsl(h-30,s,l+5), clampHsl(h+30,s,l+5),
                                clampHsl(h-45,s-10,l+10), clampHsl(h+45,s-10,l+10)];
      case 'Monochromatic': return [clampHsl(h,s,l), clampHsl(h,s-20,l+20), clampHsl(h,s+10,l-15),
                                clampHsl(h,s-30,l+30), clampHsl(h,s+20,l-25)];
      case 'Complementary': return [clampHsl(h,s,l), clampHsl(h+180,s,l), clampHsl(h,s-15,l+20),
                                clampHsl(h+180,s-15,l+20), clampHsl(h+180,s+10,l-10)];
      case 'Split Comp.': return [clampHsl(h,s,l), clampHsl(h+150,s,l), clampHsl(h+210,s,l),
                                clampHsl(h,s-20,l+15), clampHsl(h+180,s-10,l+10)];
      case 'Triadic': return [clampHsl(h,s,l), clampHsl(h+120,s,l), clampHsl(h+240,s,l),
                                clampHsl(h+120,s-15,l+15), clampHsl(h+240,s-15,l+15)];
      case 'Square': return [clampHsl(h,s,l), clampHsl(h+90,s,l), clampHsl(h+180,s,l),
                                clampHsl(h+270,s,l), clampHsl(h+45,s-10,l+10)];
      case 'Tetradic': return [clampHsl(h,s,l), clampHsl(h+60,s,l), clampHsl(h+180,s,l),
                                clampHsl(h+240,s,l), clampHsl(h+120,s-10,l+10)];
      default: return [clampHsl(h,s,l)];
    }
  }

  /** The curated shelf. Creative names are load-bearing: nobody rolls "Preset 7". */
  var GRADIENT_PRESETS = [
    ['Galaxy Punch',   ['#FD4F50','#990EA5']],
    ['Nebula Rush',    ['#E71B7B','#8324FB']],
    ['Ultraviolet',    ['#B501AA','#FD37C8']],
    ['Solar Flare',    ['#FC4D1D','#F1358A']],
    ['Chrome Sunset',  ['#C0C6CC','#FFB88C','#DE4313']],
    ['Vaporwave',      ['#FF6FD8','#3813C2']],
    ['Synthwave Drive',['#DC28B2','#2A41D2']],
    ['Deep Space',     ['#4D153C','#B30F40']],
    ['Golden Hour',    ['#FEF528','#B93B41']],
    ['Blue Hour',      ['#2AA9E9','#005AFF']],
    ['Tide Pool',      ['#0DBEBA','#00FFFB']],
    ['Aurora Sky',     ['#00C9FF','#92FE9D']],
    ['Toxic Slime',    ['#25DFC4','#E4E518']],
    ['Matrix Rain',    ['#00F032','#00A0EA']],
    ['Cherry Cola',    ['#EB3349','#F45C43']],
    ['Berry Smoothie', ['#FF1B6B','#45CAFF']],
    ['Miami Nights',   ['#FF0080','#7928CA','#4A00E0']],
    ['Ember Fade',     ['#F83600','#F9D423']],
    ['Concrete',       ['#3A3D42','#95989E']],
    ['Princess',       ['#FF9A9E','#FAD0C4','#A18CD1']],
    ['Ocean Floor',    ['#0F2027','#2C5364','#00B4DB']],
    ['Firewatch',      ['#CB2D3E','#EF473A','#2C3E50']],
    ['Mint Chip',      ['#00B09B','#96C93D']],
    ['Bubblegum',      ['#FC5C7D','#6A82FB']],
    ['Night Drive',    ['#0F0C29','#302B63','#24243E']],
    ['Sunburn',        ['#FF512F','#F09819']],
    ['Glacier',        ['#83A4D4','#B6FBFF']]
  ];

  var gradCfg = {
    enabled: true,                    // the fleet's "Gradient" checkbox: off = flat --bg
    angle: 135, intensity: 0.45, pegs: 3, harmony: 0,
    preset: -1,                       // >=0 curated, -1 harmony, -2 custom
    custom: ['#56ccff','#990ea5','#fd4f50','#25dfc4'],
    frostDark: 0.85, frostLight: 0.59
  };

  var mix = function (a, b, r) {
    r = clamp(r, 0, 1);
    return [a[0]+(b[0]-a[0])*r, a[1]+(b[1]-a[1])*r, a[2]+(b[2]-a[2])*r].map(Math.round);
  };

  /**
   * ONE PEG MEANS ONE COLOUR.
   *
   * The egui build derives a second stop here so a single peg still sweeps. That
   * is wrong: it silently turns "1 peg" into two, which is exactly what shows up
   * as an unexpected second swatch. A gradient with one peg is a solid colour
   * choice, and `ramp()` returns that colour at every t, so intensity alone
   * decides how strongly it covers the ground.
   *
   * (The Rust fleet still has monoPartner and needs this same correction.)
   */
  function monoPartner(pegs) { return pegs; }

  /** Peg colours: accent -> harmony spread, mode-adapted. */
  function gradientPegs(scale) {
    var dark = scale.dark;
    var accent = scale.steps[8];
    var n = clamp(gradCfg.pegs, 1, 4);

    if (gradCfg.preset === -2) {
      // slot 0 is the LIVE accent, always; slots 1..n are the user's exact picks
      var custom = [accent];
      for (var i = 1; i < n; i++) custom.push(hexToRgb(gradCfg.custom[i]) || accent);
      return monoPartner(custom, dark);
    }

    if (gradCfg.preset >= 0 && GRADIENT_PRESETS[gradCfg.preset]) {
      // curated stops verbatim on dark; lifted toward white on light so the
      // page stays airy under dark text
      return GRADIENT_PRESETS[gradCfg.preset][1].map(function (hx) {
        var c = hexToRgb(hx);
        return dark ? c : mix(c, [255, 255, 255], 0.40);
      });
    }

    var rule = HARMONY_RULES[gradCfg.harmony % HARMONY_RULES.length];
    var derived = generateHarmony(rgbToHsl(accent), rule).slice(0, n).map(function (h) {
      // Deep and rich on dark, pastel on light. Saturation is only capped,
      // never forced up: a grey accent legitimately yields a monochrome ramp.
      // Light pegs run richer than "airy" or white themes read as gradient-off.
      var l = dark ? clamp(h[2], 20, 42) : clamp(h[2], 55, 78);
      var s = dark ? Math.min(h[1], 90) : Math.min(h[1], 75);
      return hslToRgb(h[0], s, l);
    });
    return monoPartner(derived, dark);
  }

  /** Sample the peg ramp at t, with end-hold easing on the outer ~12%. */
  function ramp(pegs, t) {
    t = clamp((t - 0.5) * 1.28 + 0.5, 0, 1);
    if (pegs.length === 1) return pegs[0];
    var scaled = t * (pegs.length - 1);
    var i = Math.min(Math.floor(scaled), pegs.length - 2);
    return mix(pegs[i], pegs[i + 1], scaled - i);
  }

  /** The composited ramp: pegs, end-hold, then intensity mix toward --bg. */
  function rampSample(scale, t) {
    return mix(scale.steps[0], ramp(gradientPegs(scale), t), clamp(gradCfg.intensity, 0, 1));
  }

  var frost = function (dark) { return dark ? gradCfg.frostDark : gradCfg.frostLight; };

  /** The wash as PERCEIVED through frost, i.e. what text actually sits on. */
  function rampSampleFrosted(scale, t) {
    var wash = rampSample(scale, t);
    var f = frost(scale.dark);
    return mix(wash, scale.steps[1], scale.dark ? f : f * (200 / 255));
  }

  /**
   * Write the gradient to CSS. The egui build paints a 16x16 vertex grid because
   * it has no gradient primitive; CSS has one, so the ramp becomes real stops.
   * Angle is converted to the CSS convention (0deg = up, clockwise).
   */
  function applyGradient(scale, el) {
    var target = el || document.documentElement;
    var sc = scale || root.Tront.scale;
    if (!sc) return;

    var st0 = sc.steps;

    // GRADIENT OFF: flat ground, opaque panels, and the page-level inks collapse
    // back to the plain scale. Every downstream consumer keeps working because
    // the same custom properties are still written, just with flat values.
    if (!gradCfg.enabled) {
      var flat = toHex(st0[0]);
      target.style.setProperty('--wash', flat);
      target.style.setProperty('--frost', '1');
      target.style.setProperty('--panel-fill', toHex(st0[1]));
      target.style.setProperty('--panel-blur', '0px');
      target.style.setProperty('--btn-wash', toHex(st0[8]));
      target.style.setProperty('--on-btn-wash', toHex(onColor(st0[8], sc)));
      target.style.setProperty('--btn-wash-worst-lc',
        apcaAbs(onColor(st0[8], sc), st0[8]).toFixed(1));
      target.style.setProperty('--panel-frosted', toHex(st0[1]));
      target.style.setProperty('--ground-wash', flat);
      target.style.setProperty('--ground-panel', toHex(st0[1]));
      target.style.setProperty('--wash-worst-lc',
        Math.max(apcaAbs([0,0,0], st0[0]), apcaAbs([255,255,255], st0[0])).toFixed(1));
      target.style.setProperty('--panel-worst-lc',
        Math.max(apcaAbs([0,0,0], st0[1]), apcaAbs([255,255,255], st0[1])).toFixed(1));
      target.style.setProperty('--text-wash',    toHex(readableAgainst(st0[11], st0[0], LC_TEXT_MIN)));
      target.style.setProperty('--muted-wash',   toHex(readableAgainst(st0[10], st0[0], LC_MUTED)));
      target.style.setProperty('--accent-wash',  toHex(readableAgainst(st0[8],  st0[0], LC_MUTED)));
      target.style.setProperty('--text-frosted', toHex(st0[11]));
      target.style.setProperty('--muted-frosted',toHex(st0[10]));
      target.style.setProperty('--accent-frosted', toHex(readableAgainst(st0[8], st0[1], LC_MUTED)));
      return sc;
    }

    var STOPS = 12;
    var parts = [];
    for (var i = 0; i < STOPS; i++) {
      var t = i / (STOPS - 1);
      parts.push(toHex(rampSample(sc, t)) + ' ' + (t * 100).toFixed(1) + '%');
    }
    var cssAngle = (90 - gradCfg.angle + 360) % 360;
    target.style.setProperty('--wash', 'linear-gradient(' + cssAngle + 'deg,' + parts.join(',') + ')');

    // frost is what text is actually composited against, so panels get an alpha
    // and the guarantee is measured against the FROSTED colour, not the panel
    var f = frost(sc.dark);
    var alpha = sc.dark ? f : f * (200 / 255);
    target.style.setProperty('--frost', alpha.toFixed(3));

    // The concrete fill every surface paints with. Computed here rather than
    // left as a color-mix() in CSS so it works the same in every browser, and
    // so a surface cannot accidentally opt out by using --panel directly.
    var pn = sc.steps[1];
    target.style.setProperty('--panel-fill',
      'rgba(' + pn[0] + ',' + pn[1] + ',' + pn[2] + ',' + alpha.toFixed(3) + ')');
    // blur rises as frost falls: less opacity, more diffusion, so text keeps a
    // usable ground even when the panel is nearly clear
    target.style.setProperty('--panel-blur', ((1 - alpha) * 10).toFixed(1) + 'px');

    // the button gradient: pegs are the accent's own harmony, so it can never
    // clash, and the label is chosen against the ramp's WORST point
    var pegs = gradientPegs(sc);
    var btn = [];
    for (var k = 0; k < 6; k++) {
      var tk = k / 5;
      btn.push(toHex(ramp(pegs, tk)) + ' ' + (tk * 100).toFixed(0) + '%');
    }
    target.style.setProperty('--btn-wash', 'linear-gradient(90deg,' + btn.join(',') + ')');

    // worst point = the stop where white and black are BOTH least readable
    var worst = pegs[0], worstScore = Infinity;
    for (var j = 0; j <= 10; j++) {
      var c = ramp(pegs, j / 10);
      var best = Math.max(apcaAbs([255,255,255], c), apcaAbs([0,0,0], c));
      if (best < worstScore) { worstScore = best; worst = c; }
    }
    target.style.setProperty('--on-btn-wash', toHex(onColor(worst, sc)));
    target.style.setProperty('--btn-wash-worst-lc', worstScore.toFixed(1));

    // panel colour composited over the wash, for contrast checks that tell truth
    target.style.setProperty('--panel-frosted', toHex(rampSampleFrosted(sc, 0.5)));

    // ---- THE SECOND GROUND ----
    // There are two surfaces text can land on: a panel, and the raw wash. The
    // scale guarantees text against the PANEL. Anything sitting directly on the
    // page (section kickers, headings, body copy, ghost buttons) sits on the
    // WASH, and at high intensity that is a saturated ramp rather than --bg.
    // Matching those against --bg is how a roll produces unreadable text.
    //
    // So: find the ramp's worst point and walk the page-level inks against it.
    var st = sc.steps;
    var wWorst = st[0], wScore = Infinity;
    for (var q = 0; q <= 16; q++) {
      var cc = rampSample(sc, q / 16);
      var b = Math.max(apcaAbs([255, 255, 255], cc), apcaAbs([0, 0, 0], cc));
      if (b < wScore) { wScore = b; wWorst = cc; }
    }
    target.style.setProperty('--ground-wash', toHex(wWorst));
    target.style.setProperty('--wash-worst-lc', wScore.toFixed(1));
    target.style.setProperty('--text-wash',   toHex(readableAgainst(st[11], wWorst, LC_TEXT_MIN)));
    target.style.setProperty('--muted-wash',  toHex(readableAgainst(st[10], wWorst, LC_MUTED)));
    target.style.setProperty('--accent-wash', toHex(readableAgainst(st[8],  wWorst, LC_MUTED)));

    // ---- TASK #20, ACTUALLY FIXED ----
    // The panel is not --panel once frost is below 1: it is --panel composited
    // over a wash that varies across the surface. Measuring that was only half
    // the job; the inks have to be walked against it too, at its WORST point.
    var pWorst = st[1], pScore = Infinity;
    for (var r = 0; r <= 16; r++) {
      var pc = rampSampleFrosted(sc, r / 16);
      var pb = Math.max(apcaAbs([255, 255, 255], pc), apcaAbs([0, 0, 0], pc));
      if (pb < pScore) { pScore = pb; pWorst = pc; }
    }
    target.style.setProperty('--ground-panel', toHex(pWorst));
    target.style.setProperty('--panel-worst-lc', pScore.toFixed(1));
    target.style.setProperty('--text-frosted',   toHex(readableAgainst(st[11], pWorst, LC_TEXT_MIN)));
    target.style.setProperty('--muted-frosted',  toHex(readableAgainst(st[10], pWorst, LC_MUTED)));
    target.style.setProperty('--accent-frosted', toHex(readableAgainst(st[8],  pWorst, LC_MUTED)));

    return sc;
  }

  function setGradient(patch) {
    Object.keys(patch || {}).forEach(function (k) { gradCfg[k] = patch[k]; });
    return gradCfg;
  }

  // ================================================================ NAV
  //
  // One registry, three variants. This is the part that fixes drift: the main
  // site renamed /grid/ to /games/ for SEO and punk.tront.xyz still says "Grid",
  // because every navbar is a copy-paste. Rename here, fixed everywhere.

  var HUB = [
    { label: 'Home',    href: 'https://tront.xyz/' },
    { label: 'Games',   href: 'https://tront.xyz/games/' },
    { label: 'Blog',    href: 'https://tront.xyz/blog/' },
    { label: 'Discord', href: 'https://tront.xyz/discord/' }
  ];

  /** The tront network. Satellites inject themselves; crosslinks come from here. */
  var SITES = [
    { id: 'hackerpunk',   label: 'HackerPunk',   href: 'https://punk.tront.xyz/' },
    { id: 'monkeportals', label: 'MonkePortals', href: 'https://monke.tront.xyz/' },
    { id: 'eos-native',   label: 'EOS Native',   href: 'https://tront.xyz/eos-native/' },
    { id: 'spaceview',    label: 'SpaceView',    href: 'https://tront.xyz/spaceview/' },
    { id: 'critters',     label: 'critters',     href: 'https://tront.xyz/critters/' },
    { id: 'snapscan',     label: 'SnapScan',     href: 'https://tront.xyz/snapscan/' },
    { id: 'toaster',      label: 'Toaster',      href: 'https://tront.xyz/toaster/' },
    { id: 'colony',       label: 'Colony',       href: 'https://tront.xyz/colony/' },
    { id: 'lofigen',      label: 'lofigen',      href: 'https://tront.xyz/lofigen/' },
    { id: 'megabunk',     label: 'megabunk',     href: 'https://tront.xyz/megabunk/' }
  ];

  var esc = function (s) {
    return String(s).replace(/[&<>"]/g, function (c) {
      return { '&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;' }[c];
    });
  };

  /**
   * Build a nav.
   *   variant 'hub'       hub pages: the site's own navigation
   *   variant 'satellite' hub links + THIS site injected as the active item
   *                       (HackerPunk, MonkePortals: still yours, still browsing)
   *   variant 'product'   the product's own nav + a parent mark. Someone who
   *                       came to evaluate a library should not be handed a
   *                       menu about a blog.
   *
   * opts: { variant, active, site, label, links[], crosslink }
   */
  function nav(opts) {
    opts = opts || {};
    var variant = opts.variant || 'hub';
    var out = [];

    if (variant === 'product') {
      out.push('<a class="t-nav__mark" href="https://tront.xyz/">' +
               '<span aria-hidden="true">&#9664;</span> tront.xyz</a>');
      out.push('<div class="t-nav__links">');
      (opts.links || []).forEach(function (l) {
        out.push('<a class="t-nav__link" href="' + esc(l.href) + '"' +
          (l.active ? ' aria-current="page"' : '') + '>' + esc(l.label) + '</a>');
      });
    } else {
      out.push('<div class="t-nav__links">');
      var items = HUB.slice();
      if (variant === 'satellite' && opts.site) {
        var me = SITES.filter(function (s) { return s.id === opts.site; })[0];
        // inject after Home, so the satellite reads as part of the network
        if (me) items.splice(1, 0, { label: opts.label || me.label, href: me.href, self: true });
      }
      items.forEach(function (l) {
        var isActive = l.self ? variant === 'satellite'
                              : (opts.active && l.label.toLowerCase() === opts.active.toLowerCase());
        out.push('<a class="t-nav__link" href="' + esc(l.href) + '"' +
          (isActive ? ' aria-current="page"' : '') + '>' + esc(l.label) + '</a>');
      });
    }

    // crosslink: the sideways move a copy-pasted navbar can never offer
    if (opts.crosslink !== false) {
      var others = SITES.filter(function (s) { return s.id !== opts.site; });
      out.push('<details class="t-nav__more"><summary class="t-nav__link">More</summary>' +
        '<div class="t-nav__menu">' + others.map(function (s) {
          return '<a href="' + esc(s.href) + '">' + esc(s.label) + '</a>';
        }).join('') + '</div></details>');
    }

    out.push('</div>');
    return out.join('');
  }

  /** Mount a nav into an element (or every [data-tront-nav] on the page). */
  function mountNav(opts, el) {
    var targets = el ? [el] : [].slice.call(document.querySelectorAll('[data-tront-nav]'));
    targets.forEach(function (t) {
      var o = opts || {};
      if (!el) {
        o = {
          variant: t.dataset.trontNav || 'hub',
          active: t.dataset.active,
          site: t.dataset.site,
          label: t.dataset.label,
          crosslink: t.dataset.crosslink !== 'false'
        };
      }
      t.className = 't-nav t-nav--' + (o.variant || 'hub');
      t.innerHTML = nav(o);
    });
  }

  /** Audit: Lc for every pair that matters. Used by the style guide. */
  function audit(scale) {
    var st = scale.steps;
    var p = function (label, fg, bg) { return { label: label, lc: Math.round(apcaAbs(fg, bg) * 10) / 10 }; };
    return [
      p('text on bg', st[11], st[0]), p('text on panel', st[11], st[1]),
      p('text on elem', st[11], st[2]), p('text on active', st[11], st[4]),
      p('muted on bg', st[10], st[0]), p('muted on panel', st[10], st[1]),
      p('label on solid', onColor(st[8], scale), st[8]),
      p('accent ink on panel', readableAgainst(st[8], st[1], LC_MUTED), st[1])
    ];
  }

  root.Tront = {
    applyTheme: applyTheme, applyItemHue: applyItemHue, audit: audit,
    scaleFromSeed: scaleFromSeed, readableAgainst: readableAgainst, onColor: onColor,
    apcaLc: apcaLc, apcaAbs: apcaAbs, toHex: toHex, hexToRgb: hexToRgb,
    rgbToOklch: rgbToOklch, oklchToRgb: oklchToRgb,
    rgbToHsl: rgbToHsl, hslToRgb: hslToRgb, mix: mix,
    // gradient v2
    nav: nav, mountNav: mountNav, HUB: HUB, SITES: SITES,
    applyGradient: applyGradient, setGradient: setGradient,
    gradientPegs: gradientPegs, rampSample: rampSample, rampSampleFrosted: rampSampleFrosted,
    ramp: ramp, generateHarmony: generateHarmony,
    HARMONY_RULES: HARMONY_RULES, GRADIENT_PRESETS: GRADIENT_PRESETS,
    get gradCfg() { return gradCfg; },
    LC_TEXT: LC_TEXT, LC_TEXT_MIN: LC_TEXT_MIN, LC_MUTED: LC_MUTED, LC_NONTEXT: LC_NONTEXT,
    scale: null
  };
})(window);
