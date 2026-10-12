//! Distribution math over sorted, finite `f64` samples.

const INV_SQRT_2PI: f64 = 0.398_942_280_401_432_7;
/// Resolution of the pre-binning used by `kde`; keeps it O(points * grid)
/// instead of O(points * samples) on large columns.
const KDE_GRID: usize = 1024;
/// Gaussian kernel is negligible beyond this many bandwidths.
const KDE_CUTOFF_BANDWIDTHS: f64 = 4.0;
/// IQR of a standard normal, used by Silverman's rule.
const NORMAL_IQR: f64 = 1.34;

/// Linear-interpolated quantile of sorted values, `q` in [0, 1].
pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

pub struct Moments {
    pub mean: f64,
    pub std: f64,
    pub skew: f64,
}

pub fn moments(values: &[f64]) -> Moments {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let (m2, m3) = values.iter().fold((0.0, 0.0), |(m2, m3), v| {
        let d = v - mean;
        (m2 + d * d, m3 + d * d * d)
    });
    let var = m2 / n;
    let std = var.sqrt();
    let skew = if std > 0.0 { m3 / n / (var * std) } else { 0.0 };
    Moments { mean, std, skew }
}

/// Values in `[lo, hi]` of a sorted slice.
pub fn in_range(sorted: &[f64], lo: f64, hi: f64) -> &[f64] {
    let start = sorted.partition_point(|&v| v < lo);
    let end = sorted.partition_point(|&v| v <= hi);
    &sorted[start..end.max(start)]
}

pub struct Histogram {
    pub start: f64,
    pub width: f64,
    pub counts: Vec<usize>,
}

impl Histogram {
    pub fn end(&self) -> f64 {
        self.start + self.width * self.counts.len() as f64
    }
}

/// Equal-width histogram of sorted values over `[lo, hi]`. With
/// `integer_aligned` the bin width is a whole number and bins are centered on
/// integers, so integer data doesn't alias into alternating spiky bins.
pub fn histogram(
    sorted: &[f64],
    lo: f64,
    hi: f64,
    bins: usize,
    integer_aligned: bool,
) -> Histogram {
    let bins = bins.max(1);
    let (start, width, bins) = if integer_aligned {
        let lo = lo.round();
        let span = hi.round() - lo + 1.0;
        let width = (span / bins as f64).ceil().max(1.0);
        (lo - 0.5, width, (span / width).ceil() as usize)
    } else if hi > lo {
        (lo, (hi - lo) / bins as f64, bins)
    } else {
        // A single value: one bin of nominal width around it.
        (lo - 0.5, 1.0, 1)
    };

    let mut counts = vec![0usize; bins];
    for &v in in_range(sorted, start, start + width * bins as f64) {
        let idx = ((v - start) / width) as usize;
        counts[idx.min(bins - 1)] += 1;
    }
    Histogram {
        start,
        width,
        counts,
    }
}

/// Silverman's rule-of-thumb bandwidth.
pub fn silverman_bandwidth(sorted: &[f64]) -> f64 {
    let n = sorted.len() as f64;
    let std = moments(sorted).std;
    let iqr = quantile(sorted, 0.75) - quantile(sorted, 0.25);
    let spread = if iqr > 0.0 {
        std.min(iqr / NORMAL_IQR)
    } else {
        std
    };
    let bandwidth = 0.9 * spread * n.powf(-0.2);
    if bandwidth > 0.0 {
        bandwidth
    } else {
        // Constant data; any positive width gives a single spike.
        1.0
    }
}

/// Gaussian kernel density estimate at `points` evenly spaced positions over
/// `[lo, hi]` (at bin centers), integrating to 1 over the sample.
pub fn kde(sorted: &[f64], lo: f64, hi: f64, bandwidth: f64, points: usize) -> Vec<f64> {
    if sorted.is_empty() || points == 0 || hi <= lo {
        return vec![0.0; points];
    }
    // Pre-bin a margin around the view so mass just outside still bleeds in.
    let margin = KDE_CUTOFF_BANDWIDTHS * bandwidth;
    let grid = histogram(sorted, lo - margin, hi + margin, KDE_GRID, false);
    let n = sorted.len() as f64;
    let step = (hi - lo) / points as f64;

    (0..points)
        .map(|i| {
            let x = lo + step * (i as f64 + 0.5);
            let sum: f64 = grid
                .counts
                .iter()
                .enumerate()
                .filter(|(_, c)| **c > 0)
                .map(|(j, &c)| {
                    let center = grid.start + grid.width * (j as f64 + 0.5);
                    let z = (x - center) / bandwidth;
                    if z.abs() > KDE_CUTOFF_BANDWIDTHS {
                        0.0
                    } else {
                        c as f64 * (-0.5 * z * z).exp()
                    }
                })
                .sum();
            sum * INV_SQRT_2PI / (n * bandwidth)
        })
        .collect()
}

