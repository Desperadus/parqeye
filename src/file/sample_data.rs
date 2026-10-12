use std::cell::{Ref, RefCell};
use std::sync::Mutex;

use polars::prelude::*;

type LoadedRows = (Vec<Vec<String>>, Vec<String>);

const WINDOW_ROWS: usize = 2000;
const ROW_PREFETCH: usize = 500;
/// Temporary column holding each row's position in the file; named to avoid
/// clashing with real columns.
const ROW_ID_COLUMN: &str = "__parqeye_row_id";

/// Serializes temporary replacement of the process-wide panic hook.
static POLARS_PANIC_HOOK_LOCK: Mutex<()> = Mutex::new(());

/// The currently materialized slice of rows: `rows[i]` is the file row at
/// absolute index `start + i`.
#[derive(Debug, Default)]
pub struct RowWindow {
    pub start: usize,
    pub rows: Vec<Vec<String>>,
}

impl RowWindow {
    fn covers(&self, view_start: usize, view_len: usize) -> bool {
        view_start >= self.start && view_start + view_len <= self.start + self.rows.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Descending,
    Ascending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub column: usize,
    pub order: SortOrder,
}

/// An active sort: `row_ids[i]` is the file row shown at display position `i`.
#[derive(Debug)]
struct Sorting {
    spec: SortSpec,
    row_ids: Vec<IdxSize>,
}

/// A lazily-loaded, memory-bounded view over a parquet file's rows.
#[derive(Debug, Default)]
pub struct ParquetSampleData {
    file_path: String,
    pub flattened_columns: Vec<String>,
    pub total_columns: usize,
    pub total_rows: usize,
    window: RefCell<RowWindow>,
    sorting: RefCell<Option<Sorting>>,
}

impl ParquetSampleData {
    /// Open the file and load the first window. Polars panics for some unsupported
    /// parquet formats, so those panics are returned as ordinary errors.
    pub fn open(
        file_path: &str,
        total_rows: usize,
    ) -> Result<ParquetSampleData, Box<dyn std::error::Error>> {
        let path = file_path.to_string();
        catch_polars_panic(move || Self::open_with_polars(&path, total_rows))
    }

    fn open_with_polars(
        file_path: &str,
        total_rows: usize,
    ) -> Result<ParquetSampleData, Box<dyn std::error::Error>> {
        let first_len = WINDOW_ROWS.min(total_rows);
        let (rows, columns) = if first_len > 0 {
            Self::load_with_polars(file_path, 0, first_len)?
        } else {
            let mut lf = LazyFrame::scan_parquet(PlRefPath::new(file_path), Default::default())?;
            let schema = lf.collect_schema()?;
            let names = schema.iter_names().map(|s| s.to_string()).collect();
            (Vec::new(), names)
        };

        Ok(ParquetSampleData {
            file_path: file_path.to_string(),
            total_columns: columns.len(),
            flattened_columns: columns,
            total_rows,
            window: RefCell::new(RowWindow { start: 0, rows }),
            sorting: RefCell::new(None),
        })
    }

    /// Read `len` rows at `offset`; the slice is pushed into the parquet scan.
    fn load(
        file_path: &str,
        offset: usize,
        len: usize,
    ) -> Result<LoadedRows, Box<dyn std::error::Error>> {
        let path = file_path.to_string();
        catch_polars_panic(move || Self::load_with_polars(&path, offset, len))
    }

    fn load_with_polars(
        file_path: &str,
        offset: usize,
        len: usize,
    ) -> Result<LoadedRows, Box<dyn std::error::Error>> {
        let df = LazyFrame::scan_parquet(PlRefPath::new(file_path), Default::default())?
            .slice(offset as i64, len as IdxSize)
            .collect()?;
        Ok(Self::to_rows(Self::flatten_struct_columns(df)))
    }

    /// Read the rows at the given file positions, returned in the order of `row_ids`.
    fn load_by_row_ids(
        file_path: &str,
        row_ids: &[IdxSize],
    ) -> Result<LoadedRows, Box<dyn std::error::Error>> {
        let path = file_path.to_string();
        let row_ids = row_ids.to_vec();
        catch_polars_panic(move || Self::load_by_row_ids_with_polars(&path, &row_ids))
    }

    fn load_by_row_ids_with_polars(
        file_path: &str,
        row_ids: &[IdxSize],
    ) -> Result<LoadedRows, Box<dyn std::error::Error>> {
        let mut ids_in_file_order = row_ids.to_vec();
        ids_in_file_order.sort_unstable();
        let ids = Series::new(ROW_ID_COLUMN.into(), &ids_in_file_order);

        // A filter rather than a join so the scan stays streaming instead of
        // materializing the whole file.
        let df = LazyFrame::scan_parquet(PlRefPath::new(file_path), Default::default())?
            .with_row_index(ROW_ID_COLUMN, None)
            .filter(col(ROW_ID_COLUMN).is_in(lit(ids).implode(false), false))
            .collect()?
            .drop(ROW_ID_COLUMN)?;

        let positions: Vec<IdxSize> = row_ids
            .iter()
            .map(|id| ids_in_file_order.binary_search(id).unwrap() as IdxSize)
            .collect();
        let df = df.take(&IdxCa::from_vec(PlSmallStr::EMPTY, positions))?;
        Ok(Self::to_rows(Self::flatten_struct_columns(df)))
    }

