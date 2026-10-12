//! Histogram, density and CDF of a numeric column on a shared x axis.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Paragraph, Widget,
        canvas::{Canvas, Line as CanvasLine},
    },
};

use super::{
    CDF_COLOR, DENSITY_COLOR, GraphMode, GraphSettings, MEAN_COLOR, MEDIAN_COLOR, MUTED,
    SERIES_COLOR, render_subtitle,
};
use crate::file::column_profile::{CLIP_QUANTILES, NumericProfile, ValueFormat, format_float};
use crate::file::stats::{self, Histogram};
use crate::file::utils::{commas, human_readable_count};

/// Right-aligned y labels, then a space and the axis line.
const Y_LABEL_WIDTH: u16 = 7;
const Y_AXIS_WIDTH: u16 = Y_LABEL_WIDTH + 2;
const VERTICAL_EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const MARKER_SYMBOL: &str = "┊";
/// Detail step multiplies bins (or divides bandwidth) by this.
const DETAIL_FACTOR: f64 = 1.5;
const MIN_BINS: usize = 5;
/// A bin this many columns wide or more gets a gap column on its right.
const MIN_BIN_COLUMNS_FOR_GAP: f64 = 3.0;
/// Approximate columns per x tick label.
const TICK_SPACING: u16 = 12;
const TICK_LABEL_GAP: u16 = 2;
const OUTLIER_IQR_FACTOR: f64 = 1.5;
const MAX_POWER_LABEL: i32 = 4;
const MIN_MAX_FOR_MID_LABEL: f64 = 10.0;

/// The x axis: data range `[start, end]` mapped onto `width` columns.
struct XAxis {
    start: f64,
    end: f64,
    width: u16,
}

impl XAxis {
    fn new(start: f64, end: f64, width: u16) -> Self {
        // A constant column still needs a non-empty range to draw.
        let (start, end) = if end > start {
            (start, end)
        } else {
            (start - 0.5, end + 0.5)
        };
        Self { start, end, width }
    }

    fn span(&self) -> f64 {
        self.end - self.start
    }

    fn column(&self, x: f64) -> Option<u16> {
        if !(self.start..=self.end).contains(&x) {
            return None;
        }
        let col = ((x - self.start) / self.span() * self.width as f64) as u16;
        Some(col.min(self.width.saturating_sub(1)))
    }

    fn clamped_column(&self, x: f64) -> u16 {
        self.column(x.clamp(self.start, self.end)).unwrap_or(0)
    }

    fn center(&self, col: u16) -> f64 {
        self.start + (col as f64 + 0.5) * self.span() / self.width as f64
    }
}

/// Values on the plotted scale (log10 when `log_x`), plus how many were
/// dropped because they have no logarithm.
fn plotted_values<'a>(p: &'a NumericProfile, log_x: bool) -> (Cow<'a, [f64]>, usize) {
    if !log_x {
        return (Cow::Borrowed(&p.values), 0);
    }
    let logs: Vec<f64> = p
        .values
        .iter()
        .filter(|v| **v > 0.0)
        .map(|v| v.log10())
        .collect();
    let dropped = p.values.len() - logs.len();
    (Cow::Owned(logs), dropped)
}

fn to_plot_scale(v: f64, log_x: bool) -> Option<f64> {
    match log_x {
        true if v > 0.0 => Some(v.log10()),
        true => None,
        false => Some(v),
    }
}

fn scale_height(v: f64, max: f64, log_y: bool) -> f64 {
    if max <= 0.0 {
        0.0
    } else if log_y {
        v.ln_1p() / max.ln_1p()
    } else {
        v / max
    }
}

/// The value whose bar would reach `fraction` of the plot height.
fn unscale_height(fraction: f64, max: f64, log_y: bool) -> f64 {
    if log_y {
        (fraction * max.ln_1p()).exp_m1()
    } else {
        fraction * max
    }
}

fn format_count(v: f64) -> String {
    let v = v.round() as u64;
    if v < 100_000 {
        commas(v)
    } else {
        human_readable_count(v)
    }
}

fn default_bins(view: &[f64], lo: f64, hi: f64, width: u16) -> usize {
    let n = view.len() as f64;
    let iqr = stats::quantile(view, 0.75) - stats::quantile(view, 0.25);
    // Freedman–Diaconis, falling back to Sturges for degenerate spreads.
    let bins = if iqr > 0.0 && hi > lo {
        ((hi - lo) / (2.0 * iqr * n.powf(-1.0 / 3.0))).ceil() as usize
    } else {
        n.log2().ceil() as usize + 1
    };
    bins.clamp(MIN_BINS, (width as usize / 2).max(MIN_BINS))
}

