//! The Focus HUD (FocusForm card): while a session runs, the timer pill at the top centre of the
//! main screen, on top of every window; when its time is up, the notice at the bottom right of
//! the work area, where Windows notifications sit. Like the toast, each is a window made once at
//! start and cloaked when unused. Neither ever takes focus (`win::make_hud`), and each window is
//! sized to its card, so nothing beside the card takes clicks. DWM rounds the window and draws
//! the shadow around it (the card's own `float` falls outside the window).

use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use gpui::{
    AnyElement, AsyncApp, Bounds, ClickEvent, Context, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, prelude::*, size,
};
use windows::Win32::Foundation::{HWND, RECT};

use crate::{
    config,
    design::{self, Button, Tint, g, ic, px, sp, ts},
    search::Icon,
    win::{self, Msg},
};

/// What a click on the HUD asks the launcher for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Click {
    /// The pill: the launcher, on the session's row.
    Open,
    Extend,
    /// Complete, or the notice's ×.
    Complete,
}

/// What the HUD shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The pill's time and goal.
    pub pill: Option<(String, String)>,
    pub notice: bool,
    pub theme: config::Theme,
}

/// A mouse that moves this far (logical px) with the button down drags the pill, not clicks it.
const DRAG: f32 = 4.;
/// A few frames: a window shown again paints at its new size before it is placed.
const SETTLE: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Pill,
    Notice,
}

pub struct View {
    spot: Spot,
    time: String,
    goal: String,
    theme: config::Theme,
    hwnd: HWND,
    tx: UnboundedSender<Msg>,
    shown: bool,
    /// Where the mouse went down on the pill, until it moves (a drag) or comes up (a click).
    down: Option<Point<Pixels>>,
}

impl Render for View {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The launcher's scale: the scale is one global that each window sets as it renders, and
        // this one sets the same value the launcher does.
        design::scale_launcher();
        let t = design::Tokens::from_theme(&self.theme, window);
        let card = match self.spot {
            Spot::Pill => self.pill(&t, cx),
            Spot::Notice => self.notice(&t),
        };
        let (hwnd, spot) = (self.hwnd, self.spot);
        div().size_full().flex().items_start().font_family(design::FONT_UI).child(card).on_children_prepainted(move |b, window, cx| {
            let (Some(b), s) = (b.first(), window.scale_factor()) else { return };
            let (w, h) = ((f32::from(b.size.width) * s).round() as i32, (f32::from(b.size.height) * s).round() as i32);
            let cur = win::rect(hwnd);
            if (cur.right - cur.left, cur.bottom - cur.top) != (w, h) {
                // The window follows its card, between updates (it sends messages synchronously).
                cx.spawn(async move |_| win::set_rect(hwnd, refit(spot, win::rect(hwnd), w, h))).detach();
            }
        })
    }
}

impl View {
    /// Mouse down then a move drags it; down and up without one opens the launcher.
    fn pill(&self, t: &design::Tokens, cx: &mut Context<Self>) -> AnyElement {
        design::hud(&self.time, &self.goal, t)
            .flex_none()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, _, _| v.down = Some(e.position)),
            )
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, window, _| {
                if e.dragging() && v.down.is_some_and(|d| f32::from((e.position.x - d.x).abs()).max(f32::from((e.position.y - d.y).abs())) > DRAG) {
                    // No longer a click: the drag's own mouse-up never reaches the pill.
                    v.down = None;
                    window.start_window_move();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, _| {
                    if v.down.take().is_some() {
                        _ = v.tx.unbounded_send(Msg::Hud(Click::Open));
                    }
                }),
            )
            .into_any_element()
    }

    /// `.pc-notice`: tile, the two lines, Extend and Complete stacked on the right, and a ×.
    fn notice(&self, t: &design::Tokens) -> AnyElement {
        let click = |c: Click| {
            let tx = self.tx.clone();
            move |_: &ClickEvent, _: &mut Window, _: &mut gpui::App| _ = tx.unbounded_send(Msg::Hud(c))
        };
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(sp::S2))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.text).child("Focus session is up."))
            .child(div().text_color(t.text_muted).child("Extend it if you need more time, or complete the session."));
        let buttons = div()
            .flex()
            .flex_col()
            .gap(px(sp::S4))
            .mr(px(sp::S16))
            .child(design::button("Extend", Button::Quiet, false, t).id("extend").on_click(click(Click::Extend)))
            .child(design::button("Complete", Button::Primary, false, t).id("complete").on_click(click(Click::Complete)));
        // `.pc-notice`'s border is the plain `border`, not `elevated`'s rim gradient.
        design::elevated(t)
            .border_color(t.border)
            .flex_row()
            .flex_none()
            .w(px(design::NOTICE_W))
            .p(px(sp::S12))
            .items_start()
            .gap(px(sp::S12))
            .text_size(px(ts::BODY_SM))
            .child(design::icon_tile(&Icon::Vector(g::SHIELD_CHECK), t, false, Some(Tint::System)))
            .child(text)
            .child(buttons)
            .child(div().id("close").absolute().top(px(sp::S8)).right(px(sp::S8)).flex().child(design::icon(g::CANCEL, ic::MD, t.text_muted)).on_click(click(Click::Complete)))
            .into_any_element()
    }
}

/// The pill's and the notice's windows, and what they showed last.
#[derive(Default)]
pub struct Hud {
    pill: Option<WindowHandle<View>>,
    notice: Option<WindowHandle<View>>,
    last: State,
}

