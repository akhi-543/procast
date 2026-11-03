//! akscast's design tokens and small shared components (see docs/design/DESIGN.md).
//! Views take every colour, size and font from here.

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::{AnyElement, Div, Hsla, Pixels, Window, div, img, prelude::*, rgba};

use crate::{config, search::Icon};

pub const FONT_UI: &str = "Segoe UI Variable Text";
pub const FONT_DISPLAY: &str = "Segoe UI Variable Display";
pub const FONT_MONO: &str = "Cascadia Mono";
pub const FONT_ICON: &str = "Segoe Fluent Icons";
pub const FONT_EMOJI: &str = "Segoe UI Emoji";

/// Interface size (Settings → General): every length goes through `px`, which scales it.
static SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

pub fn set_size(size: &str) {
    let s: f32 = match size {
        "small" => 0.9,
        "large" => 1.15,
        _ => 1.,
    };
    SCALE.store(s.to_bits(), Ordering::Relaxed);
}

pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// Logical pixels at the chosen interface size. Use this, not `gpui::px`.
pub fn px(v: f32) -> Pixels {
    gpui::px(v * scale())
}

pub const DARK_THEMES: &[&str] = &["akscast Dark", "Midnight", "Graphite", "Forest"];
pub const LIGHT_THEMES: &[&str] = &["akscast Light", "Paper", "Mist"];

pub const BAR: f32 = 56.;
pub const ROW: f32 = 40.;
pub const FOOTER: f32 = 40.;
pub const TILE: f32 = 26.;

/// Segoe Fluent Icons codepoints (each verified to exist in the font).
pub mod g {
    pub const SEARCH: char = '\u{E721}';
    pub const BACK: char = '\u{E72B}';
    pub const CHEVRON_DOWN: char = '\u{E70D}';
    pub const PASTE: char = '\u{E77F}';
    pub const COPY: char = '\u{E8C8}';
    pub const EMOJI: char = '\u{E76E}';
    pub const LOCK: char = '\u{E72E}';
    pub const SLEEP: char = '\u{E708}';
    pub const REFRESH: char = '\u{E72C}';
    pub const POWER: char = '\u{E7E8}';
    pub const SIGN_OUT: char = '\u{F3B1}';
    pub const DELETE: char = '\u{E74D}';
    pub const PERSONALIZE: char = '\u{E771}';
    pub const DOCK_LEFT: char = '\u{E90C}';
    pub const DOCK_RIGHT: char = '\u{E90D}';
    pub const DOCK_BOTTOM: char = '\u{E90E}';
    pub const FULL_SCREEN: char = '\u{E740}';
    pub const RESTORE: char = '\u{E73F}';
    pub const SWITCH_APPS: char = '\u{E8F9}';
    pub const TILES: char = '\u{ECA5}';
    pub const DOCUMENT: char = '\u{E8A5}';
    pub const FOLDER: char = '\u{E8B7}';
    pub const SETTING: char = '\u{E713}';
    pub const SHOP: char = '\u{E719}';
    pub const PUZZLE: char = '\u{EA86}';
    pub const DOWNLOAD: char = '\u{E896}';
    pub const COMMAND: char = '\u{E756}';
    pub const CALCULATOR: char = '\u{E8EF}';
    pub const GLOBE: char = '\u{E774}';
    pub const LINK: char = '\u{E71B}';
    pub const EDIT: char = '\u{E70F}';
    pub const INFO: char = '\u{E946}';
    pub const WARNING: char = '\u{E7BA}';
    pub const CHECK: char = '\u{E73E}';
    pub const CANCEL: char = '\u{E711}';
    pub const RETURN: char = '\u{E751}';
    pub const APP: char = '\u{ECAA}';
    pub const CHECKBOX: char = '\u{E739}';
    pub const CHECKBOX_ON: char = '\u{E73A}';
    pub const FILTER: char = '\u{E71C}';
    pub const STAR: char = '\u{E734}';
    pub const CLOCK: char = '\u{E823}';
    pub const PLAY: char = '\u{E768}';
    pub const CODE: char = '\u{E943}';
    pub const NETWORK: char = '\u{E968}';
    pub const CALENDAR: char = '\u{E787}';
    pub const KEYBOARD: char = '\u{E765}';
    pub const BULLETS: char = '\u{E8FD}';
    pub const UP: char = '\u{E74A}';
    pub const PIN: char = '\u{E718}';
    pub const SHIELD: char = '\u{EA18}';
    pub const MAIL: char = '\u{E715}';
}

