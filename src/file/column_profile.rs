use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};

use polars::prelude::*;

use crate::file::sample_data::catch_polars_panic;
use crate::file::stats;
use crate::file::utils::commas;

/// Numeric columns with at most this many distinct values read better as
/// bars of their values than as a histogram.
pub const FEW_DISTINCT: usize = 15;
/// Value counts kept per column; the rest are summarized as "other".
const MAX_TOP_VALUES: usize = 500;
const OUTLIER_IQR_FACTOR: f64 = 1.5;
const PEAK_PROMINENCE: f64 = 0.15;
const PEAK_POINTS: usize = 200;
const SKEW_NOTABLE: f64 = 1.0;
const NULL_SHARE_NOTABLE: f64 = 0.2;
const ZERO_SHARE_NOTABLE: f64 = 0.1;
const DOMINANT_SHARE: f64 = 0.5;
const TOP_COVERAGE_COUNT: usize = 10;
/// Clip range used for the "o" toggle and for peak detection, so a few
/// extreme values don't squash the bulk of the distribution.
pub const CLIP_QUANTILES: (f64, f64) = (0.01, 0.99);

#[derive(Debug)]
pub struct ColumnProfile {
    pub name: String,
    pub dtype: String,
    pub rows: usize,
    pub nulls: usize,
    pub kind: ProfileKind,
    pub insights: Vec<String>,
}

#[derive(Debug)]
pub enum ProfileKind {
    Numeric(NumericProfile),
    Categorical(TopValues),
    Unsupported(String),
}

#[derive(Debug)]
pub struct NumericProfile {
    /// Sorted, finite, non-null values.
    pub values: Vec<f64>,
    pub non_finite: usize,
    pub format: ValueFormat,
    /// Whole-number data (integer or temporal dtype, or floats that are all
    /// integral); histograms align their bins to integers.
    pub integer: bool,
    /// The column is a list; values are the list lengths.
    pub of_list_lengths: bool,
    pub mean: f64,
    pub std: f64,
    pub skew: f64,
    pub top: TopValues,
}

impl NumericProfile {
    pub fn quantile(&self, q: f64) -> f64 {
        stats::quantile(&self.values, q)
    }

    pub fn min(&self) -> f64 {
        self.values.first().copied().unwrap_or(f64::NAN)
    }

    pub fn max(&self) -> f64 {
        self.values.last().copied().unwrap_or(f64::NAN)
    }

    /// Default view for this column: few distinct values as bars, floats as a
    /// smooth density, everything else as a histogram.
    pub fn prefers_bars(&self) -> bool {
        self.top.distinct <= FEW_DISTINCT
    }

    pub fn prefers_density(&self) -> bool {
        !self.integer && !self.prefers_bars()
    }
}

#[derive(Debug)]
pub struct ValueCount {
    pub label: String,
    pub count: usize,
    /// Set for numeric columns so "sort by value" can order numerically.
    pub numeric: Option<f64>,
}

#[derive(Debug, Default)]
pub struct TopValues {
    /// Most frequent first, ties in value order; at most `MAX_TOP_VALUES`.
    pub entries: Vec<ValueCount>,
    pub distinct: usize,
    pub singletons: usize,
    /// Non-null values counted.
    pub total: usize,
}

/// A candidate for the top values, ranked by count, ties to the smaller `order`.
struct Ranked<T, O> {
    count: usize,
    order: Reverse<O>,
    value: T,
}

impl<T, O: Ord> Ranked<T, O> {
    fn key(&self) -> (usize, &Reverse<O>) {
        (self.count, &self.order)
    }
}

impl<T, O: Ord> PartialEq for Ranked<T, O> {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl<T, O: Ord> Eq for Ranked<T, O> {}

impl<T, O: Ord> PartialOrd for Ranked<T, O> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T, O: Ord> Ord for Ranked<T, O> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl TopValues {
    /// Keep the most frequent `(value, count)` pairs. Only the kept ones are
    /// turned into entries, so a column of millions of distinct values doesn't
    /// allocate a label for each.
    fn collect<T, O: Ord>(
        counts: impl Iterator<Item = (T, usize)>,
        order: impl Fn(&T) -> O,
        entry: impl Fn(T, usize) -> ValueCount,
    ) -> Self {
        let mut heap = BinaryHeap::with_capacity(MAX_TOP_VALUES + 1);
        let (mut total, mut distinct, mut singletons) = (0, 0, 0);
        for (value, count) in counts {
            total += count;
            distinct += 1;
            singletons += usize::from(count == 1);
            heap.push(Reverse(Ranked {
                count,
                order: Reverse(order(&value)),
                value,
            }));
            if heap.len() > MAX_TOP_VALUES {
                heap.pop();
            }
        }
        let entries = heap
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse(ranked)| entry(ranked.value, ranked.count))
            .collect();
        Self {
            entries,
            distinct,
            singletons,
            total,
        }
    }

