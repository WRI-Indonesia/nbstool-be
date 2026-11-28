//! Component 6.1 People Demography (`people/people_demography.py`).
//!
//! WorldPop, two 20-band rasters (`female_pop_v3.tif`, `male_pop_v3.tif`, bands `f_00_2025` ..
//! `f_90_2025`). POPULATION IS A COUNT, so it is read with `sum` resampling and never multiplied
//! by cell area. Total population is male + female; the male raster is the reference grid, the
//! female stack is forced onto it with `like`. Values are rounded to ten decimals (`round10`).

use gdal::Metadata;
use serde_json::{Value, json};

use crate::common::{
    Aoi, RasterSlice, Resampling, load_raster_bands_clipped, not_applicable, open_layer,
};
use crate::config::{PEOPLE_AGE_GROUPS, POP_FEMALE_RASTER, POP_MALE_RASTER};
use crate::py::{fmt_grouped, fsum_py, round_ndigits, safe_pct};
use crate::settings::Settings;
use crate::{Error, Result};

const AGE_CODES: [&str; 20] = [
    "00", "01", "05", "10", "15", "20", "25", "30", "35", "40", "45", "50", "55", "60", "65", "70",
    "75", "80", "85", "90",
];

fn round10(x: f64) -> f64 {
    round_ndigits(x, 10)
}

/// `_check_population_band_contract`
fn check_population_band_contract(settings: &Settings, path: &str, prefix: &str) -> Result<()> {
    let src = open_layer(settings, path)?;
    let bad = || {
        Error::new(
            "ValueError",
            format!("{path} has incorrect population bands."),
        )
    };
    if src.raster_count() != 20 {
        return Err(bad());
    }
    for (i, age) in AGE_CODES.iter().enumerate() {
        let band = src.rasterband(i + 1).map_err(crate::ffi::gdal_err)?;
        if band.description().unwrap_or_default() != format!("{prefix}_{age}_2025") {
            return Err(bad());
        }
    }
    Ok(())
}

/// `_load_population_bands`: the twenty bands through one open, `sum` resampling.
fn load_population_bands(
    settings: &Settings,
    path: &str,
    aoi: &Aoi,
    like: Option<&RasterSlice>,
) -> Result<Vec<RasterSlice>> {
    let bands: Vec<usize> = (1..=20).collect();
    load_raster_bands_clipped(
        settings,
        path,
        aoi,
        Resampling::Sum,
        &bands,
        like.map(RasterSlice::grid),
    )
}

/// `_band_total`
fn band_total(sums: &[f64], positions: &[usize]) -> f64 {
    fsum_py(positions.iter().map(|i| sums[i - 1]))
}

struct AgeGroup {
    age_group: &'static str,
    male_population: f64,
    female_population: f64,
    male_percent: f64,
    female_percent: f64,
}

impl AgeGroup {
    fn to_json(&self) -> Value {
        json!({"age_group": self.age_group, "male_population": self.male_population,
               "female_population": self.female_population, "male_percent": self.male_percent,
               "female_percent": self.female_percent})
    }
}

/// `analyze_people_demography`
pub fn analyze_people_demography(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    check_population_band_contract(settings, POP_MALE_RASTER, "m")?;
    check_population_band_contract(settings, POP_FEMALE_RASTER, "f")?;

    // Male raster becomes the reference grid. Each band is reduced to `filled(0).sum()` as soon
    // as it is read; the arrays themselves are not needed afterwards.
    let male_bands = load_population_bands(settings, POP_MALE_RASTER, aoi, None)?;
    let male_sums: Vec<f64> = male_bands.iter().map(RasterSlice::filled_sum).collect();
    let reference = male_bands.into_iter().next();
    let female_sums: Vec<f64> =
        load_population_bands(settings, POP_FEMALE_RASTER, aoi, reference.as_ref())?
            .iter()
            .map(RasterSlice::filled_sum)
            .collect();
    drop(reference);

    // Total population = all male + all female age bands
    let male_population = fsum_py(male_sums.iter().copied());
    let female_population = fsum_py(female_sums.iter().copied());
    let total_population = male_population + female_population;

    if total_population <= 0.0 {
        let (narrative, flags, missing) =
            not_applicable("No population data is available for this project area.");
        let results = json!({"narrative": narrative, "tables": {"age_groups": []}, "values": {},
                             "flags": flags, "missing": missing});
        return Ok((results, empty_view()));
    }

    let age_rows: Vec<AgeGroup> = PEOPLE_AGE_GROUPS
        .iter()
        .map(|(label, positions)| {
            let male = band_total(&male_sums, positions);
            let female = band_total(&female_sums, positions);
            AgeGroup {
                age_group: label,
                male_population: round10(male),
                female_population: round10(female),
                male_percent: round10(safe_pct(male, total_population)),
                female_percent: round10(safe_pct(female, total_population)),
            }
        })
        .collect();

    let male_pct = safe_pct(male_population, total_population);
    let female_pct = safe_pct(female_population, total_population);
    let narrative = format!(
        "Based on gridded population data, the selected area has an estimated total population of \
         {}, consisting of {} males and {} females.",
        fmt_grouped(total_population, 0),
        fmt_grouped(male_population, 0),
        fmt_grouped(female_population, 0)
    );

    let results = json!({
        "narrative": narrative,
        "tables": {"age_groups": age_rows.iter().map(AgeGroup::to_json).collect::<Vec<_>>()},
        "values": {
            "total_population": round10(total_population),
            "male_population": round10(male_population),
            "female_population": round10(female_population),
            "male_pct": round10(male_pct),
            "female_pct": round10(female_pct),
            "chart_series": "age_groups",
            "chart_unit": "people",
            "chart_axis_label": "Estimated population",
        },
        "flags": [],
    });
    let view = view(
        round10(total_population),
        round10(male_population),
        round10(female_population),
        round10(male_pct),
        round10(female_pct),
        &age_rows,
    );
    Ok((results, view))
}

/// `_empty_view`
fn empty_view() -> Value {
    json!({"social_demography": {
        "total_population": 0, "male_population": 0, "female_population": 0,
        "male_population_percentage": 0, "female_population_percentage": 0,
        "age_distributions_male": [], "age_distributions_female": [], "age_groups": [],
    }})
}

/// `_view`
fn view(
    total: f64,
    male: f64,
    female: f64,
    male_pct: f64,
    female_pct: f64,
    rows: &[AgeGroup],
) -> Value {
    json!({"social_demography": {
        "total_population": total,
        "male_population": male,
        "female_population": female,
        "male_population_percentage": male_pct,
        "female_population_percentage": female_pct,
        "age_distributions_male": rows.iter().map(|r| json!({
            "id": r.age_group, "population": r.male_population, "age_group_id": r.age_group})).collect::<Vec<_>>(),
        "age_distributions_female": rows.iter().map(|r| json!({
            "id": r.age_group, "population": r.female_population, "age_group_id": r.age_group})).collect::<Vec<_>>(),
        "age_groups": rows.iter().map(|r| json!({"id": r.age_group, "color": null})).collect::<Vec<_>>(),
    }})
}
