//! Charts for the statistics pages: stacked columns, lines and areas, ranked
//! bars, ribbons and sparklines. Conventional marks (thin, anchored to the
//! baseline), one y-axis, a legend whenever there are two series or more, and
//! Ember only for the focal series. Moving over a chart reads its exact values
//! in the line above it; a chart with nothing in it says so.

use nsh_base::theme;
use nsh_core::canvas::LineCap;
use nsh_core::prelude::*;

use super::{self as ui, size};

/// One series of a chart.
#[derive(Clone)]
pub struct Ser {
    pub name: String,
    pub color: fn() -> AdaptiveColor,
}

/// One column: a value per series, what the bucket is called when hovered,
/// and whether it is still to come.
#[derive(Clone)]
pub struct Col {
    pub vals: Vec<f64>,
    pub tip: String,
    pub open: bool,
}

/// A nice top for an axis and its step: 1, 2, 2.5, 5 or 10 times a power of ten.
pub fn nice_axis(max: f64, ticks: usize) -> (f64, f64) {
    if max <= 0.0 || !max.is_finite() {
        return (1.0, 1.0);
    }
    let raw = max / ticks.max(1) as f64;
    let p = 10f64.powf(raw.log10().floor());
    let f = raw / p;
    let m = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 2.5 {
        2.5
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = m * p;
    (step, step * (max / step - 1e-9).ceil().max(1.0))
}

/// A legend: a swatch and a name for each series.
pub fn legend(items: Vec<(String, AdaptiveColor)>) -> impl IntoElement {
    view()
        .flex_row()
        .flex_wrap()
        .gap_x_4()
        .gap_y_1()
        .children(items.into_iter().map(|(name, c)| {
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(view().w(px(9.0)).h(px(9.0)).rounded(px(2.5)).bg(move || c))
                .child(ui::muted(name))
        }))
}

/// The line above a chart: the exact values under the pointer, or `idle`.
fn readout(text_now: impl Fn() -> String + 'static) -> impl IntoElement {
    ui::muted(move || -> SharedString { text_now().into() })
        .tabular_nums()
        .number_of_lines(2)
        .min_h(px(32.0))
}

/// The y-axis labels beside a chart of `height`: top, middle and zero.
fn y_labels(top: f64, fmt: impl Fn(f64) -> String, height: f32) -> impl IntoElement {
    let labels = [top, top / 2.0, 0.0].map(|v| fmt(v));
    view()
        .w(px(52.0))
        .h(px(height))
        .justify_between()
        .items_end()
        .pr_2()
        .children(labels.into_iter().map(|l| {
            ui::muted(l)
                .text_size(px(size::small() - 1.0))
                .tabular_nums()
        }))
}

/// The labels under a chart: `(first column, columns it spans, text)`.
pub type XLabel = (usize, usize, String);

fn x_labels(n: usize, labels: Vec<XLabel>) -> impl IntoElement {
    let mut cells: Vec<(usize, String)> = Vec::new();
    let mut at = 0;
    let mut labels = labels;
    labels.sort_by_key(|l| l.0);
    for (from, span, text) in labels {
        if from > at {
            cells.push((from - at, String::new()));
        }
        cells.push((span.min(n.saturating_sub(from)).max(1), text));
        at = from + span;
    }
    if at < n {
        cells.push((n - at, String::new()));
    }
    view()
        .flex_row()
        .flex_1()
        .children(cells.into_iter().map(|(span, text)| {
            view()
                .flex_grow(span as f32)
                .basis(px(0.0))
                .min_w(px(0.0))
                .child(
                    ui::muted(text)
                        .text_size(px(size::small() - 1.0))
                        .number_of_lines(1),
                )
        }))
}

fn frame(
    height: f32,
    top: f64,
    fmt: impl Fn(f64) -> String + 'static,
    plot: impl IntoElement,
    n: usize,
    labels: Vec<XLabel>,
) -> impl IntoElement {
    view()
        .gap_1()
        .child(
            view()
                .flex_row()
                .child(y_labels(top, fmt, height))
                .child(view().flex_1().min_w(px(0.0)).child(plot)),
        )
        .child(
            view()
                .flex_row()
                .child(view().w(px(52.0)))
                .child(x_labels(n, labels)),
        )
}

/// Stacked columns, oldest first. `fmt` writes a value (bytes, a count).
pub fn columns(
    series: Vec<Ser>,
    cols: Vec<Col>,
    fmt: fn(f64) -> String,
    labels: Vec<XLabel>,
    height: f32,
    idle: String,
    scale: f64,
    min_top: f64,
) -> impl IntoElement {
    let n = cols.len().max(1);
    let hover = signal(None::<usize>);
    let width = signal(0.0f32);
    let total = |c: &Col| c.vals.iter().sum::<f64>();
    let max = cols
        .iter()
        .filter(|c| !c.open)
        .map(total)
        .fold(0.0, f64::max);
    // Round the axis in the unit the labels use (MB, GB), not in raw bytes.
    let (_, top) = nice_axis(max / scale, 3);
    let top = (top * scale).max(min_top);
    let series_r = series.clone();
    let cols_r = cols.clone();
    let paint = move || {
        let mut p = Painter::new();
        let w = width.get();
        if w <= 0.0 {
            return p;
        }
        let h = height;
        // Grid: top, middle, and the baseline.
        let grid = color(super::chart_grid());
        for k in 0..3 {
            let y = (h - 1.0) * k as f32 / 2.0 + 0.5;
            p = p
                .begin_path()
                .move_to(0.0, y)
                .line_to(w, y)
                .stroke_color(grid)
                .line_width(1.0)
                .stroke();
        }
        let sw = w / n as f32;
        let bw = (sw - 3.0).clamp(1.5, 26.0);
        if let Some(i) = hover.get() {
            p = p
                .fill_color(color(theme().muted_foreground.opacity(0.10)))
                .fill_rect(i as f32 * sw, 0.0, sw, h);
        }
        for (i, c) in cols_r.iter().enumerate() {
            if c.open {
                continue;
            }
            let x = i as f32 * sw + (sw - bw) / 2.0;
            let mut acc = 0.0f64;
            let last = c.vals.iter().rposition(|v| *v > 0.0);
            for (k, v) in c.vals.iter().enumerate() {
                if *v <= 0.0 {
                    continue;
                }
                let y1 = h as f64 - (acc + v) / top * (h - 1.0) as f64;
                let y0 = h as f64 - acc / top * (h - 1.0) as f64;
                let gap = if acc > 0.0 { 2.0 } else { 0.0 };
                let seg_h = ((y0 - y1) - gap).max(1.5) as f32;
                let y = (y0 as f32 - gap as f32 - seg_h).max(0.0);
                p = p.fill_color(color((series_r[k].color)()));
                p = if Some(k) == last {
                    p.begin_path()
                        .round_rect(
                            Bounds::new(point(px(x), px(y)), size_pt(bw, seg_h)),
                            px(3.0),
                        )
                        .fill()
                } else {
                    p.fill_rect(x, y, bw, seg_h)
                };
                acc += v;
            }
        }
        p
    };
    let read = {
        let series = series.clone();
        let cols = cols.clone();
        move || match hover.get() {
            Some(i) if i < cols.len() && !cols[i].open => {
                let c = &cols[i];
                let mut s = c.tip.clone();
                for (k, v) in c.vals.iter().enumerate() {
                    s.push_str(&format!("  ·  {} {}", series[k].name, fmt(*v)));
                }
                if c.vals.len() > 1 {
                    s.push_str(&format!("  ·  Total {}", fmt(c.vals.iter().sum())));
                }
                s
            }
            _ => idle.clone(),
        }
    };
    let plot = canvas(paint)
        .h(px(height))
        .w_full()
        .on_layout(move |f: &Bounds<Points>| width.set(f.size.width.0))
        .on_mouse_move(move |e: &MouseEvent| {
            let w = width.get_untracked().max(1.0);
            let i = ((e.position.x.0 / w) * n as f32).floor() as isize;
            hover.set((i >= 0 && (i as usize) < n).then_some(i as usize));
        })
        .on_mouse_leave(move |_: &MouseEvent| hover.set(None));
    view()
        .gap_1()
        .child(readout(read))
        .child(frame(height, top, fmt, plot, n, labels))
}

fn size_pt(w: f32, h: f32) -> Size<Points> {
    Size {
        width: px(w),
        height: px(h),
    }
}

/// One line of a line chart: `None` is a gap (not measured).
#[derive(Clone)]
pub struct Line {
    pub name: String,
    pub color: fn() -> AdaptiveColor,
    pub vals: Vec<Option<f64>>,
    pub area: bool,
    pub dashed: bool,
}

/// A mark standing at a point of a line chart: a switch, a change.
#[derive(Clone)]
pub struct Mark {
    pub at: usize,
    pub text: String,
}

/// Lines (and areas) over `tips.len()` points. `ymax` fixes the top (else the
/// largest value, rounded up). `tips` name each point when hovered.
#[allow(clippy::too_many_arguments)]
pub fn lines(
    lines: Vec<Line>,
    tips: Vec<String>,
    fmt: fn(f64) -> String,
    labels: Vec<XLabel>,
    marks: Vec<Mark>,
    ymax: Option<f64>,
    height: f32,
    idle: String,
) -> impl IntoElement {
    let n = tips.len().max(1);
    let hover = signal(None::<usize>);
    let width = signal(0.0f32);
    let max = ymax.unwrap_or_else(|| {
        lines
            .iter()
            .flat_map(|l| l.vals.iter().flatten().copied())
            .fold(0.0, f64::max)
    });
    let (_, top) = nice_axis(max, 3);
    let lines_p = lines.clone();
    let marks_p = marks.clone();
    let paint = move || {
        let mut p = Painter::new();
        let w = width.get();
        if w <= 0.0 {
            return p;
        }
        let h = height;
        let grid = color(super::chart_grid());
        for k in 0..3 {
            let y = (h - 1.0) * k as f32 / 2.0 + 0.5;
            p = p
                .begin_path()
                .move_to(0.0, y)
                .line_to(w, y)
                .stroke_color(grid)
                .line_width(1.0)
                .stroke();
        }
        let sw = w / n as f32;
        let x_of = |i: usize| (i as f32 + 0.5) * sw;
        let y_of = |v: f64| h - 1.0 - (v.min(top) / top) as f32 * (h - 2.0);
        for m in &marks_p {
            if m.at < n {
                p = p
                    .begin_path()
                    .move_to(x_of(m.at), 0.0)
                    .line_to(x_of(m.at), h)
                    .stroke_color(color(theme().muted_foreground.opacity(0.5)))
                    .line_width(1.0)
                    .line_dash(&[px(2.0), px(3.0)])
                    .stroke()
                    .line_dash(&[]);
            }
        }
        if let Some(i) = hover.get() {
            p = p
                .begin_path()
                .move_to(x_of(i.min(n - 1)), 0.0)
                .line_to(x_of(i.min(n - 1)), h)
                .stroke_color(color(theme().muted_foreground.opacity(0.7)))
                .line_width(1.0)
                .stroke();
        }
        for l in &lines_p {
            let c = color((l.color)());
            // Runs of measured points.
            let mut run: Vec<(f32, f32)> = Vec::new();
            let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
            for (i, v) in l.vals.iter().enumerate() {
                match v {
                    Some(v) => run.push((x_of(i), y_of(*v))),
                    None => {
                        if !run.is_empty() {
                            runs.push(std::mem::take(&mut run));
                        }
                    }
                }
            }
            if !run.is_empty() {
                runs.push(run);
            }
            for r in &runs {
                if l.area && r.len() > 1 {
                    let mut a = p.begin_path().move_to(r[0].0, h - 1.0);
                    for pt in r {
                        a = a.line_to(pt.0, pt.1);
                    }
                    p = a
                        .line_to(r[r.len() - 1].0, h - 1.0)
                        .close_path()
                        .fill_color(color((l.color)().opacity(0.12)))
                        .fill();
                }
                let mut a = p.begin_path().move_to(r[0].0, r[0].1);
                for pt in &r[1..] {
                    a = a.line_to(pt.0, pt.1);
                }
                a = a.stroke_color(c).line_width(2.0).line_cap(LineCap::Round);
                a = if l.dashed {
                    a.line_dash(&[px(6.0), px(5.0)]).stroke().line_dash(&[])
                } else {
                    a.stroke()
                };
                p = a;
                // The newest point of a solid line gets a dot.
                if !l.dashed {
                    let (x, y) = r[r.len() - 1];
                    p = p
                        .begin_path()
                        .arc(x, y, 3.5, radians(0.0), radians(std::f32::consts::TAU))
                        .fill_color(c)
                        .fill();
                }
            }
        }
        p
    };
    let read = {
        let lines = lines.clone();
        let tips = tips.clone();
        let marks = marks.clone();
        move || match hover.get() {
            Some(i) if i < tips.len() => {
                let mut s = tips[i].clone();
                for l in &lines {
                    if let Some(Some(v)) = l.vals.get(i) {
                        s.push_str(&format!("  ·  {} {}", l.name, fmt(*v)));
                    }
                }
                if let Some(m) = marks.iter().find(|m| m.at == i) {
                    s.push_str(&format!("  ·  {}", m.text));
                }
                s
            }
            _ => idle.clone(),
        }
    };
    let plot = canvas(paint)
        .h(px(height))
        .w_full()
        .on_layout(move |f: &Bounds<Points>| width.set(f.size.width.0))
        .on_mouse_move(move |e: &MouseEvent| {
            let w = width.get_untracked().max(1.0);
            let i = ((e.position.x.0 / w) * n as f32).floor() as isize;
            hover.set((i >= 0 && (i as usize) < n).then_some(i as usize));
        })
        .on_mouse_leave(move |_: &MouseEvent| hover.set(None));
    view()
        .gap_1()
        .child(readout(read))
        .child(frame(height, top, fmt, plot, n, labels))
}

/// A row of ranked bars: a name (with an optional mark in front), a bar as
/// long as its share of the biggest, and the value.
pub struct Row {
    pub name: String,
    pub value: f64,
    pub label: String,
    pub color: fn() -> AdaptiveColor,
    pub lead: Option<AnyElement>,
}

pub fn ranked(rows: Vec<Row>, empty: &str) -> impl IntoElement {
    let max = rows.iter().map(|r| r.value).fold(0.0, f64::max).max(1.0);
    let empty = empty.to_owned();
    if rows.is_empty() {
        return view().child(ui::muted(empty));
    }
    view().gap_2().children(rows.into_iter().map(move |r| {
        let share = (r.value / max).clamp(0.0, 1.0) as f32;
        let c = r.color;
        view()
            .flex_row()
            .items_center()
            .gap_3()
            .child(
                view()
                    .w(px(120.0))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(r.lead)
                    .child(ui::body(r.name).number_of_lines(1).flex_1()),
            )
            .child(
                view()
                    .flex_1()
                    .h(px(10.0))
                    .flex_row()
                    .child(
                        view()
                            .flex_grow(share.max(0.01))
                            .basis(px(0.0))
                            .h_full()
                            .rounded_r(px(4.0))
                            .bg(move || c()),
                    )
                    .child(view().flex_grow((1.0 - share).max(0.0)).basis(px(0.0))),
            )
            .child(ui::muted(r.label).tabular_nums().w(px(104.0)).text_right())
    }))
}

/// How a cell of a ribbon reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    None,
    Good,
    Part,
    Bad,
}

