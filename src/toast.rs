//! The toast: a one-line result of a command, shown above the taskbar. A done or info toast
//! closes after 2 s; a sticky one (errors, by default) stays until Esc, a click or the launcher
//! opening. It never takes focus; an auto toast lets clicks through. Problems that need fixing
//! stay as the launcher's notice instead.

use std::time::Duration;

use gpui::{Animation, AnimationExt, AsyncApp, Bounds, Context, MouseButton, Render, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px, size};
use windows::Win32::Foundation::HWND;

use crate::{config, design, keys, win};

/// The window around the pill, logical px: room for the 480 px pill, its shadow above, and the
/// 24 px gap to the taskbar below (the window's bottom edge is the work area's).
const W: f32 = 560.;
const TOP: f32 = 16.;
const GAP: f32 = 24.;
const H: f32 = TOP + design::ROW + GAP;

const IN: Duration = Duration::from_millis(150);
/// How long an auto toast stays.
const HOLD: Duration = Duration::from_secs(2);
const OUT: Duration = Duration::from_millis(200);
/// Longest text, in characters (spec: Toast copy).
const MAX: usize = 60;

/// What the toast says, which picks its icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Done,
    Error,
    Info,
}

/// Whether the toast closes on its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stay {
    /// Closes after `HOLD`.
    Auto,
    /// Stays until Esc, a click or the launcher opening.
    Sticky,
}

impl Kind {
    /// Errors stay until dismissed: they say what to do next, which takes longer to read.
    pub fn stay(self) -> Stay {
        if self == Kind::Error { Stay::Sticky } else { Stay::Auto }
    }
}

/// `text` cut to `MAX` characters, the last an ellipsis when cut.
pub fn clip(text: &str) -> String {
    if text.chars().count() <= MAX { text.to_string() } else { text.chars().take(MAX - 1).chain(['…']).collect() }
}

pub struct Toast {
    text: String,
    kind: Kind,
    stay: Stay,
    theme: config::Theme,
    /// Bumped per toast: restarts the animation, and lets an older timer see it was replaced.
    id: u64,
    leaving: bool,
    hwnd: HWND,
}

impl Render for Toast {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = design::Tokens::from_theme(&self.theme, window);
        let pill = design::toast(&self.text, self.kind, &t);
        // ponytail: gpui ignores the Windows "show animations" setting, so the rise always runs;
        // read SPI_GETCLIENTAREAANIMATION into cx.set_reduce_motion if that matters.
        let pill = if self.leaving {
            pill.with_animation(format!("toast-out-{}", self.id), Animation::new(OUT), |p, d| p.opacity(1. - d)).into_any_element()
        } else {
            pill.with_animation(format!("toast-in-{}", self.id), Animation::new(IN), |p, d| p.opacity(d).mt(px(8. * (1. - d)))).into_any_element()
        };
        // Only a sticky toast gets clicks (an auto one lets them through).
        div().size_full().flex().justify_center().pt(px(TOP)).child(pill).on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| _ = this.dismiss(cx)))
    }
}

impl Toast {
    /// Closes a sticky toast that is showing, returning what it said.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> Option<(Kind, String)> {
        (self.stay == Stay::Sticky && !self.leaving).then(|| {
            self.leave(cx);
            (self.kind, self.text.clone())
        })
    }

    /// Fades out, then hides, unless a newer toast replaced this one by then.
    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.stay == Stay::Sticky {
            keys::sticky(false);
            win::click_through(self.hwnd, true);
        }
        self.leaving = true;
        cx.notify();
        let id = self.id;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OUT).await;
            _ = this.update(cx, |this, _| {
                if this.id == id {
                    win::cloak(this.hwnd, true);
                }
            });
        })
        .detach();
    }
}

/// Shows `text` as a toast, replacing any that is showing. Call between updates (it moves a
/// window, and Windows answers that synchronously through gpui's window proc).
pub fn show(cx: &mut AsyncApp, slot: Option<WindowHandle<Toast>>, kind: Kind, stay: Stay, text: &str, theme: config::Theme) {
    let Some(handle) = slot else { return };
    let Ok(hwnd) = handle.update(cx, |this, _, _| this.hwnd) else { return };
    win::place_toast(hwnd, &win::screen("active"), W, H);
    let text = clip(text.lines().next().unwrap_or_default());
    let Ok(id) = handle.update(cx, |this, _, cx| {
        this.id += 1;
        (this.text, this.kind, this.stay, this.theme, this.leaving) = (text, kind, stay, theme, false);
        let sticky = stay == Stay::Sticky;
        keys::sticky(sticky);
        win::click_through(this.hwnd, !sticky);
        win::cloak(this.hwnd, false);
        cx.notify();
        this.id
    }) else {
        return;
    };
    if stay == Stay::Sticky {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(IN + HOLD).await;
        _ = handle.update(cx, |this, _, cx| {
            if this.id == id {
                this.leave(cx);
            }
        });
    })
    .detach();
}

/// The toast window, made once at start and kept cloaked between toasts (like the launcher).
/// Not on first use: gpui places a new window a moment after showing it, which would undo
/// `place_toast` for that first toast.
pub fn open(cx: &mut AsyncApp) -> Option<WindowHandle<Toast>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(W), px(H)) })),
        titlebar: None,
        kind: WindowKind::PopUp,
        show: false,
        focus: false,
        windows_window_background: gpui::WindowsWindowBackground::Transparent,
        ..Default::default()
    };
    let handle = cx.open_window(options, |_, cx| cx.new(|_| Toast { text: String::new(), kind: Kind::Info, stay: Stay::Auto, theme: config::Theme::default(), id: 0, leaving: false, hwnd: HWND::default() })).ok()?;
    handle
        .update(cx, |this, window, _| {
            this.hwnd = win::hwnd(window);
            win::make_tool_window(this.hwnd);
            win::make_overlay(this.hwnd);
            win::cloak(this.hwnd, true);
            window.set_visible(true);
        })
        .ok()?;
    Some(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_60_and_cuts_longer() {
        let n = |n: usize| "x".repeat(n);
        assert_eq!(clip(&n(59)), n(59));
        assert_eq!(clip(&n(60)), n(60));
        assert_eq!(clip(&n(61)), format!("{}…", n(59)));
        assert_eq!(clip(&n(61)).chars().count(), 60);
    }

    #[test]
    fn errors_stay_until_dismissed_others_close_on_their_own() {
        assert_eq!(Kind::Error.stay(), Stay::Sticky);
        assert_eq!(Kind::Done.stay(), Stay::Auto);
        assert_eq!(Kind::Info.stay(), Stay::Auto);
    }
}
