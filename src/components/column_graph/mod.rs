mod bars;
mod plot;
pub mod settings;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Widget, Wrap},
};

use bars::Missing;
pub use settings::{GraphMode, GraphSettings};

use crate::file::column_profile::{ColumnProfile, ProfileKind};
use crate::file::utils::{commas, truncate_str};

const STATS_WIDTH: u16 = 34;
const MIN_WIDTH_FOR_STATS: u16 = 90;
const BORDER_ROWS: u16 = 2;
/// Subtitle line plus a blank line above the chart.
const SUBTITLE_ROWS: u16 = 2;
const STATS_KEY_WIDTH: usize = 10;
/// Width of the " ▸ " bullet; wrapped insight lines align under its text.
const INSIGHT_INDENT: usize = 3;

const BORDER_COLOR: Color = Color::DarkGray;
const SERIES_COLOR: Color = Color::Cyan;
const DENSITY_COLOR: Color = Color::LightMagenta;
const CDF_COLOR: Color = Color::LightGreen;
const MEDIAN_COLOR: Color = Color::Yellow;
const MEAN_COLOR: Color = Color::LightRed;
const MUTED: Color = Color::DarkGray;

/// Rows a Top values list gets inside a graph area `area_height` tall.
pub fn bar_rows(area_height: u16) -> usize {
    area_height.saturating_sub(BORDER_ROWS + SUBTITLE_ROWS) as usize
}

/// Largest useful scroll offset of the Top values list.
pub fn max_bar_scroll(profile: &ColumnProfile, bar_rows: usize) -> usize {
    match &profile.kind {
        ProfileKind::Numeric(p) => bars::max_scroll(&p.top, missing(profile), bar_rows),
        ProfileKind::Categorical(top) => bars::max_scroll(top, missing(profile), bar_rows),
        ProfileKind::Unsupported(_) => 0,
    }
}

fn missing(profile: &ColumnProfile) -> Missing {
    Missing {
        nulls: profile.nulls,
        non_finite: match &profile.kind {
            ProfileKind::Numeric(p) => p.non_finite,
            _ => 0,
        },
    }
}

/// Distribution view of a single column next to its summary statistics.
pub struct ColumnGraph<'a> {
    profile: &'a ColumnProfile,
    settings: &'a GraphSettings,
}

impl<'a> ColumnGraph<'a> {
    pub fn new(profile: &'a ColumnProfile, settings: &'a GraphSettings) -> Self {
        Self { profile, settings }
    }

    fn mode(&self) -> GraphMode {
        self.settings.mode.unwrap_or(GraphMode::Bars)
    }

    fn title(&self) -> Line<'static> {
        Line::from(vec![
            " ".into(),
            self.profile.name.clone().yellow().bold(),
            Span::styled(format!(" {} ", self.profile.dtype), Style::new().fg(MUTED)),
        ])
    }

    fn mode_strip(&self) -> Line<'static> {
        let numeric = matches!(self.profile.kind, ProfileKind::Numeric(_));
        let mut spans = Vec::new();
        for (idx, mode) in GraphMode::ALL.iter().enumerate() {
            let text = format!(" {} {} ", idx + 1, mode.label());
            let style = if *mode == self.mode() {
                Style::new()
                    .bg(SERIES_COLOR)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else if numeric || *mode == GraphMode::Bars {
                Style::new().fg(Color::Gray)
            } else {
                Style::new().fg(MUTED)
            };
            spans.push(Span::styled(text, style));
        }
        spans.push(" ".into());
        Line::from(spans).right_aligned()
    }

    fn render_stats(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(BORDER_COLOR))
            .title(" Summary ".bold());
        let value_width = (area.width as usize).saturating_sub(STATS_KEY_WIDTH + 3);

        let kv = |key: &str, value: String| {
            Line::from(vec![
                Span::styled(format!(" {key:<STATS_KEY_WIDTH$}"), Style::new().fg(MUTED)),
                Span::raw(truncate_str(&value, value_width.max(2))),
            ])
        };

        let p = self.profile;
        let mut lines = vec![kv("rows", commas(p.rows as u64))];
        let null_share = p.nulls as f64 / p.rows.max(1) as f64 * 100.0;
        lines.push(kv(
            "nulls",
            format!("{} ({null_share:.1}%)", commas(p.nulls as u64)),
        ));

        match &p.kind {
            ProfileKind::Numeric(n) => {
                lines.push(kv("distinct", commas(n.top.distinct as u64)));
                lines.push(Line::default());
                if !n.values.is_empty() {
                    let f = |v: f64| n.format.format(v);
                    lines.push(kv("min", f(n.min())));
                    lines.push(kv("p25", f(n.quantile(0.25))));
                    lines.push(
                        kv("median", f(n.quantile(0.5))).patch_style(Style::new().fg(MEDIAN_COLOR)),
                    );
                    lines.push(kv("mean", f(n.mean)).patch_style(Style::new().fg(MEAN_COLOR)));
                    lines.push(kv("p75", f(n.quantile(0.75))));
                    lines.push(kv("max", f(n.max())));
                    lines.push(kv("std dev", n.format.format_spread(n.std)));
                    lines.push(kv("skew", format!("{:.2}", n.skew)));
                }
            }
            ProfileKind::Categorical(top) => {
                lines.push(kv("distinct", commas(top.distinct as u64)));
                lines.push(Line::default());
                if let Some(first) = top.entries.first() {
                    lines.push(kv("top value", first.label.clone()));
                    // Of all rows, matching the shares next to the bars.
                    let share = first.count as f64 / p.rows.max(1) as f64 * 100.0;
                    lines.push(kv("its share", format!("{share:.1}%")));
                    lines.push(kv("once only", commas(top.singletons as u64)));
                }
            }
            ProfileKind::Unsupported(_) => {}
        }

        if !p.insights.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(" Insights".bold()));
            let text_width = (area.width as usize).saturating_sub(INSIGHT_INDENT + 2);
            for insight in &p.insights {
                for (i, chunk) in wrap_words(insight, text_width).into_iter().enumerate() {
                    let bullet = if i == 0 { " ▸ " } else { "   " };
                    lines.push(Line::from(vec![bullet.fg(SERIES_COLOR), Span::raw(chunk)]));
                }
            }
        }

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

