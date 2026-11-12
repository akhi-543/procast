//! procast's design tokens and small shared components (see docs/design/DESIGN.md).
//! Views take every colour, size and font from here.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui::{AnyElement, App, AssetSource, Div, Hsla, Pixels, SharedString, Svg, Window, div, img, prelude::*, rgba, svg};

use crate::{config, search::Icon};

/// Space Grotesk, the variable font in `assets/fonts` (registered by `register_fonts`).
pub const FONT_UI: &str = "Space Grotesk";
pub const FONT_DISPLAY: &str = FONT_UI;
pub const FONT_SMALL: &str = FONT_UI;
pub const FONT_MONO: &str = "Cascadia Mono";
pub const FONT_EMOJI: &str = "Segoe UI Emoji";

// Every length goes through `px`, which scales it. The launcher scales by interface size
// (Settings → General) times the screen factor (window width as a share of the screen);
// the settings window by interface size alone. Each sets the scale at the top of render.
const ONE: u32 = 0x3f80_0000; // 1.0f32
static SIZE: AtomicU32 = AtomicU32::new(ONE);
static SCREEN: AtomicU32 = AtomicU32::new(ONE);
static SCALE: AtomicU32 = AtomicU32::new(ONE);

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

pub fn set_size(size: &str) {
    let s: f32 = match size {
        "small" => 0.9,
        "large" => 1.15,
        _ => 1.,
    };
    SIZE.store(s.to_bits(), Ordering::Relaxed);
}

/// How much bigger than its base size the launcher is drawn on the screen it opens on.
pub fn set_screen(factor: f32) {
    SCREEN.store(factor.clamp(0.5, 3.).to_bits(), Ordering::Relaxed);
}

pub fn launcher_scale() -> f32 {
    load(&SIZE) * load(&SCREEN)
}

pub fn scale_launcher() {
    SCALE.store(launcher_scale().to_bits(), Ordering::Relaxed);
}

pub fn scale_settings() {
    SCALE.store(SIZE.load(Ordering::Relaxed), Ordering::Relaxed);
}

pub fn scale() -> f32 {
    load(&SCALE)
}

/// Logical pixels at the chosen interface size. Use this, not `gpui::px`.
pub fn px(v: f32) -> Pixels {
    gpui::px(v * scale())
}

pub const DARK_THEMES: &[&str] = &["procast Dark", "Midnight", "Graphite", "Forest"];
pub const LIGHT_THEMES: &[&str] = &["procast Light", "Paper", "Mist"];

pub const BAR: f32 = 56.;
pub const ROW: f32 = 40.;
pub const FOOTER: f32 = 40.;
/// The icon box in a row. App images and glyph tiles both fill it, so titles line up.
pub const TILE: f32 = 28.;
/// Inputs, buttons, segmented controls, dropdowns and action-panel rows.
pub const CONTROL: f32 = 32.;
/// Key caps.
pub const CAP: f32 = 20.;
/// Space above and below the list, so the window holds whole rows.
pub const LIST_PAD: f32 = 4.;

/// The type scale. Views use only these sizes for text.
pub mod ts {
    /// Key caps, tiny labels.
    pub const CAPTION: f32 = 11.;
    /// Section headers, descriptions, the settings footer.
    pub const SMALL: f32 = 12.;
    /// Subtitles, accessories, the action bar.
    pub const BODY_SM: f32 = 13.;
    /// Rows and body text.
    pub const BODY: f32 = 14.;
    /// Card titles, second-level headings, the calculator's question.
    pub const HEADING: f32 = 16.;
    /// Page headings and the calculator answer.
    pub const DISPLAY: f32 = 20.;
    /// The search field: what you typed is the largest text in the window.
    pub const QUERY: f32 = 22.;
}

/// Corner radii. Small things get small corners; things that hold other things get large ones.
pub mod r {
    /// Key caps, segments inside a control.
    pub const XS: f32 = 4.;
    /// Icon tiles, inputs, buttons, chips.
    pub const SM: f32 = 6.;
    /// Rows.
    pub const MD: f32 = 8.;
    /// Cards, popovers, dialogs.
    pub const LG: f32 = 12.;
}

/// Glyph sizes.
pub mod ic {
    /// Inside key caps, chips and dropdowns.
    pub const SM: f32 = 12.;
    /// The action bar, action-panel rows, notices.
    pub const MD: f32 = 14.;
    /// Icon tiles, the search mark, checkboxes.
    pub const LG: f32 = 16.;
    /// Empty states and dialogs.
    pub const XL: f32 = 40.;
}

