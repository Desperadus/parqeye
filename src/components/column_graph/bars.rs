//! Top values: one horizontal bar per distinct value.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use super::{GraphSettings, MUTED, SERIES_COLOR, render_subtitle};
use crate::file::column_profile::TopValues;
use crate::file::utils::{commas, truncate_str};

const HORIZONTAL_EIGHTHS: [&str; 9] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
const NULL_LABEL: &str = "∅ null";
const NON_FINITE_LABEL: &str = "NaN / ±inf";
const MAX_LABEL_WIDTH: usize = 32;
const MAX_LABEL_SHARE: f64 = 0.35;
/// "100.0%" is the widest share.
const SHARE_WIDTH: usize = 6;
const MIN_BAR_WIDTH: usize = 4;
const TOP_COLOR: Color = Color::LightCyan;
const NULL_COLOR: Color = Color::Gray;

/// Rows with no value to count under: nulls, and NaN/inf in float columns.
#[derive(Clone, Copy, Default)]
pub(super) struct Missing {
    pub nulls: usize,
    pub non_finite: usize,
}

impl Missing {
    fn rows(self) -> impl Iterator<Item = (&'static str, usize)> {
        [
            (NULL_LABEL, self.nulls),
            (NON_FINITE_LABEL, self.non_finite),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
    }
}

struct Row<'a> {
    label: &'a str,
    count: usize,
    is_missing: bool,
}

fn rows<'a>(top: &'a TopValues, missing: Missing, sort_by_value: bool) -> Vec<Row<'a>> {
    let mut entries: Vec<_> = top.entries.iter().collect();
    if sort_by_value {
        entries.sort_by(|a, b| match (a.numeric, b.numeric) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            _ => a.label.cmp(&b.label),
        });
    }
    let mut rows: Vec<Row> = entries
        .into_iter()
        .map(|e| Row {
            label: &e.label,
            count: e.count,
            is_missing: false,
        })
        .collect();
    for (label, count) in missing.rows() {
        // Ranked by count like any value, or last in value order.
        let at = match sort_by_value {
            true => rows.len(),
            false => rows.partition_point(|r| r.count >= count),
        };
        rows.insert(
            at,
            Row {
                label,
                count,
                is_missing: true,
            },
        );
    }
    rows
}

/// Rows of values on screen: all of them, or one fewer to leave room for the
/// footer line when the list doesn't fit or was truncated.
fn visible_rows(top: &TopValues, total_rows: usize, bar_rows: usize) -> usize {
    let truncated = top.distinct > top.entries.len();
    if total_rows <= bar_rows && !truncated {
        bar_rows
    } else {
        bar_rows.saturating_sub(1)
    }
}

pub(super) fn max_scroll(top: &TopValues, missing: Missing, bar_rows: usize) -> usize {
    let total = top.entries.len() + missing.rows().count();
    total.saturating_sub(visible_rows(top, total, bar_rows))
}

fn bar(fraction: f64, width: usize) -> String {
    let eighths = ((fraction * width as f64 * 8.0).round() as usize).max(1);
    let mut s = "█".repeat(eighths / 8);
    s.push_str(HORIZONTAL_EIGHTHS[eighths % 8]);
    s
}

fn scale(count: usize, max: usize, log: bool) -> f64 {
    match log {
        true => (count as f64).ln_1p() / (max as f64).ln_1p(),
        false => count as f64 / max.max(1) as f64,
    }
}