impl Widget for ColumnGraph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (graph_area, stats_area) = if area.width >= MIN_WIDTH_FOR_STATS {
            let [graph, stats] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(STATS_WIDTH)])
                    .areas(area);
            (graph, Some(stats))
        } else {
            (area, None)
        };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(BORDER_COLOR))
            .title(self.title())
            .title(self.mode_strip());
        let inner = block.inner(graph_area);
        block.render(graph_area, buf);

        let [subtitle, _, chart] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(SUBTITLE_ROWS - 1),
            Constraint::Fill(1),
        ])
        .areas(inner);

        let rows = self.profile.rows;
        match &self.profile.kind {
            ProfileKind::Unsupported(message) => {
                Paragraph::new(vec![
                    Line::from("This column can't be plotted.".yellow()),
                    Line::default(),
                    Line::from(message.clone()),
                ])
                .wrap(Wrap { trim: true })
                .render(inner, buf);
            }
            ProfileKind::Numeric(p) if self.mode() != GraphMode::Bars => {
                plot::render(p, self.settings, self.mode(), subtitle, chart, buf);
            }
            ProfileKind::Numeric(p) => {
                let missing = missing(self.profile);
                bars::render(&p.top, rows, missing, self.settings, subtitle, chart, buf);
            }
            ProfileKind::Categorical(top) => {
                let missing = missing(self.profile);
                bars::render(top, rows, missing, self.settings, subtitle, chart, buf);
            }
        }

        if let Some(stats_area) = stats_area {
            self.render_stats(stats_area, buf);
        }
    }
}

/// Greedy word wrap; a word longer than `width` gets a line of its own.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Placeholder shown for the frame in which a column is being read.
pub fn render_loading(column: &str, area: Rect, buf: &mut Buffer) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER_COLOR));
    Paragraph::new(vec![
        Line::default(),
        Line::from(vec![
            "  Reading column ".into(),
            column.to_string().yellow().bold(),
            " …".into(),
        ]),
    ])
    .block(block)
    .render(area, buf);
}

/// One subtitle line: settings on the left, a legend on the right when both fit.
fn render_subtitle(left: Line<'_>, right: Line<'_>, area: Rect, buf: &mut Buffer) {
    let fits = left.width() + right.width() + 2 <= area.width as usize;
    left.render(area, buf);
    if fits {
        right.right_aligned().render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::column_profile::ColumnProfile;

    fn render(column: &str, settings: &GraphSettings, width: u16) -> String {
        let path = format!(
            "{}/alltypes_tiny_pages.parquet",
            crate::file::parquet_test_data()
        );
        let profile = ColumnProfile::load(&path, column, 7300);
        let mut settings = settings.clone();
        settings.fit_to(&profile, bar_rows(30));
        let area = Rect::new(0, 0, width, 30);
        let mut buf = Buffer::empty(area);
        ColumnGraph::new(&profile, &settings).render(area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    #[test]
    fn every_mode_renders_for_a_numeric_column() {
        for mode in GraphMode::ALL {
            for log in [false, true] {
                let settings = GraphSettings {
                    mode: Some(mode),
                    log_x: log,
                    log_y: log,
                    clip: log,
                    ..Default::default()
                };
                let text = render("id", &settings, 120);
                assert!(text.contains(mode.label()), "{mode:?}:\n{text}");
                assert!(text.contains("Summary"));
            }
        }
    }

    #[test]
    fn histogram_draws_bars_and_axis() {
        let settings = GraphSettings {
            mode: Some(GraphMode::Histogram),
            ..Default::default()
        };
        let text = render("id", &settings, 100);
        assert!(text.contains('█'), "{text}");
        assert!(text.contains("bins"));
        assert!(text.contains('└'));
    }

    #[test]
    fn categorical_column_shows_top_values_with_shares() {
        let text = render("string_col", &GraphSettings::default(), 100);
        assert!(text.contains("Top values"));
        assert!(text.contains('%'), "{text}");
    }

    #[test]
    fn wrap_words_fills_lines_greedily() {
        assert_eq!(
            wrap_words("Right-skewed, skew 6.0 (try log x)", 16),
            vec!["Right-skewed,", "skew 6.0 (try", "log x)"]
        );
    }

    #[test]
    fn narrow_terminal_hides_the_summary_panel() {
        let text = render("id", &GraphSettings::default(), 60);
        assert!(!text.contains("Summary"));
    }

    #[test]
    fn date_axis_is_labelled_with_dates() {
        let settings = GraphSettings {
            mode: Some(GraphMode::Histogram),
            ..Default::default()
        };
        let text = render("timestamp_col", &settings, 120);
        assert!(text.contains("2009-"), "{text}");
    }
}