fn tone_color(t: Tone) -> AdaptiveColor {
    match t {
        Tone::None => theme().muted,
        Tone::Good => ui::success(),
        Tone::Part => ui::warning(),
        Tone::Bad => ui::chart_bad(),
    }
}

/// A ribbon of small cells, oldest first, with what each is on hover.
pub fn ribbon(cells: Vec<(Tone, String)>, height: f32, idle: String) -> impl IntoElement {
    let hover = signal(None::<usize>);
    let tips: Vec<String> = cells.iter().map(|c| c.1.clone()).collect();
    let read = move || match hover.get() {
        Some(i) if i < tips.len() => tips[i].clone(),
        _ => idle.clone(),
    };
    view().gap_1().child(readout(read)).child(
        view()
            .flex_row()
            .gap(px(2.0))
            .h(px(height))
            .children(cells.into_iter().enumerate().map(move |(i, (t, _))| {
                view()
                    .flex_1()
                    .min_w(px(2.0))
                    .h_full()
                    .rounded(px(2.5))
                    .bg(move || {
                        if t == Tone::None {
                            tone_color(t)
                        } else {
                            tone_color(t).opacity(0.85)
                        }
                    })
                    .on_mouse_enter(move |_: &MouseEvent| hover.set(Some(i)))
                    .on_mouse_leave(move |_: &MouseEvent| hover.set(None))
            })),
    )
}