pub(super) fn render(
    top: &TopValues,
    total: usize,
    missing: Missing,
    settings: &GraphSettings,
    subtitle: Rect,
    area: Rect,
    buf: &mut Buffer,
) {
    let all = rows(top, missing, settings.sort_by_value);
    let shown = visible_rows(top, all.len(), area.height as usize);
    let scroll = settings
        .scroll
        .min(max_scroll(top, missing, area.height as usize));
    let page = &all[scroll.min(all.len())..(scroll + shown).min(all.len())];

    let share = |count: usize| count as f64 / total.max(1) as f64 * 100.0;
    let max_count = all.iter().map(|r| r.count).max().unwrap_or(0);
    let count_width = commas(max_count as u64).len();
    let cumulative = !settings.sort_by_value;
    let label_width = all
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0)
        .min(MAX_LABEL_WIDTH)
        .min((area.width as f64 * MAX_LABEL_SHARE) as usize)
        .max(NON_FINITE_LABEL.chars().count());
    let numbers_width =
        count_width + 2 + SHARE_WIDTH + if cumulative { 2 + SHARE_WIDTH } else { 0 };
    let bar_width = (area.width as usize)
        .saturating_sub(label_width + 2 + numbers_width + 2)
        .max(MIN_BAR_WIDTH);

    let mut running: usize = all[..scroll.min(all.len())].iter().map(|r| r.count).sum();
    for (i, (row, y)) in page.iter().zip(area.y..area.bottom()).enumerate() {
        running += row.count;
        let (label_style, bar_color) = if row.is_missing {
            (
                Style::new().fg(NULL_COLOR).add_modifier(Modifier::ITALIC),
                NULL_COLOR,
            )
        } else if scroll + i == 0 && !settings.sort_by_value {
            (
                Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
                TOP_COLOR,
            )
        } else {
            (Style::new().fg(Color::White), SERIES_COLOR)
        };

        let label = truncate_str(row.label, label_width);
        let bar_text = bar(scale(row.count, max_count, settings.log_y), bar_width);
        let mut spans = vec![
            Span::styled(format!("{label:>label_width$}  "), label_style),
            Span::styled(
                format!("{bar_text:<bar_width$}  "),
                Style::new().fg(bar_color),
            ),
            Span::raw(format!("{:>count_width$}", commas(row.count as u64))),
            Span::styled(
                format!("  {:>5.1}%", share(row.count)),
                Style::new().fg(Color::Gray),
            ),
        ];
        if cumulative {
            spans.push(Span::styled(
                format!("  {:>5.1}%", share(running)),
                Style::new().fg(MUTED),
            ));
        }
        Line::from(spans).render(Rect::new(area.x, y, area.width, 1), buf);
    }

    let unlisted = top.distinct - top.entries.len();
    if shown < area.height as usize && (all.len() > shown || unlisted > 0) {
        let mut text = format!(
            "{}–{} of {} listed",
            scroll + 1,
            scroll + page.len(),
            all.len()
        );
        if scroll + page.len() < all.len() {
            text.push_str(" · ↓ for more");
        }
        if unlisted > 0 {
            let listed: usize = top.entries.iter().map(|e| e.count).sum();
            text.push_str(&format!(
                " · {} rarer values not listed ({:.1}% of rows)",
                commas(unlisted as u64),
                share(top.total - listed)
            ));
        }
        let y = area.y + page.len() as u16;
        Line::from(Span::styled(text, Style::new().fg(MUTED)))
            .render(Rect::new(area.x, y, area.width, 1), buf);
    }

    let order = if settings.sort_by_value {
        "by value"
    } else {
        "by count"
    };
    let mut left = format!("{} distinct · sorted {order}", commas(top.distinct as u64));
    if settings.log_y {
        left.push_str(" · log scale");
    }
    let legend = if cumulative {
        "count · share · cumulative "
    } else {
        "count · share "
    };
    render_subtitle(
        Line::from(Span::styled(left, Style::new().fg(Color::Gray))),
        Line::from(Span::styled(legend, Style::new().fg(MUTED))),
        subtitle,
        buf,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::column_profile::ValueCount;

    fn top(counts: &[(&str, usize)]) -> TopValues {
        TopValues {
            entries: counts
                .iter()
                .map(|(label, count)| ValueCount {
                    label: label.to_string(),
                    count: *count,
                    numeric: None,
                })
                .collect(),
            distinct: counts.len(),
            singletons: 0,
            total: counts.iter().map(|(_, c)| c).sum(),
        }
    }

    #[test]
    fn nulls_are_ranked_by_their_count() {
        let t = top(&[("a", 10), ("b", 2)]);
        let missing = Missing {
            nulls: 5,
            non_finite: 1,
        };
        let labels: Vec<&str> = rows(&t, missing, false).iter().map(|r| r.label).collect();
        assert_eq!(labels, vec!["a", NULL_LABEL, "b", NON_FINITE_LABEL]);
    }

    #[test]
    fn sorting_by_value_puts_nulls_last() {
        let t = top(&[("b", 10), ("a", 2)]);
        let missing = Missing {
            nulls: 5,
            non_finite: 0,
        };
        let labels: Vec<&str> = rows(&t, missing, true).iter().map(|r| r.label).collect();
        assert_eq!(labels, vec!["a", "b", NULL_LABEL]);
    }

    #[test]
    fn short_list_needs_no_scrolling() {
        let t = top(&[("a", 1), ("b", 1)]);
        assert_eq!(max_scroll(&t, Missing::default(), 10), 0);
    }

    #[test]
    fn long_list_scrolls_until_the_last_row_is_shown() {
        let labels: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        let counts: Vec<(&str, usize)> = labels.iter().map(|l| (l.as_str(), 1)).collect();
        // 10 rows: 9 values plus the footer line.
        assert_eq!(max_scroll(&top(&counts), Missing::default(), 10), 11);
    }

    #[test]
    fn bar_length_follows_the_fraction() {
        assert_eq!(bar(1.0, 4).chars().count(), 4);
        assert_eq!(bar(0.5, 4), "██");
        // Tiny but non-zero counts still get a sliver.
        assert_eq!(bar(0.0001, 4), "▏");
    }
}