struct Series {
    /// Height of each plot column as a fraction of the plot height.
    heights: Vec<f64>,
    /// Columns left empty to separate wide bins.
    gaps: Vec<bool>,
    color: Color,
    y_max: f64,
    /// Give every non-empty column at least one eighth, so rare values stay
    /// visible. Right for counts; for a density it would invent mass.
    min_sliver: bool,
}

fn histogram_series(hist: &Histogram, axis: &XAxis, log_y: bool) -> Series {
    let max = hist.counts.iter().copied().max().unwrap_or(0) as f64;
    let bin_of = |col: u16| {
        let idx = ((axis.center(col) - hist.start) / hist.width)
            .floor()
            .max(0.0) as usize;
        idx.min(hist.counts.len() - 1)
    };
    let wide_bins = axis.width as f64 / hist.counts.len() as f64 >= MIN_BIN_COLUMNS_FOR_GAP;
    let heights = (0..axis.width)
        .map(|col| scale_height(hist.counts[bin_of(col)] as f64, max, log_y))
        .collect();
    let gaps = (0..axis.width)
        .map(|col| wide_bins && col + 1 < axis.width && bin_of(col + 1) != bin_of(col))
        .collect();
    Series {
        heights,
        gaps,
        color: SERIES_COLOR,
        y_max: max,
        min_sliver: true,
    }
}

fn density_series(ys: &[f64], log_y: bool) -> Series {
    let max = ys.iter().cloned().fold(0.0, f64::max);
    Series {
        heights: ys.iter().map(|y| scale_height(*y, max, log_y)).collect(),
        gaps: vec![false; ys.len()],
        color: DENSITY_COLOR,
        y_max: max,
        min_sliver: false,
    }
}

fn draw_series(series: &Series, plot: Rect, buf: &mut Buffer) {
    let style = Style::new().fg(series.color);
    let eighths_total = plot.height as usize * 8;
    for (col, (&h, &gap)) in series.heights.iter().zip(&series.gaps).enumerate() {
        if gap || h <= 0.0 {
            continue;
        }
        let mut eighths = (h * eighths_total as f64).round() as usize;
        if series.min_sliver {
            eighths = eighths.max(1);
        }
        let x = plot.x + col as u16;
        for y in (plot.y..plot.bottom()).rev() {
            if eighths == 0 {
                break;
            }
            let step = eighths.min(8);
            buf[(x, y)]
                .set_symbol(VERTICAL_EIGHTHS[step])
                .set_style(style);
            eighths -= step;
        }
    }
}

fn draw_cdf(sorted: &[f64], axis: &XAxis, plot: Rect, buf: &mut Buffer) {
    let n = sorted.len() as f64;
    // Two braille dots per cell horizontally.
    let samples = axis.width as usize * 2;
    let points: Vec<(f64, f64)> = (0..=samples)
        .map(|i| {
            let x = axis.start + axis.span() * i as f64 / samples as f64;
            (x, sorted.partition_point(|v| *v <= x) as f64 / n)
        })
        .collect();
    Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([axis.start, axis.end])
        .y_bounds([0.0, 1.0])
        .paint(|ctx| {
            for pair in points.windows(2) {
                ctx.draw(&CanvasLine::new(
                    pair[0].0, pair[0].1, pair[1].0, pair[1].1, CDF_COLOR,
                ));
            }
        })
        .render(plot, buf);
}

/// A dotted line in empty cells that recolors the bars it crosses, so it
/// stays visible through tall bars.
fn draw_marker(x: f64, color: Color, axis: &XAxis, plot: Rect, buf: &mut Buffer) {
    let Some(col) = axis.column(x) else {
        return;
    };
    for y in plot.y..plot.bottom() {
        let cell = &mut buf[(plot.x + col, y)];
        if cell.symbol() == " " {
            cell.set_symbol(MARKER_SYMBOL);
        }
        cell.set_style(Style::new().fg(color));
    }
}