/// Rows of ribbons (a week of hours, a row a day) with one readout for all.
pub fn grid(rows: Vec<(String, Vec<(Tone, String)>)>, idle: String) -> impl IntoElement {
    let hover = signal(None::<(usize, usize)>);
    let tips: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.1.iter().map(|c| c.1.clone()).collect())
        .collect();
    let read = move || match hover.get() {
        Some((r, c)) if r < tips.len() && c < tips[r].len() => tips[r][c].clone(),
        _ => idle.clone(),
    };
    view().gap_1().child(readout(read)).child(
        view()
            .gap(px(2.0))
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(move |(r, (label, cells))| {
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                view()
                                    .w(px(34.0))
                                    .child(ui::muted(label).text_size(px(size::small() - 1.0))),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .flex_row()
                                    .gap(px(2.0))
                                    .h(px(15.0))
                                    .children(cells.into_iter().enumerate().map(
                                        move |(c, (t, _))| {
                                            view()
                                                .flex_1()
                                                .min_w(px(2.0))
                                                .h_full()
                                                .rounded(px(3.0))
                                                .bg(move || {
                                                    if t == Tone::None {
                                                        tone_color(t)
                                                    } else {
                                                        tone_color(t).opacity(0.85)
                                                    }
                                                })
                                                .on_mouse_enter(move |_: &MouseEvent| {
                                                    hover.set(Some((r, c)))
                                                })
                                                .on_mouse_leave(move |_: &MouseEvent| {
                                                    hover.set(None)
                                                })
                                        },
                                    )),
                            )
                    }),
            ),
    )
}