    /// File row positions ordered by `column`; only that column is read.
    fn sorted_row_ids(
        file_path: &str,
        column: &str,
        order: SortOrder,
    ) -> Result<Vec<IdxSize>, Box<dyn std::error::Error>> {
        let path = file_path.to_string();
        let column = column.to_string();
        catch_polars_panic(move || Self::sorted_row_ids_with_polars(&path, &column, order))
    }

    fn sorted_row_ids_with_polars(
        file_path: &str,
        column: &str,
        order: SortOrder,
    ) -> Result<Vec<IdxSize>, Box<dyn std::error::Error>> {
        let options = SortMultipleOptions::default()
            .with_order_descending(order == SortOrder::Descending)
            .with_nulls_last(true)
            .with_maintain_order(true);
        let df = LazyFrame::scan_parquet(PlRefPath::new(file_path), Default::default())?
            .with_row_index(ROW_ID_COLUMN, None)
            .select([col(ROW_ID_COLUMN), col(column)])
            .sort([column], options)
            .collect()?;
        Ok(df
            .column(ROW_ID_COLUMN)?
            .idx()?
            .into_no_null_iter()
            .collect())
    }

    fn to_rows(df: DataFrame) -> LoadedRows {
        let columns: Vec<String> = df
            .get_column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut rows = Vec::with_capacity(df.height());
        for row_idx in 0..df.height() {
            let mut row = Vec::with_capacity(columns.len());
            for col in df.columns() {
                let series = col.as_materialized_series();
                row.push(Self::get_value_as_string(series, row_idx));
            }
            rows.push(row);
        }

        (rows, columns)
    }

    pub fn sort(&self) -> Option<SortSpec> {
        self.sorting.borrow().as_ref().map(|sorting| sorting.spec)
    }

    /// Apply `spec` (or restore file order with `None`). On error the previous
    /// order is kept.
    pub fn set_sort(&self, spec: Option<SortSpec>) -> Result<(), Box<dyn std::error::Error>> {
        if self.sort() == spec {
            return Ok(());
        }

        let sorting = match spec {
            Some(spec) => {
                let column = &self.flattened_columns[spec.column];
                let row_ids = Self::sorted_row_ids(&self.file_path, column, spec.order)?;
                Some(Sorting { spec, row_ids })
            }
            None => None,
        };
        *self.sorting.borrow_mut() = sorting;
        // Invalidate so the next `ensure_loaded` reads rows in the new order.
        *self.window.borrow_mut() = RowWindow::default();
        Ok(())
    }

    /// Reload the bounded window when the requested viewport is outside it.
    pub fn ensure_loaded(&self, view_start: usize, view_len: usize) {
        let view_len = view_len.min(self.total_rows.saturating_sub(view_start));
        if self.window.borrow().covers(view_start, view_len) {
            return;
        }

        let new_start = view_start.saturating_sub(ROW_PREFETCH);
        let len = WINDOW_ROWS.min(self.total_rows.saturating_sub(new_start));
        if len == 0 {
            return;
        }

        let loaded = match self.sorting.borrow().as_ref() {
            Some(sorting) => Self::load_by_row_ids(
                &self.file_path,
                &sorting.row_ids[new_start..new_start + len],
            ),
            None => Self::load(&self.file_path, new_start, len),
        };
        if let Ok((rows, _columns)) = loaded {
            let mut window = self.window.borrow_mut();
            window.start = new_start;
            window.rows = rows;
        }
    }

    pub fn loaded(&self) -> Ref<'_, RowWindow> {
        self.window.borrow()
    }

    fn flatten_struct_columns(df: DataFrame) -> DataFrame {
        // Struct columns are currently displayed with their string representation.
        df
    }

    fn get_value_as_string(col: &Series, row_idx: usize) -> String {
        match col.get(row_idx) {
            Ok(any_value) if any_value.is_null() => "NULL".to_string(),
            Ok(any_value) => format!("{any_value}"),
            Err(_) => "NULL".to_string(),
        }
    }
}

