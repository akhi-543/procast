//! Screen Time's dashboard in the launcher: one scrolling column of cards over the period's
//! stats, then its Apps and Websites rows (`Launcher::items`; item 0 is the anchor).

use gpui::{AnyElement, Bounds, Context, FontWeight, Hsla, PaintQuad, PathBuilder, Window, canvas, div, point, prelude::*, size};
use serde_json::Value;

use crate::{
    Launcher, Mode,
    calendar::{civil_from_days, weekday},
    design::{self, Tokens, g, ic, px, r, ts},
    focus,
    search::Item,
    wellbeing::{self, Period, Range, duration},
    win,
};

/// The column's children before the rows: header, total, categories, tiles, productive,
/// comparison, past year (`row_child`).
const CARDS: usize = 7;
const CHART_H: f32 = 96.;
const BAR_GAP: f32 = 3.;
const LINE_H: f32 = 56.;
const CELL: f32 = 10.;
const CELL_GAP: f32 = 2.;
const WEEKS: usize = 53;

/// Slots 0..7 (`wellbeing::SLOTS`), 7 the spare, 8 Other's grey.
const LIGHT: [u32; 9] = [0x2a78d6, 0xeb6834, 0x1baf7a, 0xeda100, 0xe87ba4, 0x008300, 0x4a3aa7, 0xe34948, 0xaeaeb2];
const DARK: [u32; 9] = [0x3987e5, 0xd95926, 0x199e70, 0xc98500, 0xd55181, 0x008300, 0x9085e9, 0xe66767, 0x636366];
const HEAT_LIGHT: [u32; 5] = [0xe8e8ed, 0x9ec5f4, 0x5598e7, 0x256abf, 0x0d366b];
const HEAT_DARK: [u32; 5] = [0x2c2c2e, 0x104281, 0x1c5cab, 0x3987e5, 0x9ec5f4];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn hex(v: u32) -> Hsla {
    gpui::rgb_to_hsla(gpui::rgb(v))
}

/// A category's colour from its `wellbeing::color_slot`; None is Other's grey.
pub(crate) fn category_color(slot: Option<usize>, dark: bool) -> Hsla {
    hex(if dark { DARK } else { LIGHT }[slot.unwrap_or(8).min(8)])
}

/// A heatmap shade, 0..=4.
pub(crate) fn heat_color(level: u8, dark: bool) -> Hsla {
    hex(if dark { HEAT_DARK } else { HEAT_LIGHT }[usize::from(level.min(4))])
}

/// The label under bar `i`: every 6 hours on a day, every day of a week, every 7th of a month.
fn bar_label(range: Range, i: usize) -> Option<String> {
    match range {
        Range::Day => i.is_multiple_of(6).then(|| format!("{} {}", if i.is_multiple_of(12) { 12 } else { i % 12 }, if i < 12 { "AM" } else { "PM" })),
        Range::Week => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].get(i).map(|d| d.to_string()),
        Range::Month => i.is_multiple_of(7).then(|| (i + 1).to_string()),
    }
}

/// "+12m", "−37m", or "No change" when it rounds to nothing.
fn difference(now: u64, before: u64) -> String {
    let gap = duration(now.abs_diff(before) as f64);
    if gap == "0m" { "No change".into() } else { format!("{}{gap}", if now > before { "+" } else { "−" }) }
}

/// `(column, month)` over the heatmap's week columns from Monday `from`: a label where the
/// Monday's month changes. The first column's is dropped when the next is too close to fit.
fn month_labels(from: i64, cols: usize) -> Vec<(usize, &'static str)> {
    let month = |c: usize| civil_from_days(from + 7 * c as i64).1;
    let mut out: Vec<_> = (0..cols).filter(|&c| c == 0 || month(c) != month(c - 1)).map(|c| (c, MONTHS[month(c) as usize - 1])).collect();
    if out.len() > 1 && out[1].0 < 3 {
        out.remove(0);
    }
    out
}

/// The dashboard column's child that shows item `i`: the header for the anchor, else its row
/// past the cards and one section header per section so far.
fn row_child(items: &[Item], i: usize) -> usize {
    if i == 0 || i >= items.len() {
        return 0;
    }
    // The no-hits line is the only child after the cards: no header, no rows.
    if items[i].section.as_deref() == Some(crate::EMPTY) {
        return CARDS;
    }
    let headers = (1..=i).filter(|&k| k == 1 || items[k].section != items[k - 1].section).count();
    CARDS + i - 1 + headers
}

