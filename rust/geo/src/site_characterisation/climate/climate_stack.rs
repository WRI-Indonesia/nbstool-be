//! Shared reading of the WorldClim monthly stack (`climate/climate_stack.py`), used by 3.3 and
//! 3.4. A pixel counts only when ALL TWELVE months are present; the same pixel set feeds the
//! chart and the sentence.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_bands_clipped};
use crate::config::{CLIMATE_MIN_PIXELS, MONTH_LABELS, WORLDCLIM_MONTHS};
use crate::py::np_sum;
use crate::settings::Settings;

/// One bar of a twelve month chart: the spatial mean over the AOI for that month.
pub struct MonthlyValue {
    pub month: i64,
    pub label: &'static str,
    pub value: f64,
}

impl MonthlyValue {
    pub fn to_json(&self) -> Value {
        json!({"month": self.month, "label": self.label, "value": self.value})
    }
}

/// Twelve monthly rasters read over the AOI, restricted to fully valid pixels.
pub struct ClimateStack {
    pub monthly: Vec<MonthlyValue>,
    /// one annual value per valid pixel, in row-major order
    pub annual: Vec<f64>,
    pub n_pixels: usize,
}

impl ClimateStack {
    pub fn has_range(&self) -> bool {
        self.n_pixels >= CLIMATE_MIN_PIXELS && self.max() > self.min()
    }

    /// `annual.mean()`
    pub fn mean(&self) -> f64 {
        np_sum(&self.annual) / self.annual.len() as f64
    }

    /// `annual.min()` (the annual values are never NaN: NaN pixels are excluded)
    pub fn min(&self) -> f64 {
        self.annual.iter().copied().fold(f64::INFINITY, f64::min)
    }

    pub fn max(&self) -> f64 {
        self.annual
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Annual {
    Mean,
    Sum,
}

/// `read_monthly_stack`. The Python opens the file once per month; here the twelve bands come
/// from one open (`load_raster_bands_clipped`), which computes the same grid and the same warp.
pub fn read_monthly_stack(
    settings: &Settings,
    path: &str,
    aoi: &Aoi,
    annual: Annual,
) -> Result<Option<ClimateStack>> {
    let bands: Vec<usize> = (1..=WORLDCLIM_MONTHS).collect();
    let slices = load_raster_bands_clipped(settings, path, aoi, Resampling::Nearest, &bands, None)?;

    // (12, rows, cols), nodata as NaN so the all-months check is one operation.
    let data: Vec<Vec<f64>> = slices
        .iter()
        .map(|s| {
            s.values
                .iter()
                .zip(&s.mask)
                .map(|(v, m)| if *m { f64::NAN } else { *v })
                .collect()
        })
        .collect();
    drop(slices);
    let n = data[0].len();

    // A pixel counts only when every month is present.
    let all_valid: Vec<bool> = (0..n)
        .map(|i| !data.iter().any(|d| d[i].is_nan()))
        .collect();
    let n_pixels = all_valid.iter().filter(|v| **v).count();
    if n_pixels == 0 {
        return Ok(None);
    }

    // data.sum(axis=0) adds the months in order per pixel; data.mean(axis=0) divides that by 12.
    let annual_per_pixel: Vec<f64> = (0..n)
        .filter(|&i| all_valid[i])
        .map(|i| {
            let mut s = data[0][i];
            for d in &data[1..] {
                s += d[i];
            }
            if annual == Annual::Mean {
                s / WORLDCLIM_MONTHS as f64
            } else {
                s
            }
        })
        .collect();

    // Chart series over the same pixel set, so chart and sentence agree.
    let monthly = (0..WORLDCLIM_MONTHS)
        .map(|m| {
            let sel: Vec<f64> = (0..n)
                .filter(|&i| all_valid[i])
                .map(|i| data[m][i])
                .collect();
            MonthlyValue {
                month: m as i64 + 1,
                label: MONTH_LABELS[m],
                value: np_sum(&sel) / sel.len() as f64,
            }
        })
        .collect();

    Ok(Some(ClimateStack {
        monthly,
        annual: annual_per_pixel,
        n_pixels,
    }))
}