fn catch_polars_panic<T>(
    operation: impl FnOnce() -> Result<T, Box<dyn std::error::Error>> + std::panic::UnwindSafe,
) -> Result<T, Box<dyn std::error::Error>> {
    let _lock = POLARS_PANIC_HOOK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(operation);
    std::panic::set_hook(previous_hook);

    match result {
        Ok(result) => result,
        Err(payload) => Err(panic_message(&payload).into()),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "polars panicked while reading this file".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_data_path(filename: &str) -> String {
        format!("{}/{}", crate::file::parquet_test_data(), filename)
    }

    fn wide_file() -> String {
        test_data_path("alltypes_tiny_pages.parquet")
    }

    #[test]
    fn open_reports_full_metadata_not_just_the_window() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        assert_eq!(data.total_rows, 7300);
        assert_eq!(data.total_columns, 13);
        assert_eq!(data.loaded().rows.len(), WINDOW_ROWS);
        assert_eq!(data.loaded().start, 0);
    }

    #[test]
    fn ensure_loaded_slides_the_window_for_a_far_scroll() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        data.ensure_loaded(6000, 40);
        let win = data.loaded();
        assert_eq!(win.start, 6000 - ROW_PREFETCH);
        assert!(win.covers(6000, 40));
        assert!(win.rows.len() <= WINDOW_ROWS);
    }

    #[test]
    fn ensure_loaded_is_a_noop_inside_the_window() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        let start_before = data.loaded().start;
        data.ensure_loaded(50, 40);
        assert_eq!(data.loaded().start, start_before);
    }

    #[test]
    fn windowed_rows_match_a_direct_read_at_the_same_offset() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        data.ensure_loaded(6000, 10);
        let (direct, _) = ParquetSampleData::load(&wide_file(), 6000, 10).unwrap();
        let win = data.loaded();
        let rel = 6000 - win.start;
        assert_eq!(&win.rows[rel..rel + 10], &direct[..]);
    }

    fn sort_spec(data: &ParquetSampleData, column: &str, order: SortOrder) -> SortSpec {
        let column = data
            .flattened_columns
            .iter()
            .position(|name| name == column)
            .unwrap();
        SortSpec { column, order }
    }

    fn ids(data: &ParquetSampleData, view_start: usize, len: usize) -> Vec<i64> {
        data.ensure_loaded(view_start, len);
        let win = data.loaded();
        let id_col = data
            .flattened_columns
            .iter()
            .position(|c| c == "id")
            .unwrap();
        let rel = view_start - win.start;
        win.rows[rel..rel + len]
            .iter()
            .map(|row| row[id_col].parse().unwrap())
            .collect()
    }

    #[test]
    fn sorting_descending_then_ascending_then_back_to_file_order() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        let file_order = ids(&data, 0, 5);

        data.set_sort(Some(sort_spec(&data, "id", SortOrder::Descending)))
            .unwrap();
        assert_eq!(ids(&data, 0, 3), vec![7299, 7298, 7297]);

        data.set_sort(Some(sort_spec(&data, "id", SortOrder::Ascending)))
            .unwrap();
        assert_eq!(ids(&data, 0, 3), vec![0, 1, 2]);

        data.set_sort(None).unwrap();
        assert_eq!(ids(&data, 0, 5), file_order);
    }

    #[test]
    fn sorted_order_holds_past_the_first_window() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        data.set_sort(Some(sort_spec(&data, "id", SortOrder::Descending)))
            .unwrap();
        assert_eq!(ids(&data, 6000, 3), vec![1299, 1298, 1297]);
    }

    #[test]
    fn sorting_by_a_column_with_ties_keeps_file_order_within_ties() {
        let data = ParquetSampleData::open(&wide_file(), 7300).unwrap();
        data.set_sort(Some(sort_spec(&data, "bool_col", SortOrder::Descending)))
            .unwrap();
        let sorting = data.sorting.borrow();
        // bool_col alternates, so the first half is every "true" row.
        let trues = &sorting.as_ref().unwrap().row_ids[..3650];
        assert!(trues.windows(2).all(|w| w[0] < w[1]));
    }

    fn write_geoarrow_wkb_file() -> std::path::PathBuf {
        use arrow::array::{ArrayRef, BinaryArray, RecordBatch};
        use arrow::datatypes::{DataType, Field, Schema};
        use parquet::arrow::ArrowWriter;
        use std::collections::HashMap;
        use std::sync::Arc;

        let field = Field::new("geom", DataType::Binary, true).with_metadata(HashMap::from([
            (
                "ARROW:extension:name".to_string(),
                "geoarrow.wkb".to_string(),
            ),
            (
                "ARROW:extension:metadata".to_string(),
                r#"{"crs":"EPSG:4326"}"#.to_string(),
            ),
        ]));
        let schema = Arc::new(Schema::new(vec![field]));
        let values: ArrayRef = Arc::new(BinaryArray::from_opt_vec(vec![
            Some(&[0x01, 0x01, 0x00, 0x00, 0x00][..]),
            None,
        ]));
        let batch = RecordBatch::try_new(schema.clone(), vec![values]).unwrap();
        let path = std::env::temp_dir().join(format!(
            "parqeye_geoarrow_wkb_{}.parquet",
            std::process::id()
        ));
        let mut writer =
            ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        path
    }

    #[test]
    fn reads_arrow_extension_types() {
        let path = write_geoarrow_wkb_file();
        let data = ParquetSampleData::open(path.to_str().unwrap(), 2).unwrap();
        assert_eq!(data.flattened_columns, vec!["geom"]);
        assert_eq!(data.total_rows, 2);
        assert_eq!(data.loaded().rows[1][0], "NULL");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unsupported_file_errors_instead_of_panicking() {
        let err =
            ParquetSampleData::open(&test_data_path("geospatial/crs-srid.parquet"), 1).unwrap_err();
        assert!(
            err.to_string().contains("LogicalType"),
            "unexpected error: {err}"
        );
    }
}