/// Y axis line with labels at the given height fractions.
fn draw_y_axis(labels: &[(f64, String)], area: Rect, plot: Rect, buf: &mut Buffer) {
    let axis_x = area.x + Y_AXIS_WIDTH - 1;
    let muted = Style::new().fg(MUTED);
    for y in plot.y..plot.bottom() {
        buf[(axis_x, y)].set_symbol("│").set_style(muted);
    }
    for (fraction, label) in labels {
        let y = plot.bottom() - 1 - (fraction * (plot.height - 1) as f64).round() as u16;
        buf[(axis_x, y)].set_symbol("┤").set_style(muted);
        let text = format!("{label:>width$}", width = Y_LABEL_WIDTH as usize);
        buf.set_stringn(area.x, y, text, Y_LABEL_WIDTH as usize, muted);
    }
}

/// A round step near `raw`; integer data avoids steps like 2.5.
fn nice_step(raw: f64, integer: bool) -> f64 {
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let candidates: &[f64] = if integer && magnitude < 10.0 {
        &[1.0, 2.0, 5.0, 10.0]
    } else {
        &[1.0, 2.0, 2.5, 5.0, 10.0]
    };
    let nice = candidates
        .iter()
        .copied()
        .find(|n| *n >= normalized)
        .unwrap_or(10.0);
    nice * magnitude
}

fn power_label(exponent: i32) -> String {
    if exponent.abs() <= MAX_POWER_LABEL {
        format_float(10f64.powi(exponent))
    } else {
        format!("1e{exponent}")
    }
}

/// Tick positions on the plot scale with their labels.
fn ticks(format: &ValueFormat, integer: bool, log_x: bool, axis: &XAxis) -> Vec<(f64, String)> {
    let max_ticks = (axis.width / TICK_SPACING).max(2) as f64;
    if log_x {
        let (first, last) = (axis.start.ceil() as i32, axis.end.floor() as i32);
        if last > first {
            return (first..=last).map(|e| (e as f64, power_label(e))).collect();
        }
    }
    if log_x || matches!(format, ValueFormat::Temporal(_)) {
        let count =
            (axis.width / (format.format(axis.start).len() as u16 + TICK_LABEL_GAP * 2)).max(2);
        let step = axis.span() / count as f64;
        return (0..count)
            .map(|i| {
                let x = axis.start + step * (i as f64 + 0.5);
                let label = match log_x {
                    true => format_float(10f64.powf(x)),
                    false => format.format_tick(x, step),
                };
                (x, label)
            })
            .collect();
    }

    let mut step = nice_step(axis.span() / max_ticks, integer);
    if integer {
        step = step.max(1.0);
    }
    let first = (axis.start / step).ceil() as i64;
    let last = (axis.end / step).floor() as i64;
    (first..=last)
        .map(|i| {
            let x = i as f64 * step;
            (x, format.format_tick(x, step))
        })
        .collect()
}

fn draw_x_axis(ticks: &[(f64, String)], axis: &XAxis, area: Rect, plot: Rect, buf: &mut Buffer) {
    let muted = Style::new().fg(MUTED);
    let axis_y = plot.bottom();
    buf[(plot.x - 1, axis_y)].set_symbol("└").set_style(muted);
    for x in plot.x..plot.right() {
        buf[(x, axis_y)].set_symbol("─").set_style(muted);
    }

    let mut next_free = plot.x;
    for (value, label) in ticks {
        let Some(col) = axis.column(*value) else {
            continue;
        };
        let x = plot.x + col;
        buf[(x, axis_y)].set_symbol("┬").set_style(muted);

        let width = label.chars().count() as u16;
        let start = x
            .saturating_sub(width / 2)
            .max(plot.x)
            .min(area.right().saturating_sub(width));
        if start < next_free {
            continue;
        }
        buf.set_stringn(
            start,
            axis_y + 1,
            label,
            width as usize,
            Style::new().fg(Color::Gray),
        );
        next_free = start + width + TICK_LABEL_GAP;
    }
}