/// A ribbon without the readout, for a table cell.
pub fn ribbon_bare(cells: Vec<Tone>, height: f32) -> impl IntoElement {
    view()
        .flex_row()
        .gap(px(2.0))
        .h(px(height))
        .children(cells.into_iter().map(|t| {
            view()
                .flex_1()
                .min_w(px(2.0))
                .h_full()
                .rounded(px(2.5))
                .bg(move || {
                    if t == Tone::None {
                        tone_color(t)
                    } else {
                        tone_color(t).opacity(0.85)
                    }
                })
        }))
}

/// A small line with a dot at its end: how an exit has been doing.
pub fn spark(vals: Vec<Option<f64>>, w: f32, h: f32, c: fn() -> AdaptiveColor) -> impl IntoElement {
    let known: Vec<f64> = vals.iter().flatten().copied().collect();
    canvas(move || {
        let mut p = Painter::new();
        if known.len() < 2 {
            return p;
        }
        let (lo, hi) = known
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        let span = (hi - lo).max(1.0);
        let n = vals.len().max(2);
        let x_of = |i: usize| 3.0 + i as f32 * (w - 8.0) / (n - 1) as f32;
        let y_of = |v: f64| h - 4.0 - ((v - lo) / span) as f32 * (h - 8.0);
        let mut a = p.begin_path();
        let mut started = false;
        let mut last = (0.0, 0.0);
        for (i, v) in vals.iter().enumerate() {
            let Some(v) = v else {
                started = false;
                continue;
            };
            let pt = (x_of(i), y_of(*v));
            a = if started {
                a.line_to(pt.0, pt.1)
            } else {
                a.move_to(pt.0, pt.1)
            };
            started = true;
            last = pt;
        }
        p = a
            .stroke_color(color(c().opacity(0.85)))
            .line_width(1.6)
            .line_cap(LineCap::Round)
            .stroke()
            .begin_path()
            .arc(
                last.0,
                last.1,
                2.6,
                radians(0.0),
                radians(std::f32::consts::TAU),
            )
            .fill_color(color(c()))
            .fill();
        p
    })
    .w(px(w))
    .h(px(h))
}