/// What kind of thing a row is. The icon tile takes the kind's hue, so a list reads at a
/// glance: window moves are teal, system actions amber, clipboard blue, and so on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tint {
    /// procast's own commands.
    Accent,
    /// Power, session, recycle bin, dark mode.
    System,
    /// Window layout.
    Window,
    /// Clipboard, emoji, snippets: things you paste.
    Paste,
    /// Quicklinks and web search.
    Web,
    /// Files and folders.
    File,
    /// The calculator.
    Calc,
    /// Ending or removing something.
    Danger,
}

impl Tint {
    pub fn hue(self, t: &Tokens) -> Hsla {
        // (dark theme, light theme): lighter on dark backgrounds, deeper on light ones.
        let (d, l) = match self {
            Tint::Accent => return t.accent,
            Tint::Danger => return t.danger,
            Tint::System => (0xf2b65aff, 0xb27100ff),
            Tint::Window => (0x4fc4cfff, 0x0f8a96ff),
            Tint::Paste => (0x6aa8ffff, 0x2f6fd6ff),
            Tint::Web => (0x6fd19cff, 0x1a8a55ff),
            Tint::File => (0xd9b77eff, 0x9a6b1fff),
            Tint::Calc => (0xf08cc0ff, 0xc23d84ff),
        };
        c(if t.dark { d } else { l })
    }
}

/// Icon names: the stems of the files in `assets/icons/`. `ALL` lists them for the tests.
macro_rules! names {
    ($($k:ident = $v:literal),* $(,)?) => {
        pub mod g {
            $(pub const $k: &str = $v;)*
            pub const ALL: &[&str] = &[$($v),*];
        }
    };
}

names! {
    SEARCH = "search",
    BACK = "back",
    CHEVRON_DOWN = "chevron-down",
    PASTE = "paste",
    COPY = "copy",
    EMOJI = "emoji",
    LOCK = "lock",
    SLEEP = "sleep",
    REFRESH = "restart",
    POWER = "power",
    SIGN_OUT = "sign-out",
    DELETE = "delete",
    PERSONALIZE = "personalize",
    DOCK_LEFT = "dock-left",
    DOCK_RIGHT = "dock-right",
    DOCK_BOTTOM = "dock-bottom",
    FULL_SCREEN = "full-screen",
    RESTORE = "restore",
    SWITCH_APPS = "switch-apps",
    TILES = "tiles",
    DOCUMENT = "document",
    FOLDER = "folder",
    SETTING = "settings",
    SHOP = "store",
    PUZZLE = "extension",
    DOWNLOAD = "download",
    COMMAND = "command",
    CALCULATOR = "calculator",
    GLOBE = "web",
    LIGHTBULB = "idea",
    LINK = "link",
    EDIT = "edit",
    INFO = "info",
    WARNING = "warning",
    CHECK = "check",
    CANCEL = "cancel",
    RETURN = "return",
    APP = "app",
    CHECKBOX = "checkbox",
    CHECKBOX_ON = "checkbox-on",
    FILTER = "filter",
    STAR = "star",
    CLOCK = "clock",
    PLAY = "play",
    CODE = "code",
    NETWORK = "network",
    CALENDAR = "calendar",
    KEYBOARD = "keyboard",
    BULLETS = "list",
    UP = "up",
    DOWN = "down",
    PICTURE = "picture",
    VIDEO = "video",
    AUDIO = "audio",
    ZIP = "archive",
    PDF = "file",
    FORWARD = "forward",
    PIN = "pin",
    SHIELD = "shield",
    MAIL = "mail",
}

#[derive(Clone, Copy)]
pub struct Tokens {
    pub dark: bool,
    pub bg: Hsla,
    pub bg_elevated: Hsla,
    pub surface: Hsla,
    pub surface_strong: Hsla,
    /// Under the mouse: lighter than selection, so the two never look alike.
    pub hover: Hsla,
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
    /// Behind a dialog.
    pub scrim: Hsla,
}

fn c(hex: u32) -> Hsla {
    gpui::rgb_to_hsla(rgba(hex))
}