/// The selected row's line: its time and share, and on a day the hours it was used.
fn detail_line(stats: &Value, it: &Item, hours: &wellbeing::Hours) -> String {
    let Some(row) = wellbeing::rows(stats).into_iter().find(|r| Some(r.section) == it.section.as_deref() && r.name == it.title) else { return String::new() };
    wellbeing::detail(stats, Some(&row), hours)
        .into_iter()
        .filter_map(|(k, v)| match k.as_str() {
            "Time" | "Share" => Some(v),
            _ if k.ends_with(":00") => Some(format!("{k} {v}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn secs(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn frac(part: u64, whole: u64) -> f32 {
    if whole == 0 { 0. } else { part as f32 / whole as f32 }
}

fn card(t: &Tokens) -> gpui::Div {
    div().flex_none().w_full().mb(px(12.)).p(px(16.)).rounded(px(r::LG)).bg(t.surface).flex().flex_col().gap(px(8.))
}

fn card_title(title: &str, t: &Tokens) -> gpui::Div {
    div().text_size(px(ts::HEADING)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).truncate().child(title.to_string())
}

fn dot(color: Hsla) -> gpui::Div {
    div().size(px(8.)).flex_none().rounded_full().bg(color)
}

fn muted(text: impl Into<gpui::SharedString>, t: &Tokens) -> gpui::Div {
    div().text_size(px(ts::SMALL)).text_color(t.text_muted).child(text.into())
}

/// A thin bar `frac` full of `color`.
fn thin_bar(frac: f32, color: Hsla, t: &Tokens) -> gpui::Div {
    div().w_full().h(px(4.)).rounded_full().bg(t.surface_strong).child(div().h_full().w(gpui::relative(frac.clamp(0., 1.))).rounded_full().bg(color))
}

impl Launcher {
    /// Shows period `p`: reads it again, keeps the typed filter, and goes back to the top. The
    /// last period's numbers stay on screen until the new ones arrive. The period already shown
    /// does nothing.
    pub(crate) fn screen_time_go(&mut self, p: Period, cx: &mut Context<Self>) {
        if self.mode == Mode::ScreenTime(p) {
            return;
        }
        self.mode = Mode::ScreenTime(p);
        self.load_screen_time(p, focus::categories(&self.cfg.focus_category), cx);
        self.refresh(cx);
    }

    /// A Tab or arrow key: `to` picks the period to show from the current one and today.
    pub(crate) fn screen_time_key(&mut self, to: impl Fn(Period, i64) -> Period, cx: &mut Context<Self>) {
        if let Mode::ScreenTime(p) = self.mode {
            self.screen_time_go(to(p, win::local_now().0), cx);
        }
    }

    /// Scrolls the dashboard to the selected item.
    pub(crate) fn screen_time_scroll(&self) {
        self.st_scroll.scroll_to_item(row_child(&self.items, self.selected));
    }

    pub(crate) fn screen_time_dashboard(&self, t: &Tokens, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Mode::ScreenTime(p), Some(Ok(stats))) = (self.mode, &self.st_stats) else { return div().flex_1().into_any_element() };
        let today = win::local_now().0;
        let mut col = div()
            .id("screen-time")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.st_scroll)
            .px(px(16.))
            .pt(px(16.))
            .pb(px(4.))
            .flex()
            .flex_col()
            .child(self.st_header(p, today, t, cx))
            .child(st_total(p, today, stats, t))
            .child(st_categories(stats, t))
            .child(st_tiles(stats, t))
            .child(self.st_productive(stats, today, t))
            .child(st_compare(p, today, stats, t))
            .child(self.st_year(p, today, t));
        let mut section: Option<&str> = None;
        for (i, it) in self.items.iter().enumerate().skip(1) {
            // A filter with no hits: one line where the rows would be.
            if it.section.as_deref() == Some(crate::EMPTY) {
                col = col.child(muted(it.title.clone(), t).flex_none().px(px(8.)).py(px(8.)));
                continue;
            }
            if it.section.as_deref() != section {
                section = it.section.as_deref();
                col = col.child(design::section_header(if section == Some("Sites") { "Websites" } else { section.unwrap_or_default() }, None, t).flex_none());
            }
            let on = i == self.selected;
            let row = design::list_row(design::icon_tile(&it.icon, t, on, it.tint), &it.title, &it.subtitle, Some(it.accessory.clone().into_any_element()), design::ACCESSORY_MAX, on, t)
                .id(("st-row", i))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.selected = i;
                        this.launch(window, cx);
                    }),
                );
            // Under the row's title: past the list row's two paddings, its icon and the gap.
            let line = on.then(|| detail_line(stats, it, &self.st_hours)).filter(|s| !s.is_empty());
            col = col.child(div().flex_none().flex().flex_col().child(row).children(line.map(|s| muted(s, t).truncate().pl(px(56.)).pb(px(4.)))));
        }
        col.into_any_element()
    }

    /// Day / Week / Month, the arrows and the period's name.
    fn st_header(&self, p: Period, today: i64, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let segments = [(Range::Day, "Day"), (Range::Week, "Week"), (Range::Month, "Month")]
            .map(|(range, name)| design::segment(name, p.range == range, t).id(name).on_click(cx.listener(move |this, _, _, cx| this.screen_time_go(wellbeing::with_range(p, range), cx))));
        let arrow = |id: &'static str, glyph: &'static str, to: Period| {
            // Never past today: the step returns the same period, and the arrow is dimmed.
            let on = to != p;
            let hover = t.hover;
            div()
                .id(id)
                .size(px(design::CONTROL))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(r::SM))
                .child(design::icon(glyph, ic::MD, if on { t.text_muted } else { t.text_faint }))
                .when(on, |d| d.hover(move |s| s.bg(hover)).on_click(cx.listener(move |this, _, _, cx| this.screen_time_go(to, cx))))
        };
        let (prev, next) = (arrow("st-prev", g::BACK, wellbeing::step(p, -1, today)), arrow("st-next", g::FORWARD, wellbeing::step(p, 1, today)));
        div()
            .flex_none()
            .w_full()
            .mb(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(design::segment_track(segments, t).flex_none())
            .child(div().flex_1())
            .child(prev)
            .child(div().min_w(px(120.)).flex_none().flex().justify_center().text_size(px(ts::HEADING)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(wellbeing::title(p, today)))
            .child(next)
    }

    /// The share bar and legend, then the productive share of the past 12 weeks as a line.
    fn st_productive(&self, stats: &Value, today: i64, t: &Tokens) -> impl IntoElement {
        let (good, bad) = (&self.cfg.screen_time.productive, &self.cfg.screen_time.distracting);
        let sum = |names: &[String]| stats["categories"].as_array().into_iter().flatten().filter(|c| c["name"].as_str().is_some_and(|n| names.iter().any(|x| x == n))).map(|c| secs(&c["seconds"])).sum::<u64>();
        let (total, prod, dist) = (secs(&stats["total"]), sum(good), sum(bad));
        let neutral = total.saturating_sub(prod + dist);
        // Productive is slot 0's blue, distracting the spare red.
        let (pc, dc) = (category_color(Some(0), t.dark), category_color(Some(7), t.dark));
        let first_monday = today - weekday(today) - 11 * 7;
        let shares = self.st_days.as_ref().map_or_else(|| vec![None; 12], |d| wellbeing::week_shares(d, good, first_monday, 12));
        let rule = t.hairline;
        let line = canvas(
            |_, _, _| {},
            move |b: Bounds<gpui::Pixels>, (), window, _| {
                let pad = px(4.);
                let (w, h) = (b.size.width - pad * 2., b.size.height - pad * 2.);
                let at = |i: usize, s: f32| point(b.left() + pad + w * (i as f32 / (shares.len() - 1).max(1) as f32), b.top() + pad + h * (1. - s));
                for f in [0., 0.5, 1.] {
                    window.paint_quad(PaintQuad::new(Bounds::new(point(b.left(), at(0, f).y), size(b.size.width, gpui::px(1.)))).background(rule));
                }
                // A week under a minute has no point, so the line breaks there.
                for (i, pair) in shares.windows(2).enumerate() {
                    if let [Some(a), Some(z)] = pair {
                        let mut path = PathBuilder::stroke(px(2.));
                        path.move_to(at(i, *a as f32));
                        path.line_to(at(i + 1, *z as f32));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, pc);
                        }
                    }
                }
                let d = px(6.);
                for (i, s) in shares.iter().enumerate() {
                    if let Some(s) = s {
                        window.paint_quad(PaintQuad::new(Bounds::centered_at(at(i, *s as f32), size(d, d))).corner_radii(d / 2.).background(pc));
                    }
                }
            },
        )
        .w_full()
        .h(px(LINE_H));
        let legend = [(pc, prod, "productive"), (t.surface_strong, neutral, "neutral"), (dc, dist, "distracting")]
            .map(|(color, s, name)| div().flex().items_center().gap(px(6.)).child(dot(color)).child(muted(format!("{} {name}", duration(s as f64)), t)));
        let bar = div()
            .flex()
            .w_full()
            .h(px(8.))
            .rounded_full()
            .overflow_hidden()
            .bg(t.surface_strong)
            .child(div().h_full().w(gpui::relative(frac(prod, total))).bg(pc))
            .child(div().flex_1())
            .child(div().h_full().w(gpui::relative(frac(dist, total))).bg(dc));
        card(t)
            .child(card_title("Productive and distracting", t))
            .child(div().text_color(t.text).child(format!("{}% productive", (frac(prod, total) * 100.).round())))
            .child(bar)
            .child(div().flex().flex_wrap().gap(px(16.)).children(legend))
            .child(muted("Productive share per week, past 12 weeks", t).pt(px(8.)))
            .child(line)
            .child(div().flex().justify_between().text_size(px(ts::CAPTION)).text_color(t.text_faint).child("12 weeks ago").child("This week"))
    }

    /// 53 weeks of daily totals, Monday at the top, months over the columns; the shown
    /// period's days ringed.
    fn st_year(&self, p: Period, today: i64, t: &Tokens) -> impl IntoElement {
        let from = today - weekday(today) - 52 * 7;
        let levels = self.st_days.as_ref().map(|d| wellbeing::heat_levels(d, from, today)).unwrap_or_default();
        let (shown, _) = wellbeing::span(p, today);
        let step = CELL + CELL_GAP;
        let labels = month_labels(from, WEEKS).into_iter().map(|(c, m)| div().absolute().top(px(0.)).left(px(c as f32 * step)).whitespace_nowrap().child(m));
        let columns = (0..WEEKS as i64).map(|c| {
            div().flex().flex_col().gap(px(CELL_GAP)).children((0..7).map(|d| {
                let day = from + 7 * c + d;
                let level = levels.get((day - from) as usize).copied().unwrap_or(0);
                div()
                    .size(px(CELL))
                    .rounded(px(2.))
                    .when(day <= today, |s| s.bg(heat_color(level, t.dark)))
                    .when(day <= today && (shown.0..=shown.1).contains(&day), |s| s.border_1().border_color(t.text))
            }))
        });
        card(t)
            .child(card_title("Past year", t))
            .child(div().relative().h(px(14.)).w(px(WEEKS as f32 * step)).text_size(px(ts::CAPTION)).text_color(t.text_faint).children(labels))
            .child(div().flex().gap(px(CELL_GAP)).children(columns))
    }
}

