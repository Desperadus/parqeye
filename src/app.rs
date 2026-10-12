use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use std::io;

use crate::components::column_graph::{self, GraphSettings};
use crate::file::parquet_ctx::ParquetCtx;
use crate::file::sample_data::{SortOrder, SortSpec};
use crate::tabs::TabManager;

pub struct AppRenderView<'a> {
    pub title: &'a str,
    pub parquet_ctx: &'a ParquetCtx,
    file_name: &'a str,
    tabs: &'a TabManager,
    pub state: &'a AppState,
}

impl<'a> AppRenderView<'a> {
    fn from_app(app: &'a App) -> Self {
        Self {
            title: "parqeye",
            parquet_ctx: app.parquet_ctx,
            file_name: &app.file_name,
            tabs: &app.tabs,
            state: &app.state,
        }
    }

    pub fn tabs(&self) -> &TabManager {
        self.tabs
    }

    pub fn file_name(&self) -> &str {
        self.file_name
    }

    pub fn state(&self) -> &AppState {
        self.state
    }
}

pub struct App<'a> {
    pub parquet_ctx: &'a ParquetCtx,
    pub file_name: String,
    pub exit: bool,
    pub tabs: TabManager,
    pub state: AppState,
}

pub struct AppState {
    horizontal_offset: usize,
    vertical_offset: usize,
    tree_scroll_offset: usize,
    data_vertical_scroll: usize,
    visible_data_rows: usize,
    // Upper bound for `horizontal_offset`, recomputed each frame from the
    // on-screen column count. Prevents scrolling past the last visible column.
    max_horizontal_offset: usize,
    // Visualize tab: in column mode Left/Right move `selected_column` instead
    // of scrolling.
    column_mode: bool,
    selected_column: usize,
    num_columns: usize,
    sort: Option<SortSpec>,
    // Distribution graph of `selected_column`, opened from column mode.
    graph_open: bool,
    graph: GraphSettings,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            horizontal_offset: 0,
            vertical_offset: 0,
            tree_scroll_offset: 0,
            data_vertical_scroll: 0,
            visible_data_rows: 20, // Default fallback
            max_horizontal_offset: usize::MAX,
            column_mode: false,
            selected_column: 0,
            num_columns: 0,
            sort: None,
            graph_open: false,
            graph: GraphSettings::default(),
        }
    }

    /// Resets navigation; an active sort is kept.
    pub fn reset(&mut self) {
        self.horizontal_offset = 0;
        self.vertical_offset = 0;
        self.tree_scroll_offset = 0;
        self.data_vertical_scroll = 0;
        self.column_mode = false;
        self.graph_open = false;
    }

    pub fn horizontal_offset(&self) -> usize {
        self.horizontal_offset
    }

    pub fn vertical_offset(&self) -> usize {
        self.vertical_offset
    }

    pub fn down(&mut self) {
        self.vertical_offset += 1;
    }

    pub fn up(&mut self) {
        self.vertical_offset = self.vertical_offset.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.horizontal_offset = (self.horizontal_offset + 1).min(self.max_horizontal_offset);
    }

    pub fn left(&mut self) {
        self.horizontal_offset = self.horizontal_offset.saturating_sub(1);
    }

    /// Set the upper bound for horizontal scrolling and clamp the current offset
    /// to it (handles overshoot from a previous frame and terminal resizes).
    pub fn set_max_horizontal_offset(&mut self, max: usize) {
        self.max_horizontal_offset = max;
        self.horizontal_offset = self.horizontal_offset.min(max);
    }

    pub fn set_horizontal_offset(&mut self, offset: usize) {
        self.horizontal_offset = offset.min(self.max_horizontal_offset);
    }

    pub fn column_mode(&self) -> bool {
        self.column_mode
    }

    /// Entering column mode selects the first column on screen.
    pub fn toggle_column_mode(&mut self) {
        self.column_mode = !self.column_mode;
        if self.column_mode {
            self.selected_column = self.horizontal_offset;
        }
    }

    pub fn selected_column(&self) -> usize {
        self.selected_column
    }

    pub fn set_num_columns(&mut self, num_columns: usize) {
        self.num_columns = num_columns;
        self.selected_column = self.selected_column.min(num_columns.saturating_sub(1));
    }

    pub fn select_next_column(&mut self) {
        self.selected_column = (self.selected_column + 1).min(self.num_columns.saturating_sub(1));
    }

    pub fn select_prev_column(&mut self) {
        self.selected_column = self.selected_column.saturating_sub(1);
    }

    pub fn graph_open(&self) -> bool {
        self.graph_open
    }

    pub fn graph(&self) -> &GraphSettings {
        &self.graph
    }

    pub fn graph_mut(&mut self) -> &mut GraphSettings {
        &mut self.graph
    }

    pub fn open_graph(&mut self) {
        self.graph_open = true;
        self.graph.reset_view();
    }

    /// Back to the table, still in column mode.
    pub fn close_graph(&mut self) {
        self.graph_open = false;
    }

    /// Move the graph to a neighbouring column; its view starts fresh.
    pub fn graph_select_column(&mut self, forward: bool) {
        let before = self.selected_column;
        if forward {
            self.select_next_column();
        } else {
            self.select_prev_column();
        }
        if self.selected_column != before {
            self.graph.reset_view();
        }
    }

    pub fn sort(&self) -> Option<SortSpec> {
        self.sort
    }

    pub fn clear_sort(&mut self) {
        self.sort = None;
    }

    /// Cycle the selected column through descending -> ascending -> file order.
    /// Choosing another column starts it at descending.
    pub fn cycle_sort(&mut self) {
        let column = self.selected_column;
        self.sort = match self.sort {
            Some(SortSpec {
                column: sorted,
                order: SortOrder::Descending,
            }) if sorted == column => Some(SortSpec {
                column,
                order: SortOrder::Ascending,
            }),
            Some(SortSpec {
                column: sorted,
                order: SortOrder::Ascending,
            }) if sorted == column => None,
            _ => Some(SortSpec {
                column,
                order: SortOrder::Descending,
            }),
        };
        self.jump_to_top();
    }

    pub fn tree_scroll_offset(&self) -> usize {
        self.tree_scroll_offset
    }

    pub fn tree_scroll_up(&mut self) {
        self.tree_scroll_offset = self.tree_scroll_offset.saturating_sub(1);
    }

    pub fn tree_scroll_down(&mut self) {
        self.tree_scroll_offset += 1;
    }

    pub fn data_vertical_scroll(&self) -> usize {
        self.data_vertical_scroll
    }

    pub fn set_data_vertical_scroll(&mut self, scroll: usize) {
        self.data_vertical_scroll = scroll;
    }

    pub fn visible_data_rows(&self) -> usize {
        self.visible_data_rows
    }

    pub fn set_visible_data_rows(&mut self, rows: usize) {
        self.visible_data_rows = rows;
    }

    pub fn page_up(&mut self, visible_rows: usize, max_rows: usize) {
        // Move selection up by visible_rows
        self.vertical_offset = self.vertical_offset.saturating_sub(visible_rows);
        // Adjust scroll to keep selection visible
        self.adjust_scroll_to_selection(visible_rows, max_rows);
    }

    pub fn page_down(&mut self, visible_rows: usize, max_rows: usize) {
        // Move selection down by visible_rows, clamped to max_rows - 1
        self.vertical_offset =
            (self.vertical_offset + visible_rows).min(max_rows.saturating_sub(1));
        // Adjust scroll to keep selection visible
        self.adjust_scroll_to_selection(visible_rows, max_rows);
    }

    /// Jump the selection (and viewport) to the first row.
    pub fn jump_to_top(&mut self) {
        self.vertical_offset = 0;
        self.data_vertical_scroll = 0;
    }

    /// Jump the selection (and viewport) to the last row.
    pub fn jump_to_bottom(&mut self, visible_rows: usize, max_rows: usize) {
        self.vertical_offset = max_rows.saturating_sub(1);
        self.adjust_scroll_to_selection(visible_rows, max_rows);
    }

    pub fn adjust_scroll_to_selection(&mut self, visible_rows: usize, max_rows: usize) {
        // Ensure selected row is visible in viewport
        if self.vertical_offset < self.data_vertical_scroll {
            // Selection is above viewport, scroll up
            self.data_vertical_scroll = self.vertical_offset;
        } else if self.vertical_offset >= self.data_vertical_scroll + visible_rows {
            // Selection is below viewport, scroll down
            self.data_vertical_scroll = self.vertical_offset.saturating_sub(visible_rows - 1);
        }

        // Clamp scroll to valid range
        let max_scroll = max_rows.saturating_sub(visible_rows);
        self.data_vertical_scroll = self.data_vertical_scroll.min(max_scroll);
    }
}