impl Tokens {
    fn dark() -> Self {
        Self {
            dark: true,
            bg: c(0x17171cff),
            bg_elevated: c(0x222229ff),
            surface: c(0xffffff0f),
            surface_strong: c(0xffffff1c),
            hover: c(0xffffff0a),
            text: c(0xf2f2f5ff),
            text_muted: c(0xa0a0abff),
            text_faint: c(0x8b8b97ff),
            hairline: c(0xffffff14),
            border: c(0xffffff1f),
            accent: c(0x7c6cffff),
            accent_text: c(INK),
            success: c(0x4cc38aff),
            warning: c(0xf5b74fff),
            danger: c(0xff6b6bff),
            scrim: c(0x00000066),
        }
    }

    fn light() -> Self {
        Self {
            dark: false,
            bg: c(0xf7f7f9ff),
            bg_elevated: c(0xffffffff),
            surface: c(0x0000000d),
            surface_strong: c(0x00000017),
            hover: c(0x00000008),
            text: c(0x1b1b1fff),
            text_muted: c(0x53535dff),
            text_faint: c(0x60606bff),
            hairline: c(0x00000014),
            border: c(0x0000001f),
            accent: c(0x5b4bf0ff),
            accent_text: c(0xffffffff),
            success: c(0x1f9d61ff),
            warning: c(0x946000ff),
            danger: c(0xd43838ff),
            scrim: c(0x00000040),
        }
    }

    /// A named palette; unknown names fall back to procast Dark / Light.
    pub fn named(name: &str, dark: bool) -> Self {
        let base = if dark { Self::dark() } else { Self::light() };
        // (background, panels, text, muted, faint, accent)
        let (bg, elevated, text, muted, faint, accent) = match (dark, name) {
            (true, "Midnight") => (0x0f1424ff, 0x18203aff, 0xe8eefcff, 0x9aa6c4ff, 0x8591b0ff, 0x4fb3ffff),
            (true, "Graphite") => (0x1c1c1cff, 0x2a2a2aff, 0xf0f0f0ff, 0xb2b2b2ff, 0x989898ff, 0xff8a3dff),
            (true, "Forest") => (0x141c18ff, 0x1d2822ff, 0xe9f2ecff, 0x9db3a6ff, 0x86a092ff, 0x4cc38aff),
            (false, "Paper") => (0xfbf8f1ff, 0xffffffff, 0x2b2620ff, 0x5c5449ff, 0x675e53ff, 0xc2410cff),
            (false, "Mist") => (0xf1f5f9ff, 0xffffffff, 0x14212bff, 0x465a6eff, 0x546678ff, 0x0b7f74ff),
            _ => return base,
        };
        Self { bg: c(bg), bg_elevated: c(elevated), text: c(text), text_muted: c(muted), text_faint: c(faint), accent: c(accent), accent_text: on_color(c(accent)), ..base }
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
        // Text on the accent is white or ink, whichever reads better, unless you set it.
        k.accent_text = on_color(k.accent);
        set(&mut k.accent_text, &t.selected_text);
        set(&mut k.border, &t.border);
        set(&mut k.hairline, &t.separator);
        set(&mut k.bg_elevated, &t.container);
        k
    }

    /// The colour a row lights up in when selected: its kind's hue, an app icon's own
    /// dominant colour, or the accent.
    pub fn hue_for(&self, tint: Option<Tint>, icon: &Icon) -> Hsla {
        match (tint, icon) {
            (Some(k), _) => k.hue(self),
            (None, Icon::Image(image)) => image_hue(image).map_or(self.accent, |h| gpui::hsla(h, 0.75, if self.dark { 0.68 } else { 0.42 }, 1.)),
            _ => self.accent,
        }
    }

    /// The selected fill everywhere: a wash of the hue. Hover is always lighter than this.
    pub fn selected(&self, hue: Hsla) -> Hsla {
        Hsla { alpha: if self.dark { 0.18 } else { 0.13 }, ..hue }
    }
}

const INK: u32 = 0x121216ff;

fn luminance(color: Hsla) -> f32 {
    let rgb = gpui::hsla_to_rgba(color);
    let f = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * f(rgb.red) + 0.7152 * f(rgb.green) + 0.0722 * f(rgb.blue)
}

/// White or ink, whichever has more contrast on `bg`. The better one is at least 4.3:1 on
/// any colour; the tests hold the shipped accents and `danger` to 4.5:1.
pub fn on_color(bg: Hsla) -> Hsla {
    let l = luminance(bg) + 0.05;
    if 1.05 / l >= l / (luminance(c(INK)) + 0.05) { c(0xffffffff) } else { c(INK) }
}