    pub fn share(&self, count: usize) -> f64 {
        count as f64 / self.total.max(1) as f64
    }
}

#[derive(Debug, Clone)]
pub enum ValueFormat {
    Integer,
    Float,
    Temporal(DataType),
}

impl ValueFormat {
    /// A single value at full useful precision (stats, labels).
    pub fn format(&self, v: f64) -> String {
        match self {
            ValueFormat::Integer => format_integer(v),
            ValueFormat::Float => format_float(v),
            ValueFormat::Temporal(dtype) => format_temporal(v, dtype),
        }
    }

    /// An axis tick, with precision matched to the spacing between ticks.
    pub fn format_tick(&self, v: f64, step: f64) -> String {
        match self {
            ValueFormat::Temporal(dtype) => {
                let label = format_temporal(v, dtype);
                // Ticks a day or more apart don't need the time of day.
                match day_length(dtype) {
                    Some(day) if step >= day => {
                        label.split(' ').next().unwrap_or(&label).to_string()
                    }
                    _ => label,
                }
            }
            _ if v != 0.0 && (v.abs() >= 1e6 || v.abs() < 1e-4) => format!("{v:.1e}"),
            _ => {
                let decimals = if step > 0.0 {
                    (-step.log10().floor()).clamp(0.0, 6.0) as usize
                } else {
                    0
                };
                format!("{v:.decimals$}")
            }
        }
    }

    /// A spread or width (std dev, bin width): for temporal data a duration
    /// rather than a point in time.
    pub fn format_spread(&self, v: f64) -> String {
        let unit = match self {
            ValueFormat::Temporal(DataType::Datetime(unit, _) | DataType::Duration(unit)) => *unit,
            ValueFormat::Temporal(DataType::Time) => TimeUnit::Nanoseconds,
            ValueFormat::Temporal(_) => return format!("{} days", format_float(v)),
            _ => return format_float(v),
        };
        let per_second = units_per_second(unit);
        // Two parts are enough: "88d 17h", not "88d 16h 38m 23s 334ms".
        let units = [SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, 1.0];
        let largest = units
            .iter()
            .position(|seconds| v >= seconds * per_second)
            .unwrap_or(units.len() - 1);
        let resolution = units[(largest + 1).min(units.len() - 1)] * per_second;
        let rounded = (v / resolution).round() * resolution;
        format_temporal(rounded, &DataType::Duration(unit))
    }
}

/// Above this, decimals of a fractional value in an integer column (e.g. its
/// mean) are below display precision anyway.
const INTEGER_DECIMALS_BELOW: f64 = 1_000.0;
const SECONDS_PER_MINUTE: f64 = 60.0;
const SECONDS_PER_HOUR: f64 = 3_600.0;
const SECONDS_PER_DAY: f64 = 86_400.0;
fn units_per_second(unit: TimeUnit) -> f64 {
    match unit {
        TimeUnit::Nanoseconds => 1e9,
        TimeUnit::Microseconds => 1e6,
        TimeUnit::Milliseconds => 1e3,
    }
}

/// Physical units in one day, for datetime-like types.
fn day_length(dtype: &DataType) -> Option<f64> {
    match dtype {
        DataType::Datetime(unit, _) => Some(SECONDS_PER_DAY * units_per_second(*unit)),
        _ => None,
    }
}

/// Integral values with thousands separators; fractional ones (a mean, an
/// interpolated quantile) keep their decimals.
fn format_integer(v: f64) -> String {
    if v.fract() != 0.0 && v.abs() < INTEGER_DECIMALS_BELOW {
        return format_float(v);
    }
    let sign = if v < 0.0 { "-" } else { "" };
    format!("{sign}{}", commas(v.abs().round() as u64))
}