impl<'a> App<'a> {
    pub fn new(file_info: &'a ParquetCtx) -> Self {
        let sample_data_rows = file_info.sample_data.total_rows;

        let tab_manager = TabManager::new(
            file_info.schema.column_size(),
            file_info.row_groups.num_row_groups(),
            sample_data_rows,
            file_info.metadata.total_property_display_lines(),
        );

        Self {
            parquet_ctx: file_info,
            file_name: file_info.file_path.clone(),
            exit: false,
            tabs: tab_manager,
            state: AppState::new(),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.exit {
            // Calculate visible data rows based on terminal size
            let terminal_size = terminal.size()?;
            // Account for: header (3 lines), footer (1 line), table header (3 lines) = 7 lines total
            let visible_data_rows = (terminal_size.height.saturating_sub(7) as usize).max(1);
            self.state.set_visible_data_rows(visible_data_rows);

            let is_visualize = self.tabs.active_tab().to_string() == "Visualize";
            let sample_data = &self.parquet_ctx.sample_data;

            if is_visualize {
                self.state.set_num_columns(sample_data.total_columns);
                // Some column types (e.g. structs) cannot be sorted; drop the
                // request so the table keeps its current order.
                if sample_data.set_sort(self.state.sort()).is_err() {
                    self.state.clear_sort();
                }
                // Lazily (re)load the row window backing the Visualize table so only a
                // bounded slice of a large file is ever held in memory.
                sample_data.ensure_loaded(self.state.data_vertical_scroll(), visible_data_rows);
            }

            // No columns when polars couldn't read the file; the error screen shows instead.
            if is_visualize && self.state.graph_open() && sample_data.total_columns > 0 {
                let column = self.state.selected_column();
                if sample_data.cached_profile(column).is_none() {
                    // Reading a whole column can take a moment; show that first.
                    let render_view = AppRenderView::from_app(self);
                    terminal.draw(|frame| crate::ui::render_app(&render_view, frame))?;
                }
                let profile = sample_data.profile(column);
                let bar_rows = column_graph::bar_rows(terminal_size.height.saturating_sub(4));
                self.state.graph_mut().fit_to(&profile, bar_rows);
            }

            // Bound visualize horizontal scrolling to what fits.
            // terminal minus the tab header (3 lines) and footer (1 line).
            let table_height = terminal_size.height.saturating_sub(4);
            let table = crate::components::DataTable::new(sample_data)
                .with_vertical_scroll(self.state.data_vertical_scroll());
            let max_horizontal_offset = if is_visualize {
                table.max_horizontal_scroll(terminal_size.width, table_height)
            } else {
                usize::MAX
            };
            self.state.set_max_horizontal_offset(max_horizontal_offset);

            if is_visualize && self.state.column_mode() {
                let offset = table.horizontal_scroll_showing(
                    self.state.selected_column(),
                    self.state.horizontal_offset(),
                    terminal_size.width,
                    table_height,
                );
                self.state.set_horizontal_offset(offset);
            }

            let render_view = AppRenderView::from_app(self);
            terminal.draw(|frame| crate::ui::render_app(&render_view, frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn handle_events(&mut self) -> io::Result<()> {
        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
            KeyCode::Esc if self.state.graph_open() => self.state.close_graph(),
            KeyCode::Esc => self.state.reset(),
            KeyCode::Tab => {
                self.tabs.next();
                self.state.reset();
            }
            KeyCode::BackTab => {
                self.tabs.prev();
                self.state.reset();
            }
            _ => {
                self.tabs
                    .active_tab()
                    .on_event(key_event, &mut self.state)
                    .unwrap();
            }
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_right_is_clamped_to_max_horizontal_offset() {
        let mut state = AppState::new();
        state.set_max_horizontal_offset(3);
        for _ in 0..10 {
            state.right();
        }
        // Without the clamp the offset would run past the visible range,
        // producing the "empty clicks" seen when scrolling back.
        assert_eq!(state.horizontal_offset(), 3);
    }

    #[test]
    fn test_left_saturates_at_zero() {
        let mut state = AppState::new();
        state.set_max_horizontal_offset(5);
        state.right();
        state.right();
        state.left();
        state.left();
        state.left();
        assert_eq!(state.horizontal_offset(), 0);
    }

    #[test]
    fn test_jump_to_bottom_then_top() {
        let mut state = AppState::new();

        state.jump_to_bottom(20, 7300);
        assert_eq!(state.vertical_offset(), 7299);
        assert_eq!(state.data_vertical_scroll(), 7300 - 20);

        state.jump_to_top();
        assert_eq!(state.vertical_offset(), 0);
        assert_eq!(state.data_vertical_scroll(), 0);
    }

    #[test]
    fn test_cycle_sort_goes_descending_ascending_then_off() {
        let mut state = AppState::new();
        state.set_num_columns(5);
        state.toggle_column_mode();
        state.select_next_column();

        state.cycle_sort();
        assert_eq!(
            state.sort(),
            Some(SortSpec {
                column: 1,
                order: SortOrder::Descending
            })
        );
        state.cycle_sort();
        assert_eq!(
            state.sort(),
            Some(SortSpec {
                column: 1,
                order: SortOrder::Ascending
            })
        );
        state.cycle_sort();
        assert_eq!(state.sort(), None);
    }

    #[test]
    fn test_sorting_another_column_starts_descending() {
        let mut state = AppState::new();
        state.set_num_columns(5);
        state.toggle_column_mode();
        state.cycle_sort();
        state.cycle_sort();
        state.select_next_column();
        state.cycle_sort();
        assert_eq!(
            state.sort(),
            Some(SortSpec {
                column: 1,
                order: SortOrder::Descending
            })
        );
    }

    #[test]
    fn test_moving_the_graph_to_another_column_resets_its_view() {
        let mut state = AppState::new();
        state.set_num_columns(3);
        state.toggle_column_mode();
        state.open_graph();
        state.graph_mut().log_x = true;
        state.graph_select_column(true);
        assert_eq!(state.selected_column(), 1);
        assert!(!state.graph().log_x);
    }

    #[test]
    fn test_graph_view_is_kept_at_the_last_column() {
        let mut state = AppState::new();
        state.set_num_columns(1);
        state.toggle_column_mode();
        state.open_graph();
        state.graph_mut().log_x = true;
        state.graph_select_column(true);
        assert!(state.graph().log_x);
    }

    #[test]
    fn test_selected_column_is_clamped_to_the_column_count() {
        let mut state = AppState::new();
        state.set_num_columns(2);
        for _ in 0..5 {
            state.select_next_column();
        }
        assert_eq!(state.selected_column(), 1);
    }

    #[test]
    fn test_shrinking_the_max_clamps_the_current_offset() {
        let mut state = AppState::new();
        state.set_max_horizontal_offset(10);
        for _ in 0..10 {
            state.right();
        }
        assert_eq!(state.horizontal_offset(), 10);
        // e.g. the terminal was widened, so fewer columns need scrolling.
        state.set_max_horizontal_offset(4);
        assert_eq!(state.horizontal_offset(), 4);
    }
}