/// The most common saturated hue in an image (0..1), or None when it's mostly grey.
// ponytail: recomputed per frame for the one selected row (about a thousand samples);
// cache it beside the icon if a profile ever shows it.
pub fn image_hue(image: &gpui::RenderImage) -> Option<f32> {
    const N: usize = 24;
    let bytes = image.as_bytes(0)?;
    let step = (bytes.len() / 4 / 1024).max(1);
    let mut buckets = [0f32; N];
    for p in bytes.chunks_exact(4).step_by(step) {
        let [b, g, r, a] = [p[0], p[1], p[2], p[3]].map(|v| v as f32 / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let d = max - min;
        if d < 0.2 || a < 0.5 {
            continue;
        }
        let h = if max == r { ((g - b) / d).rem_euclid(6.) } else if max == g { (b - r) / d + 2. } else { (r - g) / d + 4. } / 6.;
        buckets[((h * N as f32) as usize).min(N - 1)] += d * a;
    }
    let (best, weight) = buckets.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    // Under about 3% of the samples being colourful, the icon reads as grey.
    (*weight > 0.03 * (bytes.len() / 4 / step) as f32 * 0.5).then(|| (best as f32 + 0.5) / N as f32)
}

/// Registers Space Grotesk with the text system. Call before any window opens.
pub fn register_fonts(cx: &App) {
    static FONT: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk[wght].ttf");
    if let Err(e) = cx.text_system().add_fonts(vec![Cow::Borrowed(FONT)]) {
        eprintln!("procast: could not load Space Grotesk: {e}");
    }
}

macro_rules! icon_files {
    ($($name:literal),* $(,)?) => {
        /// (stem, SVG source) for every file in `assets/icons`.
        const ICONS: &[(&str, &str)] = &[$(($name, include_str!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}

icon_files![
    "app",
    "archive",
    "audio",
    "back",
    "calculator",
    "calendar",
    "cancel",
    "check",
    "checkbox",
    "checkbox-on",
    "chevron-down",
    "clock",
    "code",
    "command",
    "copy",
    "delete",
    "dock-bottom",
    "dock-left",
    "dock-right",
    "document",
    "down",
    "download",
    "edit",
    "emoji",
    "extension",
    "file",
    "filter",
    "folder",
    "forward",
    "full-screen",
    "idea",
    "info",
    "keyboard",
    "link",
    "list",
    "lock",
    "mail",
    "network",
    "paste",
    "personalize",
    "picture",
    "pin",
    "play",
    "power",
    "restart",
    "restore",
    "return",
    "search",
    "settings",
    "shield",
    "sign-out",
    "sleep",
    "star",
    "store",
    "switch-apps",
    "tiles",
    "up",
    "video",
    "warning",
    "web",
];

/// Serves the embedded icons: `icons/<stem>.svg` as stored (stroke 1.75), `icons/<stem>@xl.svg`
/// (stroke 1.25, for 40 px) and `icons/<stem>@cap.svg` (stroke 2, for key caps).
pub struct Icons;

impl Icons {
    fn names() -> impl Iterator<Item = String> {
        ICONS.iter().flat_map(|(n, _)| [format!("icons/{n}.svg"), format!("icons/{n}@xl.svg"), format!("icons/{n}@cap.svg")])
    }
}

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let Some(rest) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")) else { return Ok(None) };
        let (stem, stroke) = match rest.split_once('@') {
            None => (rest, None),
            Some((stem, "xl")) => (stem, Some("1.25")),
            Some((stem, "cap")) => (stem, Some("2")),
            Some(_) => return Ok(None),
        };
        let Some((_, src)) = ICONS.iter().find(|(n, _)| *n == stem) else { return Ok(None) };
        Ok(Some(match stroke {
            None => Cow::Borrowed(src.as_bytes()),
            Some(w) => Cow::Owned(src.replace("stroke-width=\"1.75\"", &format!("stroke-width=\"{w}\"")).into_bytes()),
        }))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Self::names().filter(|n| n.starts_with(path)).map(Into::into).collect())
    }
}

/// A vector icon (a `g::` name) of `size` px in `color`; the 40 px size uses the thinner stroke.
pub fn icon(name: &'static str, size: f32, color: Hsla) -> Svg {
    let path = if size >= ic::XL { format!("icons/{name}@xl.svg") } else { format!("icons/{name}.svg") };
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

/// An icon inside a key cap: `ic::SM` with the heavier stroke.
pub fn icon_cap(name: &'static str, color: Hsla) -> Svg {
    svg().path(format!("icons/{name}@cap.svg")).size(px(ic::SM)).flex_none().text_color(color)
}

/// A list icon with no kind hue.
pub fn icon_view(icon: &Icon, t: &Tokens, selected: bool) -> AnyElement {
    icon_tile(icon, t, selected, None)
}

/// A glyph tile's (wash, glyph) colours. Selected, the wash deepens and the glyph steps up
/// to the text colour: a hue on a deeper wash of itself falls under 3:1.
fn tile_colors(t: &Tokens, tint: Option<Tint>, selected: bool) -> (Hsla, Hsla) {
    match tint.map(|k| k.hue(t)) {
        Some(h) => (Hsla { alpha: if selected { 0.26 } else { 0.14 }, ..h }, if selected { t.text } else { h }),
        None => (if selected { t.surface_strong } else { t.surface }, if selected { t.text } else { t.text_muted }),
    }
}

/// A list icon in the shared `TILE` box. Glyphs sit on a wash of the row's kind hue; images
/// fill the box; emoji draw bare.
pub fn icon_tile(kind: &Icon, t: &Tokens, selected: bool, tint: Option<Tint>) -> AnyElement {
    let frame = || div().size(px(TILE)).flex_none().flex().items_center().justify_center();
    match kind {
        Icon::Image(image) => frame().child(img(image.clone()).size(px(TILE - 4.))).into_any_element(),
        Icon::Vector(name) => {
            let (wash, glyph) = tile_colors(t, tint, selected);
            frame().rounded(px(r::SM)).bg(wash).child(icon(name, ic::LG, glyph)).into_any_element()
        }
        Icon::Glyph(s) => frame().font_family(FONT_EMOJI).text_size(px(18.)).child(s.clone()).into_any_element(),
    }
}

/// The selection pill, centred on the left edge of a `height`-tall row.
pub fn pill(hue: Hsla, height: f32) -> Div {
    div().absolute().left(px(0.)).top(px((height - 16.) / 2.)).w(px(3.)).h(px(16.)).rounded_full().bg(hue)
}

/// A selectable row's inner box: the shared radius, the selected wash and pill, the hover.
/// Used by launcher rows, action-panel rows, the settings sidebar and the theme preview.
pub fn selectable(height: f32, on: bool, hue: Hsla, t: &Tokens) -> Div {
    let (fill, hover) = (t.selected(hue), t.hover);
    div()
        .relative()
        .h(px(height))
        .w_full()
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(r::MD))
        .when(on, |d| d.bg(fill).child(pill(hue, height)))
        .when(!on, |d| d.hover(move |s| s.bg(hover)))
}

/// A launcher list row in its `ROW` slot: icon, title, a subtitle that gives way first, and
/// an accessory on the right.
pub fn list_row(icon: AnyElement, title: &str, subtitle: &str, accessory: Option<AnyElement>, on: bool, hue: Hsla, t: &Tokens) -> Div {
    div().h(px(ROW)).w_full().px(px(8.)).flex().items_center().child(
        selectable(ROW - 4., on, hue, t)
            .gap(px(12.))
            .child(icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().min_w(px(48.)).flex_shrink_0().max_w(gpui::relative(0.7)).truncate().text_color(t.text).child(title.to_string()))
                    .when(!subtitle.is_empty(), |d| d.child(div().min_w_0().flex_1().truncate().text_size(px(ts::BODY_SM)).text_color(t.text_muted).child(subtitle.to_string()))),
            )
            .children(accessory),
    )
}

/// A keyboard key cap; "↵", "↓" and "↑" draw vector icons.
pub fn key_cap(label: &str, t: &Tokens) -> Div {
    let cap = div()
        .h(px(CAP))
        .min_w(px(CAP))
        .px(px(5.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(r::XS))
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .font_family(FONT_SMALL)
        .text_size(px(ts::CAPTION))
        .text_color(t.text_muted);
    match label {
        "↵" => cap.child(icon_cap(g::RETURN, t.text_muted)),
        "↓" => cap.child(icon_cap(g::DOWN, t.text_muted)),
        "↑" => cap.child(icon_cap(g::UP, t.text_muted)),
        _ => cap.child(label.to_string()),
    }
}

/// A hotkey like "Ctrl+Shift+V" as caps.
pub fn key_caps(spec: &str, t: &Tokens) -> Div {
    div().flex().flex_none().items_center().gap(px(4.)).children(spec.split('+').map(|k| key_cap(k, t)))
}

/// "Actions  Ctrl K": a label followed by caps.
pub fn shortcut(label: &str, keys: &[&str], t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(div().pr(px(4.)).text_color(t.text).child(label.to_string()))
        .children(keys.iter().map(|k| key_cap(k, t)))
}

pub fn vdivider(t: &Tokens) -> Div {
    div().w(px(1.)).h(px(16.)).bg(t.hairline)
}

/// Section header: a label and a rule, so its row-height slot reads as a divider (rows are
/// virtualised, so the slot is as tall as a row).
pub fn section_header(title: &str, count: Option<usize>, t: &Tokens) -> Div {
    div()
        .h(px(ROW))
        .w_full()
        .px(px(16.))
        .pt(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .font_family(FONT_SMALL)
        .text_size(px(ts::SMALL))
        .text_color(t.text_faint)
        .child(div().flex_none().font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_string()))
        .children(count.map(|n| div().flex_none().child(n.to_string())))
        .child(div().ml(px(4.)).flex_1().h(px(1.)).bg(t.hairline))
}

/// Centred icon, title and hint for empty views.
pub fn empty_state(glyph: &'static str, title: &str, hint: &str, t: &Tokens) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(icon(glyph, ic::XL, t.text_faint))
        .child(div().pt(px(8.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.text).child(title.to_string()))
        .children((!hint.is_empty()).then(|| div().text_size(px(ts::BODY_SM)).text_color(t.text_muted).child(hint.to_string())))
}

/// The focus ring: a soft halo in the control's own colour.
fn ring(color: Hsla) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow::new(px(0.), px(0.), Hsla { alpha: 0.32, ..color }).spread_radius(px(2.))]
}

/// The one input box: launcher forms, settings text, hotkey capture. Focus shows as an
/// accent border plus the ring.
pub fn field(focused: bool, t: &Tokens) -> Div {
    div()
        .h(px(CONTROL))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(r::SM))
        .bg(t.surface)
        .border_1()
        .border_color(if focused { t.accent } else { t.border })
        .when(focused, |d| d.shadow(ring(t.accent)))
}

