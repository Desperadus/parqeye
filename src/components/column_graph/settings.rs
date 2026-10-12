use crate::file::column_profile::{ColumnProfile, ProfileKind};

/// Detail steps either side of the data-driven default (bins / smoothing).
const MAX_DETAIL: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMode {
    Histogram,
    Density,
    Cdf,
    Bars,
}

impl GraphMode {
    pub const ALL: [GraphMode; 4] = [
        GraphMode::Histogram,
        GraphMode::Density,
        GraphMode::Cdf,
        GraphMode::Bars,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GraphMode::Histogram => "Histogram",
            GraphMode::Density => "Density",
            GraphMode::Cdf => "CDF",
            GraphMode::Bars => "Top values",
        }
    }

    fn next(self) -> GraphMode {
        let idx = GraphMode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        GraphMode::ALL[(idx + 1) % GraphMode::ALL.len()]
    }

    /// The view that suits a column best when it is first opened.
    pub fn default_for(profile: &ColumnProfile) -> GraphMode {
        match &profile.kind {
            ProfileKind::Numeric(p) if p.prefers_bars() => GraphMode::Bars,
            ProfileKind::Numeric(p) if p.prefers_density() => GraphMode::Density,
            ProfileKind::Numeric(_) => GraphMode::Histogram,
            _ => GraphMode::Bars,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphSettings {
    /// `None` until the column's profile picks a default.
    pub mode: Option<GraphMode>,
    /// More (+) or fewer (-) bins, less or more smoothing.
    pub detail: i32,
    pub log_x: bool,
    pub log_y: bool,
    /// Limit the x range to the 1st..99th percentile.
    pub clip: bool,
    pub show_box: bool,
    pub show_markers: bool,
    /// Top values ordered by value instead of by count.
    pub sort_by_value: bool,
    pub scroll: usize,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            mode: None,
            detail: 0,
            log_x: false,
            log_y: false,
            clip: false,
            show_box: true,
            show_markers: true,
            sort_by_value: false,
            scroll: 0,
        }
    }
}

impl GraphSettings {
    /// Per-column choices go back to their defaults; display toggles stay.
    pub fn reset_view(&mut self) {
        *self = Self {
            show_box: self.show_box,
            show_markers: self.show_markers,
            ..Self::default()
        };
    }

    pub fn next_mode(&mut self) {
        self.set_mode(self.mode.unwrap_or(GraphMode::Histogram).next());
    }

    pub fn set_mode(&mut self, mode: GraphMode) {
        self.mode = Some(mode);
        self.scroll = 0;
    }

    pub fn more_detail(&mut self) {
        self.detail = (self.detail + 1).min(MAX_DETAIL);
    }

    pub fn less_detail(&mut self) {
        self.detail = (self.detail - 1).max(-MAX_DETAIL);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll += 1;
    }

    /// Settle choices that depend on the data: the default mode, and the
    /// scroll limit for a list showing `bar_rows` entries at once.
    pub fn fit_to(&mut self, profile: &ColumnProfile, bar_rows: usize) {
        let mode = match profile.kind {
            ProfileKind::Numeric(_) => self.mode.unwrap_or(GraphMode::default_for(profile)),
            // Only value counts make sense for text and booleans.
            _ => GraphMode::Bars,
        };
        self.mode = Some(mode);
        self.scroll = self.scroll.min(super::max_bar_scroll(profile, bar_rows));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_cycles_through_all_and_wraps() {
        let mut settings = GraphSettings {
            mode: Some(GraphMode::Histogram),
            ..Default::default()
        };
        for expected in [
            GraphMode::Density,
            GraphMode::Cdf,
            GraphMode::Bars,
            GraphMode::Histogram,
        ] {
            settings.next_mode();
            assert_eq!(settings.mode, Some(expected));
        }
    }

    #[test]
    fn reset_view_keeps_display_toggles() {
        let mut settings = GraphSettings {
            log_x: true,
            show_box: false,
            detail: 3,
            ..Default::default()
        };
        settings.reset_view();
        assert!(!settings.log_x);
        assert_eq!(settings.detail, 0);
        assert!(!settings.show_box);
    }

    #[test]
    fn detail_is_bounded() {
        let mut settings = GraphSettings::default();
        for _ in 0..50 {
            settings.more_detail();
        }
        assert_eq!(settings.detail, MAX_DETAIL);
    }
}
