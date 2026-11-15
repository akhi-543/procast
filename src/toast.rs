//! The toast: a one-line result of a command (for now, why it failed), shown above the taskbar
//! for a few seconds, then gone. It never takes focus and lets clicks through. Problems that
//! need fixing stay as the launcher's notice instead.

use std::time::Duration;

use gpui::{Animation, AnimationExt, AsyncApp, Bounds, Context, Render, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px, size};
use windows::Win32::Foundation::HWND;

use crate::{config, design, win};

/// The window around the pill, logical px: room for the 480 px pill, its shadow above, and the
/// 24 px gap to the taskbar below (the window's bottom edge is the work area's).
const W: f32 = 560.;
const TOP: f32 = 16.;
const GAP: f32 = 24.;
const H: f32 = TOP + design::ROW + GAP;

const IN: Duration = Duration::from_millis(150);
/// Errors stay 4 s: they take longer to read than a "done".
const HOLD: Duration = Duration::from_secs(4);
const OUT: Duration = Duration::from_millis(200);

pub struct Toast {
    text: String,
    theme: config::Theme,
    /// Bumped per toast: restarts the animation, and lets an older timer see it was replaced.
    id: u64,
    leaving: bool,
    hwnd: HWND,
}

impl Render for Toast {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = design::Tokens::from_theme(&self.theme, window);
        let pill = design::toast(&self.text, &t);
        // ponytail: gpui ignores the Windows "show animations" setting, so the rise always runs;
        // read SPI_GETCLIENTAREAANIMATION into cx.set_reduce_motion if that matters.
        let pill = if self.leaving {
            pill.with_animation(format!("toast-out-{}", self.id), Animation::new(OUT), |p, d| p.opacity(1. - d)).into_any_element()
        } else {
            pill.with_animation(format!("toast-in-{}", self.id), Animation::new(IN), |p, d| p.opacity(d).mt(px(8. * (1. - d)))).into_any_element()
        };
        div().size_full().flex().justify_center().pt(px(TOP)).child(pill)
    }
}

/// Shows `text` as a toast, replacing any that is showing. Call between updates (it moves a
/// window, and Windows answers that synchronously through gpui's window proc).
pub fn show(cx: &mut AsyncApp, slot: Option<WindowHandle<Toast>>, text: String, theme: config::Theme) {
    let Some(handle) = slot else { return };
    let Ok(hwnd) = handle.update(cx, |this, _, _| this.hwnd) else { return };
    win::place_toast(hwnd, &win::screen("active"), W, H);
    let Ok(id) = handle.update(cx, |this, _, cx| {
        this.id += 1;
        (this.text, this.theme, this.leaving) = (text, theme, false);
        win::cloak(this.hwnd, false);
        cx.notify();
        this.id
    }) else {
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(IN + HOLD).await;
        _ = handle.update(cx, |this, _, cx| {
            if this.id == id {
                this.leaving = true;
                cx.notify();
            }
        });
        cx.background_executor().timer(OUT).await;
        _ = handle.update(cx, |this, _, _| {
            if this.id == id {
                win::cloak(this.hwnd, true);
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
    let handle = cx.open_window(options, |_, cx| cx.new(|_| Toast { text: String::new(), theme: config::Theme::default(), id: 0, leaving: false, hwnd: HWND::default() })).ok()?;
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