/// The period's total and its bars, stacked by category, the biggest at the bottom.
fn st_total(p: Period, today: i64, stats: &Value, t: &Tokens) -> impl IntoElement {
    let order: Vec<&str> = stats["categories"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str()).collect();
    let buckets: Vec<Vec<(&str, u64)>> = stats["buckets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            let mut cats: Vec<(&str, u64)> = b["categories"].as_object().into_iter().flatten().map(|(n, s)| (n.as_str(), secs(s))).collect();
            cats.sort_by_key(|(n, _)| order.iter().position(|o| o == n).unwrap_or(usize::MAX));
            cats
        })
        .collect();
    let max = buckets.iter().map(|b| b.iter().map(|c| c.1).sum::<u64>()).max().unwrap_or(0).max(1);
    let bars = buckets.iter().map(|b| {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .justify_end()
            .children(b.iter().rev().map(|(n, s)| div().w_full().flex_none().h(px(CHART_H * frac(*s, max))).bg(category_color(wellbeing::color_slot(n, stats), t.dark))))
    });
    let grid = [0., 0.5, 1.].map(|f| div().absolute().left(px(0.)).right(px(0.)).top(px((CHART_H - 1.) * f)).h(px(1.)).bg(t.hairline));
    // Labels overflow their narrow bar to the right, into the unlabelled bars after them.
    let labels = (0..buckets.len()).map(|i| div().flex_1().min_w_0().whitespace_nowrap().children(bar_label(p.range, i)));
    card(t)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(div().text_size(px(ts::DISPLAY)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(duration(secs(&stats["total"]) as f64)))
                .child(div().text_size(px(ts::BODY_SM)).text_color(t.text_muted).child(wellbeing::caption(p, today))),
        )
        .children(wellbeing::versus(secs(&stats["total"]), secs(&stats["previous"]), p, today).map(|v| muted(v, t)))
        .child(div().relative().w_full().h(px(CHART_H)).children(grid).child(div().absolute().top(px(0.)).bottom(px(0.)).left(px(0.)).right(px(0.)).flex().items_end().gap(px(BAR_GAP)).children(bars)))
        .child(div().flex().gap(px(BAR_GAP)).text_size(px(ts::CAPTION)).text_color(t.text_faint).children(labels))
}

