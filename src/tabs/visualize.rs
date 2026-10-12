use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Stylize;
use ratatui::text::Span;
use std::io;

use crate::components::column_graph::GraphMode;
use crate::{app::AppState, tabs::Tab};

pub struct VisualizeTab {
    pub max_horizontal_scroll: Option<usize>,
    pub max_rows: Option<usize>,
    pub visible_rows: Option<usize>,
}

impl Default for VisualizeTab {
    fn default() -> Self {
        Self::new()
    }
}

impl VisualizeTab {
    pub fn new() -> Self {
        Self {
            max_horizontal_scroll: None,
            max_rows: None,
            visible_rows: None,
        }
    }

    pub fn with_max_horizontal_scroll(mut self, max_horizontal_scroll: usize) -> Self {
        self.max_horizontal_scroll = Some(max_horizontal_scroll);
        self
    }

    pub fn with_max_rows(mut self, max_rows: usize) -> Self {
        self.max_rows = Some(max_rows);
        self
    }

    pub fn with_visible_rows(mut self, visible_rows: usize) -> Self {
        self.visible_rows = Some(visible_rows);
        self
    }
}

impl VisualizeTab {
    fn on_graph_event(key_event: KeyEvent, state: &mut AppState) {
        match key_event.code {
            KeyCode::Char('G') => state.close_graph(),
            KeyCode::Left => state.graph_select_column(false),
            KeyCode::Right => state.graph_select_column(true),
            KeyCode::Up => state.graph_mut().scroll_up(),
            KeyCode::Down => state.graph_mut().scroll_down(),
            KeyCode::Char('m') => state.graph_mut().next_mode(),
            KeyCode::Char(c @ '1'..='4') => {
                let idx = c as usize - '1' as usize;
                state.graph_mut().set_mode(GraphMode::ALL[idx]);
            }
            KeyCode::Char('+') | KeyCode::Char('=') => state.graph_mut().more_detail(),
            KeyCode::Char('-') | KeyCode::Char('_') => state.graph_mut().less_detail(),
            KeyCode::Char('x') => toggle(&mut state.graph_mut().log_x),
            KeyCode::Char('y') => toggle(&mut state.graph_mut().log_y),
            KeyCode::Char('o') => toggle(&mut state.graph_mut().clip),
            KeyCode::Char('b') => toggle(&mut state.graph_mut().show_box),
            KeyCode::Char('a') => toggle(&mut state.graph_mut().show_markers),
            KeyCode::Char('s') => toggle(&mut state.graph_mut().sort_by_value),
            KeyCode::Char('r') => state.graph_mut().reset_view(),
            _ => {}
        }
    }
}

fn toggle(flag: &mut bool) {
    *flag = !*flag;
}

/// Footer entry: highlighted keys, then what they do.
fn key_hint(keys: &'static str, action: &'static str) -> [Span<'static>; 4] {
    [keys.green(), " ".into(), action.into(), " | ".white()]
}

impl Tab for VisualizeTab {
    fn on_event(&self, key_event: KeyEvent, state: &mut AppState) -> Result<(), io::Error> {
        if state.graph_open() {
            Self::on_graph_event(key_event, state);
            return Ok(());
        }

        let max_rows = self.max_rows.unwrap_or(0);
        let visible_rows = state.visible_data_rows();

        match key_event.code {
            // Row navigation (Up/Down arrows)
            KeyCode::Up => {
                if state.vertical_offset() > 0 {
                    state.up();
                    state.adjust_scroll_to_selection(visible_rows, max_rows);
                }
            }
            KeyCode::Down => {
                if state.vertical_offset() < max_rows.saturating_sub(1) {
                    state.down();
                    state.adjust_scroll_to_selection(visible_rows, max_rows);
                }
            }
            // Page navigation (u/d keys)
            KeyCode::Char('u') | KeyCode::Char('U') => {
                state.page_up(visible_rows, max_rows);
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                state.page_down(visible_rows, max_rows);
            }
            // In column mode G graphs the selected column; End still jumps down.
            KeyCode::Char('G') if state.column_mode() => state.open_graph(),
            // Jump to first / last row (vim-style g/G, or Home/End)
            KeyCode::Char('g') | KeyCode::Home => state.jump_to_top(),
            KeyCode::Char('G') | KeyCode::End => {
                state.jump_to_bottom(visible_rows, max_rows);
            }
            KeyCode::Char('M') => state.toggle_column_mode(),
            KeyCode::Char('s') if state.column_mode() => state.cycle_sort(),
            // In column mode the arrows pick the column; scrolling follows it.
            KeyCode::Left if state.column_mode() => state.select_prev_column(),
            KeyCode::Right if state.column_mode() => state.select_next_column(),
            // Column navigation (Left/Right arrows)
            KeyCode::Left if state.horizontal_offset() > 0 => state.left(),
            // Upper bound is enforced in AppState against the on-screen column
            // count each frame, so no guard is needed here.
            KeyCode::Right => state.right(),
            _ => {}
        }
        Ok(())
    }

    fn instructions(&self, state: &AppState) -> Vec<Span<'static>> {
        let hints = if state.graph_open() {
            vec![
                key_hint("←/→", "Column"),
                key_hint("m/1-4", "View"),
                key_hint("+/-", "Detail"),
                key_hint("x/y", "Log"),
                key_hint("o", "Clip"),
                key_hint("b", "Box"),
                key_hint("a", "Marks"),
                key_hint("s", "Order"),
                key_hint("r", "Reset"),
                key_hint("Esc", "Table"),
            ]
        } else if state.column_mode() {
            vec![
                key_hint("↑/↓", "Row"),
                key_hint("←/→", "Select column"),
                key_hint("s", "Sort"),
                key_hint("G", "Graph"),
                key_hint("M", "Exit column mode"),
            ]
        } else {
            vec![
                key_hint("↑/↓", "Row"),
                key_hint("←/→", "Column"),
                key_hint("u/d", "Page"),
                key_hint("g/G", "Top/Bottom"),
                key_hint("M", "Column mode"),
            ]
        };
        let mut spans: Vec<Span<'static>> = hints.into_iter().flatten().collect();
        // Drop the trailing separator; the tab manager adds its own.
        spans.pop();
        spans
    }

    fn to_string(&self) -> String {
        "Visualize".to_string()
    }
}