#[derive(Clone, Copy)]
pub struct Tokens {
    pub bg: Hsla,
    pub bg_elevated: Hsla,
    pub surface: Hsla,
    pub surface_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub hairline: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
}

fn c(hex: u32) -> Hsla {
    gpui::rgb_to_hsla(rgba(hex))
}

impl Tokens {
    fn dark() -> Self {
        Self {
            bg: c(0x1b1b1fdb),
            bg_elevated: c(0x26262cff),
            surface: c(0xffffff10),
            surface_strong: c(0xffffff1a),
            text: c(0xf2f2f5ff),
            text_muted: c(0xa0a0abff),
            text_faint: c(0x6e6e7aff),
            hairline: c(0xffffff14),
            border: c(0xffffff1f),
            accent: c(0x7c6cffff),
            accent_text: c(0xffffffff),
            success: c(0x4cc38aff),
            warning: c(0xf5b74fff),
            danger: c(0xff6b6bff),
        }
    }

    fn light() -> Self {
        Self {
            bg: c(0xf7f7f9e0),
            bg_elevated: c(0xffffffff),
            surface: c(0x0000000d),
            surface_strong: c(0x00000014),
            text: c(0x1b1b1fff),
            text_muted: c(0x5f5f6bff),
            text_faint: c(0x8e8e9aff),
            hairline: c(0x00000014),
            border: c(0x0000001f),
            accent: c(0x5b4bf0ff),
            accent_text: c(0xffffffff),
            success: c(0x1f9d61ff),
            warning: c(0xc98100ff),
            danger: c(0xd93d3dff),
        }
    }

    /// A named palette; unknown names fall back to akscast Dark / Light.
    pub fn named(name: &str, dark: bool) -> Self {
        let base = if dark { Self::dark() } else { Self::light() };
        // (background, panels, text, muted, faint, accent)
        let (bg, elevated, text, muted, faint, accent) = match (dark, name) {
            (true, "Midnight") => (0x0f1424db, 0x18203aff, 0xe8eefcff, 0x9aa6c4ff, 0x67728fff, 0x4fb3ffff),
            (true, "Graphite") => (0x1f1f1fdb, 0x2a2a2aff, 0xf0f0f0ff, 0xa8a8a8ff, 0x707070ff, 0xff8a3dff),
            (true, "Forest") => (0x141c18db, 0x1d2822ff, 0xe9f2ecff, 0x9db3a6ff, 0x6a8174ff, 0x4cc38aff),
            (false, "Paper") => (0xfbf8f1e6, 0xffffffff, 0x2b2620ff, 0x6e655aff, 0x9a9083ff, 0xc2410cff),
            (false, "Mist") => (0xf1f5f9e6, 0xffffffff, 0x14212bff, 0x55667aff, 0x8796a8ff, 0x0f9b8eff),
            _ => return base,
        };
        Self { bg: c(bg), bg_elevated: c(elevated), text: c(text), text_muted: c(muted), text_faint: c(faint), accent: c(accent), ..base }
    }