/// Every category with time, largest first: dot, name, share, time, bar.
fn st_categories(stats: &Value, t: &Tokens) -> impl IntoElement {
    let total = secs(&stats["total"]);
    let cats: Vec<(&str, u64)> = stats["categories"].as_array().into_iter().flatten().filter_map(|c| Some((c["name"].as_str()?, secs(&c["seconds"])))).collect();
    let top = cats.first().map_or(1, |c| c.1.max(1));
    let rows = cats.iter().map(|&(name, s)| {
        let color = category_color(wellbeing::color_slot(name, stats), t.dark);
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(24.))
            .child(dot(color))
            .child(div().w(px(140.)).flex_none().truncate().text_color(t.text).child(name.to_string()))
            .child(div().w(px(40.)).flex_none().text_right().text_size(px(ts::BODY_SM)).text_color(t.text_muted).child(format!("{}%", (frac(s, total) * 100.).round())))
            .child(div().w(px(64.)).flex_none().text_right().text_size(px(ts::BODY_SM)).text_color(t.text).child(duration(s as f64)))
            .child(div().flex_1().min_w_0().child(thin_bar(frac(s, top), color, t)))
    });
    card(t).child(card_title("Categories", t)).children(rows).when(cats.is_empty(), |c| c.child(muted("No screen time recorded", t)))
}