/// Local maxima rising at least `min_prominence` (fraction of the global max)
/// above the valleys on both sides.
pub fn count_peaks(ys: &[f64], min_prominence: f64) -> usize {
    let max = ys.iter().cloned().fold(0.0, f64::max);
    if max <= 0.0 {
        return 0;
    }
    let threshold = max * min_prominence;
    let mut peaks = 0;
    let mut valley = 0.0f64;
    let mut climbing_peak: Option<f64> = None;
    for &y in ys {
        match climbing_peak {
            None => {
                valley = valley.min(y);
                if y - valley >= threshold {
                    climbing_peak = Some(y);
                }
            }
            Some(peak) if y > peak => climbing_peak = Some(y),
            Some(peak) if peak - y >= threshold => {
                peaks += 1;
                climbing_peak = None;
                valley = y;
            }
            Some(_) => {}
        }
    }
    peaks + usize::from(climbing_peak.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantile_interpolates_between_values() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(quantile(&v, 0.0), 1.0);
        assert_eq!(quantile(&v, 1.0), 4.0);
        assert_eq!(quantile(&v, 0.5), 2.5);
    }

    #[test]
    fn moments_of_a_symmetric_sample() {
        let m = moments(&[1.0, 2.0, 3.0]);
        assert_eq!(m.mean, 2.0);
        assert!(m.skew.abs() < 1e-12);
    }

    #[test]
    fn right_tail_gives_positive_skew() {
        assert!(moments(&[1.0, 1.0, 1.0, 1.0, 10.0]).skew > 1.0);
    }

    #[test]
    fn histogram_counts_every_value_including_the_maximum() {
        let v: Vec<f64> = (0..100).map(f64::from).collect();
        let h = histogram(&v, 0.0, 99.0, 10, false);
        assert_eq!(h.counts.iter().sum::<usize>(), 100);
        assert_eq!(h.counts.len(), 10);
    }

    #[test]
    fn integer_aligned_bins_are_whole_and_centered() {
        let v = [1.0, 2.0, 2.0, 3.0, 3.0, 3.0];
        let h = histogram(&v, 1.0, 3.0, 10, true);
        assert_eq!(h.width, 1.0);
        assert_eq!(h.start, 0.5);
        assert_eq!(h.counts, vec![1, 2, 3]);
    }

    #[test]
    fn histogram_of_a_constant_column_has_one_bin() {
        let h = histogram(&[5.0, 5.0], 5.0, 5.0, 10, false);
        assert_eq!(h.counts, vec![2]);
    }

    #[test]
    fn kde_integrates_to_about_one() {
        let v: Vec<f64> = (0..1000).map(|i| (i % 100) as f64).collect();
        let bw = silverman_bandwidth(&v);
        let (lo, hi, points) = (-50.0, 150.0, 400);
        let area: f64 = kde(&v, lo, hi, bw, points).iter().sum::<f64>() * (hi - lo) / points as f64;
        assert!((area - 1.0).abs() < 0.02, "area {area}");
    }

    #[test]
    fn two_separated_clusters_give_two_peaks() {
        let mut v: Vec<f64> = (0..500).map(|i| (i % 10) as f64 * 0.1).collect();
        v.extend((0..500).map(|i| 10.0 + (i % 10) as f64 * 0.1));
        v.sort_by(f64::total_cmp);
        let ys = kde(&v, -1.0, 12.0, 0.5, 200);
        assert_eq!(count_peaks(&ys, 0.1), 2);
    }

    #[test]
    fn a_single_bump_is_one_peak() {
        let ys = [0.0, 1.0, 3.0, 5.0, 3.0, 1.0, 0.0];
        assert_eq!(count_peaks(&ys, 0.1), 1);
    }
}