impl Hud {
    /// Both windows, cloaked. Made at start, not on first use: gpui places a new window a
    /// moment after showing it (see `toast::open`).
    pub fn open(cx: &mut AsyncApp, tx: UnboundedSender<Msg>) -> Self {
        Self { pill: window(cx, Spot::Pill, tx.clone()), notice: window(cx, Spot::Notice, tx), last: State::default() }
    }

    /// Shows what `state` says. Call between updates (it moves windows).
    pub fn sync(&mut self, cx: &mut AsyncApp, state: State) {
        if state == self.last {
            return;
        }
        let (time, goal) = state.pill.clone().unwrap_or_default();
        for (slot, on) in [(self.pill, state.pill.is_some()), (self.notice, state.notice)] {
            let Some(handle) = slot else { continue };
            let Ok((hwnd, was)) = handle.update(cx, |v, _, cx| {
                (v.time, v.goal, v.theme) = (time.clone(), goal.clone(), state.theme.clone());
                if !on {
                    v.down = None;
                }
                cx.notify();
                (v.hwnd, std::mem::replace(&mut v.shown, on))
            }) else {
                continue;
            };
            match (was, on) {
                (false, true) => reveal(cx, handle),
                (true, false) => win::cloak(hwnd, true),
                _ => {}
            }
        }
        self.last = state;
    }
}

/// Once a window just shown has painted at its size: placed at its spot and uncloaked, unless
/// it was hidden again meanwhile.
fn reveal(cx: &mut AsyncApp, handle: WindowHandle<View>) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SETTLE).await;
        if let Ok(Some((hwnd, spot))) = handle.update(cx, |v, _, _| v.shown.then_some((v.hwnd, v.spot))) {
            let r = win::rect(hwnd);
            win::set_rect(hwnd, home(spot, &win::screen("primary"), r.right - r.left, r.bottom - r.top));
            win::cloak(hwnd, false);
        }
    })
    .detach();
}

fn window(cx: &mut AsyncApp, spot: Spot, tx: UnboundedSender<Msg>) -> Option<WindowHandle<View>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(gpui::px(0.), gpui::px(0.)), size: size(gpui::px(design::NOTICE_W), gpui::px(design::HUD)) })),
        titlebar: None,
        kind: WindowKind::PopUp,
        // Shown at once, cloaked straight after: gpui shows it with SW_SHOWNOACTIVATE only
        // here. `show: false` then `set_visible` replays a placement that activates it.
        show: true,
        focus: false,
        windows_window_background: gpui::WindowsWindowBackground::Transparent,
        ..Default::default()
    };
    let view = |tx| View { spot, time: String::new(), goal: String::new(), theme: config::Theme::default(), hwnd: HWND::default(), tx, shown: false, down: None };
    let handle = cx.open_window(options, |_, cx| cx.new(|_| view(tx))).ok()?;
    handle
        .update(cx, |v, window, _| {
            v.hwnd = win::hwnd(window);
            win::make_tool_window(v.hwnd);
            win::make_hud(v.hwnd);
            win::round_corners(v.hwnd);
            win::cloak(v.hwnd, true);
        })
        .ok()?;
    Some(handle)
}

/// Where a window of w × h (physical px) shows: the pill `sp::S8` below the top of the work area,
/// centred; the notice `sp::S24` in from its bottom-right corner.
fn home(spot: Spot, screen: &win::Screen, w: i32, h: i32) -> RECT {
    let RECT { left, top, right, bottom } = screen.work;
    let (x, y) = match spot {
        Spot::Pill => (left + (right - left - w) / 2, top + (sp::S8 * screen.dpi).round() as i32),
        Spot::Notice => {
            let m = (sp::S24 * screen.dpi).round() as i32;
            (right - m - w, bottom - m - h)
        }
    };
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

/// `cur` resized to w × h: the pill keeps its top centre (wherever it was dragged), the notice
/// its bottom-right corner.
fn refit(spot: Spot, cur: RECT, w: i32, h: i32) -> RECT {
    match spot {
        Spot::Pill => {
            let left = (cur.left + cur.right) / 2 - w / 2;
            RECT { left, top: cur.top, right: left + w, bottom: cur.top + h }
        }
        Spot::Notice => RECT { left: cur.right - w, top: cur.bottom - h, right: cur.right, bottom: cur.bottom },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pill_sits_top_centre_and_the_notice_bottom_right_and_each_keeps_its_anchor() {
        let screen = win::Screen { work: RECT { left: 0, top: 0, right: 2000, bottom: 1000 }, dpi: 1.5 };
        assert_eq!(home(Spot::Pill, &screen, 200, 48), RECT { left: 900, top: 12, right: 1100, bottom: 60 });
        assert_eq!(home(Spot::Notice, &screen, 660, 120), RECT { left: 1304, top: 844, right: 1964, bottom: 964 });
        let dragged = RECT { left: 100, top: 300, right: 300, bottom: 348 };
        assert_eq!(refit(Spot::Pill, dragged, 240, 48), RECT { left: 80, top: 300, right: 320, bottom: 348 }, "grows about its centre, where it was dragged");
        assert_eq!(refit(Spot::Notice, RECT { left: 1304, top: 844, right: 1964, bottom: 964 }, 660, 140).top, 824, "grows upward");
    }
}