/// First use (a week's or month's earliest start), the busiest hour or day, the longest stretch
/// and app switches.
fn st_tiles(stats: &Value, t: &Tokens) -> impl IntoElement {
    let f = &stats["focus"];
    let day = stats["from"] == stats["to"];
    let (first, last) = if day { ("First use", "Last use") } else { ("Earliest start", "Latest end") };
    let busiest = wellbeing::busiest(stats);
    let tile = |label: &str, value: String, sub: Option<String>| {
        div()
            .flex_1()
            .min_w_0()
            .p(px(12.))
            .rounded(px(r::LG))
            .bg(t.surface)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(muted(label.to_string(), t).truncate())
            .child(div().text_size(px(ts::HEADING)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).truncate().child(value))
            .children(sub.map(|s| muted(s, t).truncate()))
    };
    div()
        .flex_none()
        .w_full()
        .mb(px(12.))
        .flex()
        .gap(px(12.))
        .child(tile(first, f["first"].as_str().unwrap_or("—").to_string(), f["last"].as_str().map(|l| format!("{last} {l}"))))
        .child(tile(if day { "Busiest hour" } else { "Busiest day" }, busiest.as_ref().map_or("—".into(), |b| b.0.clone()), busiest.as_ref().map(|b| duration(b.1 as f64))))
        .child(tile("Longest stretch", duration(secs(&f["longest"]) as f64), None))
        .child(tile("App switches", design::thousands(secs(&f["switches"]) as usize), None))
}

