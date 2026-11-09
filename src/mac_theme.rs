//! The NSH content's colors on macOS: AppKit's own, resolved for the light
//! and the dark appearance (the increased-contrast ones when the Mac asks for
//! them) when Kairo starts, over `theme.json`'s. The accent is the user's.

use std::cell::RefCell;

use block2::RcBlock;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAccessibilityHighContrastAqua, NSAppearanceNameAccessibilityHighContrastDarkAqua,
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSColorSpace, NSWorkspace,
};
use serde_json::{Map, Value};

#[derive(Clone, Copy)]
struct Rgba(f64, f64, f64, f64);

impl Rgba {
    /// This color over `base` (opaque).
    fn over(self, base: Rgba) -> Rgba {
        let a = self.3;
        Rgba(self.0 * a + base.0 * (1.0 - a), self.1 * a + base.1 * (1.0 - a), self.2 * a + base.2 * (1.0 - a), 1.0)
    }

    fn alpha(self, a: f64) -> Rgba {
        Rgba(self.0, self.1, self.2, self.3 * a)
    }

    fn hex(self) -> String {
        let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", b(self.0), b(self.1), b(self.2))
    }
}

fn rgba(color: &NSColor) -> Rgba {
    // Catalog colors resolve in the drawing appearance when converted.
    match color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()) {
        Some(c) => Rgba(c.redComponent(), c.greenComponent(), c.blueComponent(), c.alphaComponent()),
        None => Rgba(0.5, 0.5, 0.5, 1.0),
    }
}

/// The theme's tokens of one appearance.
fn palette(name: &objc2_app_kit::NSAppearanceName, dark: bool) -> Map<String, Value> {
    let out = RefCell::new(Map::new());
    let Some(appearance) = NSAppearance::appearanceNamed(name) else {
        return Map::new();
    };
    let block = RcBlock::new(|| {
        let window = rgba(&NSColor::windowBackgroundColor()).over(Rgba(0.0, 0.0, 0.0, 1.0));
        let control = rgba(&NSColor::controlBackgroundColor()).over(window);
        let label = rgba(&NSColor::labelColor());
        let fill = |a: f64| label.alpha(a).over(window);
        // Grouped boxes: white on the light window, a lighter gray on the dark one.
        let card = if dark { fill(0.05) } else { control };
        let accent = rgba(&NSColor::controlAccentColor()).over(window);
        let mut m = out.borrow_mut();
        let mut put = |k: &str, c: Rgba| {
            m.insert(k.to_owned(), Value::String(c.hex()));
        };
        put("background", window);
        put("foreground", label.over(window));
        put("card", card);
        put("card-foreground", label.over(card));
        put("popover", control);
        put("popover-foreground", label.over(control));
        put("primary", accent);
        put("primary-foreground", rgba(&NSColor::alternateSelectedControlTextColor()).over(accent));
        put("secondary", fill(if dark { 0.10 } else { 0.05 }));
        put("secondary-foreground", label.over(window));
        put("muted", fill(0.05));
        put("muted-foreground", rgba(&NSColor::secondaryLabelColor()).over(window));
        put("accent", fill(if dark { 0.08 } else { 0.05 }));
        put("accent-foreground", label.over(window));
        put("destructive", rgba(&NSColor::systemRedColor()).over(window));
        put("border", rgba(&NSColor::separatorColor()).over(window));
        put("input", fill(if dark { 0.18 } else { 0.15 }));
        put("ring", rgba(&NSColor::keyboardFocusIndicatorColor()).over(window));
        put("sidebar", window);
        put("sidebar-foreground", label.over(window));
        put("sidebar-accent", fill(0.08));
        put("sidebar-accent-foreground", label.over(window));
        put("sidebar-border", rgba(&NSColor::separatorColor()).over(window));
        put("sidebar-primary", accent);
        put("success", rgba(&NSColor::systemGreenColor()).over(window));
        put("warning", rgba(&NSColor::systemOrangeColor()).over(window));
        put("track", fill(if dark { 0.16 } else { 0.12 }));
        put("selection", accent.alpha(0.16).over(card));
        put("graph-up", rgba(&NSColor::tertiaryLabelColor()).over(window));
        put("subtle", control);
    });
    appearance.performAsCurrentDrawingAppearance(&block);
    drop(block);
    out.into_inner()
}

/// `theme.json` with AppKit's colors for both appearances.
pub fn theme_json(base: &str) -> String {
    let contrast = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
    // SAFETY: AppKit's constant appearance names.
    let (light, dark) = unsafe {
        if contrast {
            (NSAppearanceNameAccessibilityHighContrastAqua, NSAppearanceNameAccessibilityHighContrastDarkAqua)
        } else {
            (NSAppearanceNameAqua, NSAppearanceNameDarkAqua)
        }
    };
    let mut theme: Value = serde_json::from_str(base).expect("theme.json");
    if let Some(vars) = theme.get_mut("cssVars").and_then(Value::as_object_mut) {
        for (key, name, is_dark) in [("light", light, false), ("dark", dark, true)] {
            if let Some(set) = vars.get_mut(key).and_then(Value::as_object_mut) {
                set.extend(palette(name, is_dark));
            }
        }
        if let Some(t) = vars.get_mut("theme").and_then(Value::as_object_mut) {
            // macOS 26's rounder controls and grouped boxes.
            t.insert("radius".into(), Value::String("12px".into()));
            t.insert("radius-control".into(), Value::String("7px".into()));
        }
    }
    theme.to_string()
}