#[derive(Clone, Copy, PartialEq)]
pub enum Button {
    /// The one solid accent fill on a screen.
    Primary,
    Quiet,
    Danger,
}

/// The one button. `focused` adds the same ring as a focused field.
pub fn button(label: &str, kind: Button, focused: bool, t: &Tokens) -> Div {
    let (bg, text, border) = match kind {
        Button::Primary => (t.accent, t.accent_text, t.accent),
        Button::Danger => (t.danger, on_color(t.danger), t.danger),
        Button::Quiet => (t.surface, t.text, t.border),
    };
    div()
        .h(px(CONTROL))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(r::SM))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_size(px(ts::BODY_SM))
        .text_color(text)
        .when(focused, |d| d.shadow(ring(if kind == Button::Quiet { t.accent } else { border })))
        .child(label.to_string())
}

/// On/off switch for settings.
pub fn toggle(on: bool, t: &Tokens) -> Div {
    div()
        .w(px(36.))
        .h(px(CAP))
        .flex_none()
        .rounded_full()
        .p(px(2.))
        .flex()
        .when(on, |d| d.justify_end())
        .bg(if on { t.accent } else { t.surface_strong })
        .child(div().size(px(16.)).rounded_full().bg(if on { t.accent_text } else { t.text_muted }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_name_is_served() {
        for name in g::ALL {
            for (suffix, stroke) in [("", "1.75"), ("@xl", "1.25"), ("@cap", "2")] {
                let svg = Icons.load(&format!("icons/{name}{suffix}.svg")).unwrap().unwrap_or_else(|| panic!("no icon {name}{suffix}"));
                assert!(String::from_utf8_lossy(&svg).contains(&format!("stroke-width=\"{stroke}\"")), "{name}{suffix}");
            }
        }
        assert!(Icons.load("icons/nope.svg").unwrap().is_none());
    }

    /// The family name the embedded font reports is the one `FONT_UI` asks for.
    #[test]
    fn space_grotesk_registers_under_font_ui() {
        gpui_ce_platform::application().run(|cx| {
            register_fonts(cx);
            assert!(cx.text_system().all_font_names().iter().any(|n| n == FONT_UI));
            cx.quit();
        });
    }

    fn ratio(a: f32, b: f32) -> f32 {
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// `top` drawn over an opaque `under`.
    fn over(top: Hsla, under: Hsla) -> Hsla {
        let (t, u, a) = (gpui::hsla_to_rgba(top), gpui::hsla_to_rgba(under), top.alpha);
        let mix = |x: f32, y: f32| x * a + y * (1. - a);
        gpui::rgb_to_hsla(gpui::Rgba::new(mix(t.red, u.red), mix(t.green, u.green), mix(t.blue, u.blue), 1.))
    }

    #[test]
    fn every_theme_meets_aa_contrast() {
        let fails = std::cell::RefCell::new(vec![]);
        for (name, dark) in DARK_THEMES.iter().map(|n| (n, true)).chain(LIGHT_THEMES.iter().map(|n| (n, false))) {
            let t = Tokens::named(name, dark);
            let body = luminance(t.bg);
            let check = |what: &str, r: f32| {
                if r < 4.5 {
                    fails.borrow_mut().push(format!("{name}: {what} is {r:.2}:1"));
                }
            };
            check("text on the accent", ratio(luminance(t.accent), luminance(t.accent_text)));
            check("faint text on the body", ratio(luminance(t.text_faint), body));
            // The selected row in the accent and in every kind hue; a card; a selected card row.
            let hues = [Tint::Accent, Tint::System, Tint::Window, Tint::Paste, Tint::Web, Tint::File, Tint::Calc, Tint::Danger];
            for hue in hues.map(|k| k.hue(&t)) {
                check("muted text on a selected row", ratio(luminance(t.text_muted), luminance(over(t.selected(hue), t.bg))));
            }
            let card = over(t.surface, t.bg);
            check("muted text on a card", ratio(luminance(t.text_muted), luminance(card)));
            check("faint text on a card", ratio(luminance(t.text_faint), luminance(card)));
            check("muted text on a selected card row", ratio(luminance(t.text_muted), luminance(over(t.selected(t.accent), card))));
        }
        assert!(fails.borrow().is_empty(), "{:#?}", fails.borrow());
    }

    fn themes() -> impl Iterator<Item = (&'static str, Tokens)> {
        DARK_THEMES.iter().map(|n| (*n, true)).chain(LIGHT_THEMES.iter().map(|n| (*n, false))).map(|(n, dark)| (n, Tokens::named(n, dark)))
    }

    /// The armed line in the action bar is `warning` text on the body; a Danger button's
    /// label is `on_color(danger)` on `danger`.
    #[test]
    fn status_colours_meet_aa_as_text() {
        let mut fails = vec![];
        for (name, t) in themes() {
            for (what, r) in [
                ("warning text on the body", ratio(luminance(t.warning), luminance(t.bg))),
                ("text on a danger button", ratio(luminance(on_color(t.danger)), luminance(t.danger))),
            ] {
                if r < 4.5 {
                    fails.push(format!("{name}: {what} is {r:.2}:1"));
                }
            }
        }
        assert!(fails.is_empty(), "{fails:#?}");
    }

    /// A tile's glyph is the row's icon, so it holds 3:1 against its own wash: at rest on the
    /// body and on a panel, and in a selected row.
    #[test]
    fn tile_glyphs_stand_out_from_their_wash() {
        let mut fails = vec![];
        for (name, t) in themes() {
            for tint in [Tint::Accent, Tint::System, Tint::Window, Tint::Paste, Tint::Web, Tint::File, Tint::Calc, Tint::Danger] {
                let row = over(t.selected(tint.hue(&t)), t.bg);
                for (what, selected, under) in [("at rest", false, t.bg), ("on a panel", false, t.bg_elevated), ("selected", true, row)] {
                    let (wash, glyph) = tile_colors(&t, Some(tint), selected);
                    let r = ratio(luminance(glyph), luminance(over(wash, under)));
                    if r < 3. {
                        fails.push(format!("{name}: {tint:?} glyph {what} is {r:.2}:1"));
                    }
                }
            }
        }
        assert!(fails.is_empty(), "{fails:#?}");
    }
}