/// Shown where a chart would be, before there is anything to draw.
pub fn collecting(title: &str, line: &str) -> impl IntoElement {
    let bars: Vec<f32> = (0..18).map(|i| 18.0 + ((i * 37) % 56) as f32).collect();
    view()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_end()
                .gap_1p5()
                .h(px(96.0))
                .children(bars.into_iter().map(|b| {
                    view()
                        .flex_1()
                        .max_w(px(24.0))
                        .h(px(b))
                        .rounded_t(px(4.0))
                        .bg(move || theme().muted_foreground.opacity(0.12))
                })),
        )
        .child(
            view()
                .gap_1()
                .child(ui::body(title.to_owned()).font_weight(FontWeight::SEMIBOLD))
                .child(ui::muted(line.to_owned())),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_round_up_to_nice_tops() {
        assert_eq!(nice_axis(0.0, 3), (1.0, 1.0));
        assert_eq!(nice_axis(f64::NAN, 3), (1.0, 1.0));
        let (step, top) = nice_axis(850.0, 3);
        assert_eq!(step, 500.0);
        assert_eq!(top, 1000.0);
        let (step, top) = nice_axis(2540.0, 3);
        assert_eq!(step, 1000.0);
        assert_eq!(top, 3000.0);
        let (step, top) = nice_axis(100.0, 4);
        assert_eq!((step, top), (25.0, 100.0));
        // The top always holds the maximum.
        for m in [1.0, 7.0, 99.0, 123.4, 5000.0, 0.4] {
            assert!(nice_axis(m, 3).1 >= m, "{m}");
        }
    }
}