/// Each category now (solid) against the period before (faded), with the difference.
fn st_compare(p: Period, today: i64, stats: &Value, t: &Tokens) -> impl IntoElement {
    let rows = wellbeing::compare_rows(stats);
    let max = rows.iter().map(|r| r.1.max(r.2)).max().unwrap_or(0).max(1);
    let lines = rows.iter().map(|(name, now, before)| {
        let color = category_color(wellbeing::color_slot(name, stats), t.dark);
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .child(dot(color))
            .child(div().w(px(120.)).flex_none().truncate().text_color(t.text).child(name.clone()))
            .child(div().w(px(72.)).flex_none().text_right().text_size(px(ts::BODY_SM)).text_color(t.text_muted).child(difference(*now, *before)))
            .child(div().w(px(56.)).flex_none().text_right().text_size(px(ts::BODY_SM)).text_color(t.text).child(duration(*now as f64)))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(thin_bar(frac(*now, max), color, t)).child(thin_bar(frac(*before, max), design::alpha(color, 0.35), t)))
    });
    card(t).child(card_title(&format!("Compared with {}", wellbeing::before_name(p, today)), t)).children(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Act, Icon};
    use serde_json::json;

    #[test]
    fn colours_match_the_web_palette() {
        assert_eq!(category_color(Some(0), false), gpui::rgb_to_hsla(gpui::rgb(0x2a78d6)));
        assert_eq!(category_color(Some(7), true), gpui::rgb_to_hsla(gpui::rgb(0xe66767)));
        assert_eq!(category_color(None, true), gpui::rgb_to_hsla(gpui::rgb(0x636366)));
        assert_eq!(heat_color(0, false), gpui::rgb_to_hsla(gpui::rgb(0xe8e8ed)));
        assert_eq!(heat_color(4, true), gpui::rgb_to_hsla(gpui::rgb(0x9ec5f4)));
    }

    #[test]
    fn bar_labels_leave_room_between_them() {
        let day: Vec<_> = (0..24).filter_map(|i| bar_label(Range::Day, i)).collect();
        assert_eq!(day, ["12 AM", "6 AM", "12 PM", "6 PM"]);
        let week: Vec<_> = (0..7).filter_map(|i| bar_label(Range::Week, i)).collect();
        assert_eq!(week, ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);
        let month: Vec<_> = (0..31).filter_map(|i| bar_label(Range::Month, i)).collect();
        assert_eq!(month, ["1", "8", "15", "22", "29"]);
    }

    #[test]
    fn differences_are_signed() {
        assert_eq!(difference(120, 2340), "−37m");
        assert_eq!(difference(3600, 0), "+1h 0m");
        assert_eq!(difference(600, 610), "No change");
    }

    #[test]
    fn month_labels_sit_over_each_months_first_week() {
        // From Monday 8 Dec 2025: January's first Monday is the 5th, four columns on.
        let dec_8 = crate::calendar::days_from_civil(2025, 12, 8);
        assert_eq!(month_labels(dec_8, 8), [(0, "Dec"), (4, "Jan")]);
        // From Monday 29 Dec: December's one column has no room before January's.
        assert_eq!(month_labels(dec_8 + 21, 6), [(1, "Jan"), (5, "Feb")]);
    }

    #[test]
    fn a_row_scrolls_to_its_child_past_the_cards_and_headers() {
        let row = |s: Option<&str>| {
            let mut it = Item::new("x", "", Icon::Vector(g::APP), Act::None);
            it.section = s.map(Into::into);
            it
        };
        let items = [row(None), row(Some("Apps")), row(Some("Apps")), row(Some("Sites"))];
        assert_eq!((0..4).map(|i| row_child(&items, i)).collect::<Vec<_>>(), [0, CARDS + 1, CARDS + 2, CARDS + 4]);
        assert_eq!(row_child(&[row(None), row(Some(crate::EMPTY))], 1), CARDS, "the no-hits line follows the cards");
    }

    #[test]
    fn the_selected_rows_line_is_its_time_share_and_hours() {
        let stats = json!({"total": 1200, "apps": [{"name": "code.exe", "seconds": 600, "category": "Development"}], "focus": {"days": 1, "switches": 3}});
        let mut hours = wellbeing::Hours::new();
        hours.insert((wellbeing::Kind::App, "code.exe".into()), std::array::from_fn(|h| if h == 9 { 600. } else { 0. }));
        let mut it = Item::new("code.exe", "", Icon::Vector(g::APP), Act::None);
        it.section = Some("Apps".into());
        assert_eq!(detail_line(&stats, &it, &hours), "10m · 50% of total · 09:00 10m");
        assert_eq!(detail_line(&stats, &it, &Default::default()), "10m · 50% of total", "no hours outside a day");
    }
}
