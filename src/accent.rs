//! Kairo's accent: mostly neutral, the color only where the eye should go
//! (the switch when connected, the place in use, the traffic line, focus).
//! Never the platform's blue: the content takes these over the system's
//! accent on both platforms.

use std::sync::OnceLock;

use serde_json::Value;

/// One accent: its fill, the text on the fill, and its color as text, for
/// the light and the dark appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accent {
    pub name: &'static str,
    pub title: &'static str,
    pub light: [&'static str; 3],
    pub dark: [&'static str; 3],
}

pub const ACCENTS: [Accent; 4] = [
    Accent {
        name: "graphite",
        title: "Graphite",
        light: ["#1d1d1f", "#ffffff", "#1d1d1f"],
        dark: ["#f2f2f4", "#1d1d1f", "#f2f2f4"],
    },
    Accent {
        name: "ember",
        title: "Ember",
        light: ["#e0552f", "#ffffff", "#c2431f"],
        dark: ["#ff6a3d", "#ffffff", "#ff8a65"],
    },
    Accent {
        name: "iris",
        title: "Iris",
        light: ["#6b55d6", "#ffffff", "#5a43c4"],
        dark: ["#9d8bff", "#17112e", "#b2a5ff"],
    },
    Accent {
        name: "gold",
        title: "Gold",
        light: ["#b07d12", "#ffffff", "#8f6409"],
        dark: ["#e3b34f", "#241a00", "#ebc46e"],
    },
];

/// The default, when nothing else is asked for.
pub const DEFAULT: &str = "ember";

static CURRENT: OnceLock<Accent> = OnceLock::new();

/// The accent: `KAIRO_ACCENT`, else the setting, else [`DEFAULT`]; fixed for
/// the process' life once read.
pub fn current() -> Accent {
    *CURRENT.get_or_init(|| {
        let wanted = std::env::var("KAIRO_ACCENT")
            .ok()
            .or_else(|| kairo_core::Service::try_global().map(|s| s.settings().accent))
            .unwrap_or_default();
        named(&wanted).unwrap_or_else(|| named(DEFAULT).expect("the default accent"))
    })
}

pub fn named(name: &str) -> Option<Accent> {
    ACCENTS
        .iter()
        .copied()
        .find(|a| a.name.eq_ignore_ascii_case(name.trim()))
}

fn rgb(hex: &str) -> Option<(f32, f32, f32)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|v| v as f32 / 255.0)
    };
    Some((p(0)?, p(2)?, p(4)?))
}

fn hex((r, g, b): (f32, f32, f32)) -> String {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

/// `a` over `b` at `alpha`.
pub fn mix(a: &str, b: &str, alpha: f32) -> String {
    match (rgb(a), rgb(b)) {
        (Some(a), Some(b)) => hex((
            a.0 * alpha + b.0 * (1.0 - alpha),
            a.1 * alpha + b.1 * (1.0 - alpha),
            a.2 * alpha + b.2 * (1.0 - alpha),
        )),
        _ => a.to_owned(),
    }
}

/// A theme (`theme.json`'s shape) with the accent's tokens in both appearances.
pub fn apply(theme: &str) -> String {
    let accent = current();
    let Ok(mut v) = serde_json::from_str::<Value>(theme) else {
        return theme.to_owned();
    };
    if let Some(vars) = v.get_mut("cssVars").and_then(Value::as_object_mut) {
        for (key, [fill, on, text]) in [("light", accent.light), ("dark", accent.dark)] {
            let Some(set) = vars.get_mut(key).and_then(Value::as_object_mut) else {
                continue;
            };
            let base = set
                .get("background")
                .and_then(Value::as_str)
                .unwrap_or(if key == "dark" { "#1e1e20" } else { "#ffffff" })
                .to_owned();
            let card = set
                .get("card")
                .and_then(Value::as_str)
                .unwrap_or(&base)
                .to_owned();
            let mut put = |k: &str, c: String| {
                set.insert(k.to_owned(), Value::String(c));
            };
            put("primary", fill.to_owned());
            put("primary-foreground", on.to_owned());
            put("sidebar-primary", text.to_owned());
            put("ring", mix(text, &base, 0.55));
            put("selection", mix(text, &card, 0.12));
        }
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn mixes_and_applies() {
        assert_eq!(super::mix("#ffffff", "#000000", 0.5), "#808080");
        let t = super::apply(
            r##"{"cssVars":{"theme":{},"light":{"background":"#ffffff","primary":"#007aff"},"dark":{"background":"#000000"}}}"##,
        );
        assert!(!t.contains("#007aff"));
        for a in super::ACCENTS {
            for c in a.light.iter().chain(a.dark.iter()) {
                assert!(super::rgb(c).is_some(), "{c}");
            }
        }
    }
}