    /// From `[theme]`: the named palette for the mode (system follows Windows), then overrides.
    pub fn from_theme(t: &config::Theme, window: &Window) -> Self {
        let dark = match t.mode.as_str() {
            "dark" => true,
            "light" => false,
            _ => !matches!(window.appearance(), gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight),
        };
        let mut k = Self::named(if dark { &t.dark } else { &t.light }, dark);
        let set = |slot: &mut Hsla, v: &Option<String>| {
            if let Some(hex) = v.as_deref().and_then(config::parse_hex) {
                *slot = c(hex);
            }
        };
        set(&mut k.bg, &t.background);
        set(&mut k.text, &t.text);
        set(&mut k.text_muted, &t.muted);
        set(&mut k.accent, &t.selected);
        set(&mut k.accent_text, &t.selected_text);
        set(&mut k.border, &t.border);
        set(&mut k.hairline, &t.separator);
        set(&mut k.bg_elevated, &t.container);
        k
    }

}

/// A Segoe Fluent Icons glyph.
pub fn fluent(glyph: char, size: f32, color: Hsla) -> Div {
    div().font_family(FONT_ICON).text_size(px(size)).line_height(px(size)).text_color(color).child(glyph.to_string())
}

/// A list icon: Fluent glyphs sit in a rounded tile; images and emoji draw bare.
pub fn icon_view(icon: &Icon, t: &Tokens, selected: bool) -> AnyElement {
    let frame = || div().size(px(TILE)).flex_none().flex().items_center().justify_center();
    match icon {
        Icon::Image(image) => frame().child(img(image.clone()).size(px(20.))).into_any_element(),
        Icon::Fluent(ch) => frame()
            .rounded(px(6.))
            .bg(if selected { t.surface_strong } else { t.surface })
            .child(fluent(*ch, 15., if selected { t.text } else { t.text_muted }))
            .into_any_element(),
        Icon::Glyph(s) => frame().font_family(FONT_EMOJI).text_size(px(17.)).child(s.clone()).into_any_element(),
    }
}

/// A keyboard key cap; "↵" draws the Fluent ReturnKey glyph.
pub fn key_cap(label: &str, t: &Tokens) -> Div {
    let cap = div()
        .h(px(20.))
        .min_w(px(20.))
        .px(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .text_size(px(11.))
        .text_color(t.text_muted);
    if label == "↵" { cap.child(fluent(g::RETURN, 11., t.text_muted)) } else { cap.child(label.to_string()) }
}

/// "Actions  Ctrl K": a label followed by caps.
pub fn shortcut(label: &str, keys: &[&str], t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(div().pr(px(3.)).text_color(t.text).child(label.to_string()))
        .children(keys.iter().map(|k| key_cap(k, t)))
}

pub fn vdivider(t: &Tokens) -> Div {
    div().w(px(1.)).h(px(16.)).bg(t.hairline)
}

/// Section header: sits on the bottom of a row-height slot (rows are virtualised).
pub fn section_header(title: &str, count: Option<usize>, t: &Tokens) -> Div {
    div()
        .h(px(ROW))
        .w_full()
        .px(px(12.))
        .pb(px(4.))
        .flex()
        .items_end()
        .gap(px(8.))
        .text_size(px(12.5))
        .text_color(t.text_faint)
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_string()))
        .children(count.map(|n| div().child(n.to_string())))
}

/// Centred icon, title and hint for empty views.
pub fn empty_state(glyph: char, title: &str, hint: &str, t: &Tokens) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .child(fluent(glyph, 44., t.text_faint))
        .child(div().pt(px(6.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.text).child(title.to_string()))
        .children((!hint.is_empty()).then(|| div().text_size(px(13.)).text_color(t.text_muted).child(hint.to_string())))
}

/// On/off switch for settings.
pub fn toggle(on: bool, t: &Tokens) -> Div {
    div()
        .w(px(36.))
        .h(px(20.))
        .rounded_full()
        .p(px(2.))
        .flex()
        .when(on, |d| d.justify_end())
        .bg(if on { t.accent } else { t.surface_strong })
        .child(div().size(px(16.)).rounded_full().bg(if on { t.accent_text } else { t.text_muted }))
}