/// Four significant digits, trailing zeros trimmed.
pub fn format_float(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    if v.abs() >= 1e7 || v.abs() < 1e-4 {
        return format!("{v:.3e}");
    }
    let decimals = (3 - v.abs().log10().floor() as i32).clamp(0, 6) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn format_temporal(v: f64, dtype: &DataType) -> String {
    Series::new(PlSmallStr::EMPTY, [v.round() as i64])
        .cast(&dtype.to_physical())
        .and_then(|s| s.cast(dtype))
        .ok()
        .and_then(|s| s.get(0).ok().map(|value| format!("{value}")))
        // Sub-second digits are noise at plotting resolution.
        .map(|s| s.split('.').next().unwrap_or(&s).to_string())
        .map(|s| s.trim_end_matches(" 00:00:00").to_string())
        .unwrap_or_else(|| format_float(v))
}

impl ColumnProfile {
    /// Read one column of the file and summarize it. Polars panics on some
    /// unsupported formats; those become an `Unsupported` profile.
    pub fn load(file_path: &str, column: &str, rows: usize) -> ColumnProfile {
        let path = file_path.to_string();
        let name = column.to_string();
        catch_polars_panic(move || Self::load_with_polars(&path, &name, rows)).unwrap_or_else(|e| {
            ColumnProfile {
                name: column.to_string(),
                dtype: String::new(),
                rows,
                nulls: 0,
                kind: ProfileKind::Unsupported(e.to_string()),
                insights: Vec::new(),
            }
        })
    }

    fn load_with_polars(
        file_path: &str,
        column: &str,
        rows: usize,
    ) -> Result<ColumnProfile, Box<dyn std::error::Error>> {
        let mut lf = LazyFrame::scan_parquet(PlRefPath::new(file_path), Default::default())?;
        let dtype = lf
            .collect_schema()?
            .get(column)
            .cloned()
            .ok_or_else(|| format!("column {column} not found"))?;

        let is_list = matches!(dtype, DataType::List(_));
        let expr = if is_list {
            col(column).list().len()
        } else {
            col(column)
        };
        let df = lf.select([expr.alias(column)]).collect()?;
        let series = df.column(column)?.as_materialized_series();
        let nulls = series.null_count();

        let kind = if matches!(dtype, DataType::Struct(_) | DataType::Array(_, _)) {
            ProfileKind::Unsupported(format!(
                "{dtype} columns can't be plotted yet; only their string form is shown in the table."
            ))
        } else if is_list
            || dtype.is_primitive_numeric()
            || dtype.is_decimal()
            || dtype.is_temporal()
        {
            ProfileKind::Numeric(numeric_profile(series, &dtype, is_list)?)
        } else {
            ProfileKind::Categorical(categorical_counts(series)?)
        };

        let mut profile = ColumnProfile {
            name: column.to_string(),
            dtype: dtype.to_string(),
            rows,
            nulls,
            kind,
            insights: Vec::new(),
        };
        profile.insights = insights(&profile);
        Ok(profile)
    }
}

fn numeric_profile(
    series: &Series,
    dtype: &DataType,
    of_list_lengths: bool,
) -> Result<NumericProfile, Box<dyn std::error::Error>> {
    // Temporal values are plotted on their physical (integer) scale and only
    // formatted back for labels; decimals must cast directly to keep their scale.
    let floats = if dtype.is_temporal() {
        series.to_physical_repr().cast(&DataType::Float64)?
    } else {
        series.cast(&DataType::Float64)?
    };

    let mut non_finite = 0;
    let mut values = Vec::with_capacity(floats.len());
    for v in floats.f64()?.iter().flatten() {
        if v.is_finite() {
            values.push(v);
        } else {
            non_finite += 1;
        }
    }
    values.sort_unstable_by(f64::total_cmp);

    let integer = dtype.is_integer()
        || dtype.is_temporal()
        || of_list_lengths
        || values.iter().all(|v| v.fract() == 0.0);
    let format = if dtype.is_temporal() {
        ValueFormat::Temporal(dtype.clone())
    } else if integer {
        ValueFormat::Integer
    } else {
        ValueFormat::Float
    };

    // Runs of equal values in sorted order, indexed so ties rank by value.
    let runs = values
        .chunk_by(|a, b| a == b)
        .enumerate()
        .map(|(idx, run)| ((idx, run[0]), run.len()));
    let top = TopValues::collect(
        runs,
        |(idx, _)| *idx,
        |(_, v), count| ValueCount {
            label: format.format(v),
            count,
            numeric: Some(v),
        },
    );

    let (mean, std, skew) = if values.is_empty() {
        (f64::NAN, f64::NAN, 0.0)
    } else {
        let m = stats::moments(&values);
        (m.mean, m.std, m.skew)
    };

    Ok(NumericProfile {
        values,
        non_finite,
        format,
        integer,
        of_list_lengths,
        mean,
        std,
        skew,
        top,
    })
}

fn categorical_counts(series: &Series) -> Result<TopValues, Box<dyn std::error::Error>> {
    let strings = series.cast(&DataType::String)?;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in strings.str()?.iter().flatten() {
        *counts.entry(s).or_default() += 1;
    }
    Ok(TopValues::collect(
        counts.into_iter(),
        |label| *label,
        |label, count| ValueCount {
            label: label.to_string(),
            count,
            numeric: None,
        },
    ))
}

fn pct(share: f64) -> String {
    format!("{:.1}%", share * 100.0)
}

fn insights(profile: &ColumnProfile) -> Vec<String> {
    let mut out = Vec::new();
    let null_share = profile.nulls as f64 / profile.rows.max(1) as f64;
    if null_share >= NULL_SHARE_NOTABLE {
        out.push(format!("{} of rows are null", pct(null_share)));
    }

    match &profile.kind {
        ProfileKind::Numeric(p) => numeric_insights(p, &mut out),
        ProfileKind::Categorical(top) => categorical_insights(top, &mut out),
        ProfileKind::Unsupported(_) => {}
    }
    out
}

fn numeric_insights(p: &NumericProfile, out: &mut Vec<String>) {
    let n = p.values.len();
    if p.of_list_lengths {
        out.insert(
            0,
            "List column: plotting the length of each list".to_string(),
        );
    }
    if p.non_finite > 0 {
        out.push(format!(
            "{} NaN/inf values left out",
            commas(p.non_finite as u64)
        ));
    }
    if n == 0 {
        out.push("No values to plot".to_string());
        return;
    }
    if p.top.distinct == 1 {
        out.push(format!(
            "Constant: every value is {}",
            p.format.format(p.min())
        ));
        return;
    }
    if p.top.distinct == n && p.integer {
        out.push("Every value is unique: looks like an ID or key".to_string());
    }
    if p.prefers_bars() {
        out.push(format!(
            "Only {} distinct values, shown as bars",
            p.top.distinct
        ));
    }

    if p.skew > SKEW_NOTABLE {
        let log_helps = p.min() > 0.0 && !matches!(p.format, ValueFormat::Temporal(_));
        let hint = if log_helps { " (try log x)" } else { "" };
        out.push(format!("Right-skewed, skew {:.1}{hint}", p.skew));
    } else if p.skew < -SKEW_NOTABLE {
        out.push(format!("Left-skewed, skew {:.1}", p.skew));
    }

    let (q1, q3) = (p.quantile(0.25), p.quantile(0.75));
    let iqr = q3 - q1;
    if iqr > 0.0 {
        let (lo, hi) = (q1 - OUTLIER_IQR_FACTOR * iqr, q3 + OUTLIER_IQR_FACTOR * iqr);
        let outliers = n - stats::in_range(&p.values, lo, hi).len();
        if outliers > 0 {
            out.push(format!(
                "{} outliers ({}) beyond 1.5×IQR",
                commas(outliers as u64),
                pct(outliers as f64 / n as f64)
            ));
        }
    }

    let zeros = stats::in_range(&p.values, 0.0, 0.0).len();
    let zero_share = zeros as f64 / n as f64;
    if zero_share >= ZERO_SHARE_NOTABLE && p.top.distinct > 2 {
        out.push(format!("{} of values are zero", pct(zero_share)));
    }

    if !p.prefers_bars() {
        let lo = p.quantile(CLIP_QUANTILES.0);
        let hi = p.quantile(CLIP_QUANTILES.1);
        let core = stats::in_range(&p.values, lo, hi);
        if core.len() > 1 && hi > lo {
            let ys = stats::kde(core, lo, hi, stats::silverman_bandwidth(core), PEAK_POINTS);
            let peaks = stats::count_peaks(&ys, PEAK_PROMINENCE);
            if peaks >= 2 {
                out.push(format!("Multimodal: about {peaks} peaks"));
            }
        }
    }
}

fn categorical_insights(top: &TopValues, out: &mut Vec<String>) {
    let Some(first) = top.entries.first() else {
        out.push("No values to plot".to_string());
        return;
    };
    if top.distinct == 1 {
        out.push(format!("Constant: every value is \"{}\"", first.label));
        return;
    }
    if top.distinct == top.total {
        out.push("Every value is unique: an ID or free text".to_string());
        return;
    }
    let first_share = top.share(first.count);
    if first_share >= DOMINANT_SHARE {
        out.push(format!(
            "\"{}\" dominates with {}",
            first.label,
            pct(first_share)
        ));
    }
    if top.distinct > TOP_COVERAGE_COUNT {
        let covered: usize = top
            .entries
            .iter()
            .take(TOP_COVERAGE_COUNT)
            .map(|e| e.count)
            .sum();
        out.push(format!(
            "Top {TOP_COVERAGE_COUNT} of {} values cover {}",
            commas(top.distinct as u64),
            pct(top.share(covered))
        ));
    }
    if top.singletons > 0 && top.distinct > TOP_COVERAGE_COUNT {
        out.push(format!(
            "{} values appear only once",
            commas(top.singletons as u64)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file() -> String {
        format!(
            "{}/alltypes_tiny_pages.parquet",
            crate::file::parquet_test_data()
        )
    }

    fn numeric(profile: &ColumnProfile) -> &NumericProfile {
        match &profile.kind {
            ProfileKind::Numeric(p) => p,
            other => panic!("expected numeric, got {other:?}"),
        }
    }

    #[test]
    fn integer_id_column_is_numeric_and_unique() {
        let profile = ColumnProfile::load(&test_file(), "id", 7300);
        let p = numeric(&profile);
        assert_eq!(p.values.len(), 7300);
        assert_eq!(p.min(), 0.0);
        assert_eq!(p.max(), 7299.0);
        assert!(p.integer);
        assert_eq!(p.top.distinct, 7300);
        assert!(profile.insights.iter().any(|i| i.contains("unique")));
    }

    #[test]
    fn small_integer_column_prefers_bars() {
        let profile = ColumnProfile::load(&test_file(), "tinyint_col", 7300);
        let p = numeric(&profile);
        assert!(p.prefers_bars());
        assert_eq!(p.top.total, 7300);
    }

    #[test]
    fn float_column_prefers_density_unless_few_values() {
        let profile = ColumnProfile::load(&test_file(), "double_col", 7300);
        let p = numeric(&profile);
        assert_eq!(p.prefers_density(), !p.prefers_bars() && !p.integer);
    }

    #[test]
    fn boolean_column_is_categorical() {
        let profile = ColumnProfile::load(&test_file(), "bool_col", 7300);
        let ProfileKind::Categorical(top) = &profile.kind else {
            panic!("expected categorical");
        };
        assert_eq!(top.distinct, 2);
        assert_eq!(top.total, 7300);
    }

    #[test]
    fn timestamp_column_formats_as_dates() {
        let profile = ColumnProfile::load(&test_file(), "timestamp_col", 7300);
        let p = numeric(&profile);
        let label = p.format.format(p.min());
        assert!(label.starts_with("20"), "got {label}");
    }

    #[test]
    fn missing_column_is_unsupported_not_a_panic() {
        let profile = ColumnProfile::load(&test_file(), "no_such_column", 7300);
        assert!(matches!(profile.kind, ProfileKind::Unsupported(_)));
    }

    #[test]
    fn format_float_keeps_four_significant_digits() {
        assert_eq!(format_float(1.23456), "1.235");
        assert_eq!(format_float(1234.5678), "1235");
        assert_eq!(format_float(2.5), "2.5");
    }

    #[test]
    fn top_values_keep_the_most_frequent_with_ties_in_value_order() {
        let counts = (0..2 * MAX_TOP_VALUES).map(|i| (i, if i == 7 { 5 } else { 1 }));
        let top = TopValues::collect(
            counts,
            |i| *i,
            |i, count| ValueCount {
                label: i.to_string(),
                count,
                numeric: None,
            },
        );
        assert_eq!(top.entries.len(), MAX_TOP_VALUES);
        assert_eq!(top.distinct, 2 * MAX_TOP_VALUES);
        assert_eq!(top.singletons, 2 * MAX_TOP_VALUES - 1);
        let labels: Vec<&str> = top
            .entries
            .iter()
            .take(3)
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels, vec!["7", "0", "1"]);
    }

    #[test]
    fn integer_format_keeps_decimals_of_a_mean() {
        assert_eq!(ValueFormat::Integer.format(12345.0), "12,345");
        assert_eq!(ValueFormat::Integer.format(2.1), "2.1");
        assert_eq!(ValueFormat::Integer.format(500_066.7), "500,067");
    }

    #[test]
    fn temporal_spread_is_a_duration() {
        let format = ValueFormat::Temporal(DataType::Datetime(TimeUnit::Microseconds, None));
        let one_day = 86_400e6;
        assert_eq!(format.format_spread(one_day), "1d");
        let noisy = 88.0 * one_day + 16.0 * 3_600e6 + 38.0 * 60e6 + 23_334_000.0;
        assert_eq!(format.format_spread(noisy), "88d 17h");
        assert!(
            !format
                .format_tick(one_day * 100.0, one_day * 30.0)
                .contains(':')
        );
    }
}