/// Tukey box plot on the axis scale: whiskers to the furthest values within
/// 1.5×IQR, the box over the middle half, outliers as dots.
fn draw_box(xs: &[f64], axis: &XAxis, plot: Rect, y: u16, buf: &mut Buffer) {
    let (q1, median, q3) = (
        stats::quantile(xs, 0.25),
        stats::quantile(xs, 0.5),
        stats::quantile(xs, 0.75),
    );
    let iqr = q3 - q1;
    let (lo_fence, hi_fence) = (q1 - OUTLIER_IQR_FACTOR * iqr, q3 + OUTLIER_IQR_FACTOR * iqr);
    let inside = stats::in_range(xs, lo_fence, hi_fence);
    let (Some(&whisker_lo), Some(&whisker_hi)) = (inside.first(), inside.last()) else {
        return;
    };

    let whisker = Style::new().fg(Color::Gray);
    let col = |v: f64| plot.x + axis.clamped_column(v);
    for x in col(whisker_lo)..=col(whisker_hi) {
        buf[(x, y)].set_symbol("─").set_style(whisker);
    }
    if axis.column(whisker_lo).is_some() {
        buf[(col(whisker_lo), y)].set_symbol("├");
    }
    if axis.column(whisker_hi).is_some() {
        buf[(col(whisker_hi), y)].set_symbol("┤");
    }
    let box_style = Style::new().bg(Color::Blue).fg(Color::White);
    for x in col(q1)..=col(q3) {
        buf[(x, y)].set_symbol(" ").set_style(box_style);
    }
    buf[(col(median), y)]
        .set_symbol("┃")
        .set_style(box_style.fg(MEDIAN_COLOR).add_modifier(Modifier::BOLD));

    let outlier = Style::new().fg(MEAN_COLOR);
    for &v in xs.iter().filter(|v| **v < lo_fence || **v > hi_fence) {
        if let Some(c) = axis.column(v) {
            buf[(plot.x + c, y)].set_symbol("•").set_style(outlier);
        }
    }
}

