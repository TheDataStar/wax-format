//! Colour themes: light, dark, and colour-blind-safe state colours.
//!
//! * **Light** is `design-language.md` §4's locked palette, transcribed
//!   exactly.
//! * **Dark** is new. Commander decided on 2026-11-15 that Delt has light
//!   *and* dark modes, which overrides design-language §4's "one light
//!   theme". The design language defines no dark values, so the ones here
//!   are a **proposal for its owner to adopt or replace**, held to the same
//!   bar as the light palette: every pairing is measured by the tests below
//!   against all three surfaces (`bg`, `surface`, `raised`), not asserted.
//! * **Colour-blind mode** swaps the one pairing red/green colour blindness
//!   cannot tell apart, `success` against `error`, by moving `success` to
//!   blue. State never rests on colour alone anyway (design-language §4:
//!   "every state carries its dot *and* its word"); this mode is extra help.
//!
//! The window reads [`css`] once at start, so this table is the only place
//! a colour is written.

/// `(token, hex)` in the order the CSS lists them.
pub type Palette = &'static [(&'static str, &'static str)];

/// design-language §4, exactly, plus `on-accent` (the white §4 measures on
/// `accent-ink`).
pub const LIGHT: Palette = &[
    ("bg", "#E9E6DF"),
    ("surface", "#F2EEE6"),
    ("raised", "#FAF8F4"),
    ("line", "#DDD6C8"),
    ("line-strong", "#C7BFAE"),
    ("ink", "#2B2926"),
    ("ink-2", "#6B655C"),
    ("ink-3", "#8E887E"),
    ("accent", "#C1622A"),
    ("accent-ink", "#A4460C"),
    ("on-accent", "#FFFFFF"),
    ("success", "#3A7143"),
    ("warn", "#8A5A00"),
    ("error", "#9E2F2F"),
    ("charcoal", "#3A3834"),
    ("rule-a", "#A7A298"),
];

/// Proposed dark palette (see the module note): warm near-black grounds in
/// the light palette's hue family, same token roles and the same rules
/// (`accent` is never text; `ink-3` is decorative only).
pub const DARK: Palette = &[
    ("bg", "#1C1B19"),
    ("surface", "#252320"),
    ("raised", "#2E2B27"),
    ("line", "#3A3732"),
    ("line-strong", "#6A645B"),
    ("ink", "#EDE8DF"),
    ("ink-2", "#B5AEA2"),
    ("ink-3", "#7A746B"),
    ("accent", "#D9773C"),
    ("accent-ink", "#F2A673"),
    ("on-accent", "#1C1B19"),
    ("success", "#7CC08A"),
    ("warn", "#E0B04E"),
    ("error", "#F08C84"),
    // the rule's dark segment has to read on a dark ground, so it inverts
    ("charcoal", "#D8D2C7"),
    ("rule-a", "#6B665E"),
];

/// Colour-blind mode: `success` moves to blue in each theme.
pub const COLORBLIND_LIGHT: Palette = &[("success", "#1F5F99")];
pub const COLORBLIND_DARK: Palette = &[("success", "#7FB8F0")];

pub fn get(p: Palette, token: &str) -> Option<&'static str> {
    p.iter().find(|(k, _)| *k == token).map(|(_, v)| *v)
}