pub(super) fn render(
    p: &NumericProfile,
    settings: &GraphSettings,
    mode: GraphMode,
    subtitle: Rect,
    area: Rect,
    buf: &mut Buffer,
) {
    let (xs, dropped) = plotted_values(p, settings.log_x);
    if xs.is_empty() {
        let message = if settings.log_x {
            "No positive values: log x can't be drawn (press x)."
        } else {
            "No values to plot."
        };
        Paragraph::new(message.yellow()).render(area, buf);
        return;
    }

    let (lo, hi) = if settings.clip {
        (
            stats::quantile(&xs, CLIP_QUANTILES.0),
            stats::quantile(&xs, CLIP_QUANTILES.1),
        )
    } else {
        (xs[0], xs[xs.len() - 1])
    };
    let view = stats::in_range(&xs, lo, hi);

    let footer_rows = 2 + u16::from(settings.show_box);
    if area.height <= footer_rows + 1 || area.width <= Y_AXIS_WIDTH + 2 {
        return;
    }
    let mut plot = Rect::new(
        area.x + Y_AXIS_WIDTH,
        area.y,
        area.width - Y_AXIS_WIDTH - 1,
        area.height - footer_rows,
    );

    let detail = DETAIL_FACTOR.powi(settings.detail);
    let integer_bins = p.integer && !settings.log_x;
    let mut summary = Vec::new();

    let (axis, series) = match mode {
        GraphMode::Histogram => {
            let bins = ((default_bins(view, lo, hi, plot.width) as f64 * detail).round() as usize)
                .clamp(1, plot.width as usize);
            let hist = stats::histogram(view, lo, hi, bins, integer_bins);
            // Same number of columns for every bin, so bars look even.
            let per_bin = plot.width / hist.counts.len() as u16;
            plot.width = per_bin * hist.counts.len() as u16;
            let axis = XAxis::new(hist.start, hist.end(), plot.width);
            summary.push(format!("{} bins", hist.counts.len()));
            if !settings.log_x {
                summary.push(format!("width {}", p.format.format_spread(hist.width)));
            }
            let series = histogram_series(&hist, &axis, settings.log_y);
            (axis, Some(series))
        }
        GraphMode::Density => {
            let bandwidth = stats::silverman_bandwidth(view) / detail;
            let pad = if settings.clip { 0.0 } else { bandwidth };
            let axis = XAxis::new(lo - pad, hi + pad, plot.width);
            let ys = stats::kde(view, axis.start, axis.end, bandwidth, plot.width as usize);
            summary.push(format!("bandwidth {}", format_float(bandwidth)));
            (axis, Some(density_series(&ys, settings.log_y)))
        }
        GraphMode::Cdf | GraphMode::Bars => {
            summary.push("share of values ≤ x".to_string());
            (XAxis::new(lo, hi, plot.width), None)
        }
    };

    if settings.log_x {
        summary.push("log x".to_string());
    }
    if settings.log_y && series.is_some() {
        summary.push("log y".to_string());
    }
    if settings.clip {
        summary.push(format!(
            "p1–p99, {} outside",
            commas((xs.len() - view.len()) as u64)
        ));
    }
    if dropped > 0 {
        summary.push(format!("{} ≤ 0 hidden", commas(dropped as u64)));
    }

    let median = to_plot_scale(p.quantile(0.5), settings.log_x);
    let mean = to_plot_scale(p.mean, settings.log_x);
    let draw_markers = |buf: &mut Buffer| {
        if settings.show_markers {
            for (value, color) in [(mean, MEAN_COLOR), (median, MEDIAN_COLOR)] {
                if let Some(v) = value {
                    draw_marker(v, color, &axis, plot, buf);
                }
            }
        }
    };

    let y_labels = match &series {
        Some(series) => {
            draw_series(series, plot, buf);
            draw_markers(buf);
            let label = |fraction: f64| {
                let v = unscale_height(fraction, series.y_max, settings.log_y);
                match mode {
                    GraphMode::Density => format_float(v),
                    _ => format_count(v),
                }
            };
            let mut labels = vec![(1.0, label(1.0)), (0.0, "0".to_string())];
            // A mid label on a tiny count axis would be a rounded half-count.
            if mode == GraphMode::Density || series.y_max >= MIN_MAX_FOR_MID_LABEL {
                labels.push((0.5, label(0.5)));
            }
            labels
        }
        None => {
            // Under the curve: the canvas only paints cells it draws in.
            draw_markers(buf);
            draw_cdf(&xs, &axis, plot, buf);
            [1.0, 0.75, 0.5, 0.25, 0.0]
                .into_iter()
                .map(|f| (f, format!("{:.0}%", f * 100.0)))
                .collect()
        }
    };
    draw_y_axis(&y_labels, area, plot, buf);
    let tick_marks = ticks(&p.format, p.integer, settings.log_x, &axis);
    draw_x_axis(&tick_marks, &axis, area, plot, buf);
    if settings.show_box {
        draw_box(&xs, &axis, plot, plot.bottom() + 2, buf);
    }

    let left = Line::from(Span::styled(
        summary.join(" · "),
        Style::new().fg(Color::Gray),
    ));
    let right = if settings.show_markers {
        let mut spans = Vec::new();
        for (name, value, raw, color) in [
            ("median", median, p.quantile(0.5), MEDIAN_COLOR),
            ("mean", mean, p.mean, MEAN_COLOR),
        ] {
            if value.is_some() {
                spans.push(Span::styled(
                    format!("{MARKER_SYMBOL} {name} {}  ", p.format.format(raw)),
                    Style::new().fg(color),
                ));
            }
        }
        Line::from(spans)
    } else {
        Line::default()
    };
    render_subtitle(left, right, subtitle, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_maps_both_ends_inside_the_width() {
        let axis = XAxis::new(0.0, 10.0, 20);
        assert_eq!(axis.column(0.0), Some(0));
        assert_eq!(axis.column(10.0), Some(19));
        assert_eq!(axis.column(10.1), None);
    }

    #[test]
    fn constant_range_is_widened() {
        let axis = XAxis::new(3.0, 3.0, 10);
        assert!(axis.column(3.0).is_some());
    }

    #[test]
    fn nice_steps_are_round() {
        assert_eq!(nice_step(0.7, false), 1.0);
        assert_eq!(nice_step(13.0, false), 20.0);
        assert_eq!(nice_step(2.2, false), 2.5);
        assert_eq!(nice_step(2.2, true), 5.0);
        assert_eq!(nice_step(22.0, true), 25.0);
    }

    #[test]
    fn integer_ticks_never_fall_between_integers() {
        let axis = XAxis::new(0.0, 3.0, 100);
        let t = ticks(&ValueFormat::Integer, true, false, &axis);
        assert!(t.iter().all(|(x, _)| x.fract() == 0.0));
    }

    #[test]
    fn log_ticks_are_powers_of_ten() {
        let axis = XAxis::new(0.0, 3.0, 100);
        let labels: Vec<String> = ticks(&ValueFormat::Float, false, true, &axis)
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert_eq!(labels, vec!["1", "10", "100", "1000"]);
    }

    #[test]
    fn log_y_lifts_small_bars() {
        assert!(scale_height(10.0, 1000.0, true) > scale_height(10.0, 1000.0, false));
        assert!((unscale_height(1.0, 1000.0, true) - 1000.0).abs() < 1e-6);
    }
}