/// WCAG 2.x relative luminance of `#RRGGBB`.
pub fn luminance(hex: &str) -> f64 {
    let h = hex.trim_start_matches('#');
    let ch = |i: usize| {
        let c = u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(0) + 0.7152 * ch(2) + 0.0722 * ch(4)
}

/// WCAG 2.x contrast ratio.
pub fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn block(selector: &str, p: Palette) -> String {
    let mut s = format!("{selector} {{\n");
    for (k, v) in p {
        s.push_str(&format!("  --{k}: {v};\n"));
    }
    s.push_str("}\n");
    s
}

/// The custom properties for every theme. The window sets
/// `data-theme="light|dark"` on `<html>` (or leaves it unset to follow the
/// computer), and `data-colorblind="on"` for colour-blind mode.
pub fn css() -> String {
    let mut s = String::new();
    s.push_str(&block(":root, :root[data-theme=\"light\"]", LIGHT));
    s.push_str(&block(":root[data-theme=\"dark\"]", DARK));
    s.push_str("@media (prefers-color-scheme: dark) {\n");
    s.push_str(&block(":root:not([data-theme=\"light\"])", DARK));
    s.push_str(&block(":root:not([data-theme=\"light\"])[data-colorblind=\"on\"]", COLORBLIND_DARK));
    s.push_str("}\n");
    s.push_str(&block(":root[data-colorblind=\"on\"]", COLORBLIND_LIGHT));
    s.push_str(&block(":root[data-theme=\"dark\"][data-colorblind=\"on\"]", COLORBLIND_DARK));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUNDS: [&str; 3] = ["bg", "surface", "raised"];

    /// The requirement each foreground token must meet on every ground.
    const RULES: &[(&str, f64)] = &[
        ("ink", 7.0),
        ("ink-2", 4.5),
        ("accent-ink", 4.5), // text, links, focus ring
        ("accent", 3.0),     // fills and UI only, never text
        ("success", 4.5),
        ("warn", 4.5),
        ("error", 4.5),
    ];

    fn check(name: &str, p: Palette, over: Palette) {
        let tok = |t: &str| get(over, t).or_else(|| get(p, t)).unwrap();
        for g in GROUNDS {
            for (t, need) in RULES {
                let c = contrast(tok(t), tok(g));
                assert!(c >= *need, "{name}: {t} on {g} is {c:.2}, needs {need}");
            }
        }
        let c = contrast(tok("on-accent"), tok("accent-ink"));
        assert!(c >= 4.5, "{name}: on-accent on accent-ink is {c:.2}");
        // the focus ring (accent-ink) against the ground: 3:1 non-text
        assert!(contrast(tok("accent-ink"), tok("bg")) >= 3.0);
    }

    #[test]
    fn light_matches_the_design_language_figures() {
        // design-language §4's own numbers, measured on surface and bg
        let r = |a: &str, b: &str| (contrast(get(LIGHT, a).unwrap(), get(LIGHT, b).unwrap()) * 100.0).round() / 100.0;
        assert_eq!(r("ink", "bg"), 11.64);
        assert_eq!(r("ink-2", "bg"), 4.63);
        assert_eq!(r("accent-ink", "bg"), 4.87);
        assert_eq!(r("accent", "bg"), 3.34);
        assert_eq!(r("error", "bg"), 5.81);
        assert!((r("ink-3", "bg") - 2.82).abs() < 0.011);
    }

    #[test]
    fn every_theme_passes_every_pairing() {
        // ink on light raised is the light palette's tightest ink pairing:
        // design-language §4 quotes 12.5 on surface; 7:1 holds everywhere
        check("light", LIGHT, &[]);
        check("dark", DARK, &[]);
        check("light+colorblind", LIGHT, COLORBLIND_LIGHT);
        check("dark+colorblind", DARK, COLORBLIND_DARK);
    }

    #[test]
    fn ink_3_stays_decorative_in_dark_too() {
        // below body-text contrast, like the light ink-3, so it cannot be
        // mistaken for a text colour
        assert!(contrast(get(DARK, "ink-3").unwrap(), get(DARK, "bg").unwrap()) < 4.5);
    }

    #[test]
    fn both_palettes_define_the_same_tokens() {
        let a: Vec<_> = LIGHT.iter().map(|(k, _)| *k).collect();
        let b: Vec<_> = DARK.iter().map(|(k, _)| *k).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn css_carries_every_token() {
        let s = css();
        for (k, v) in DARK.iter().chain(LIGHT) {
            assert!(s.contains(&format!("--{k}: {v};")), "{k}");
        }
        assert!(s.contains("prefers-color-scheme: dark"));
    }
}
