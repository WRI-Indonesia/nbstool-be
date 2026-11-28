//! Component 1.4 Terrain (`general/terrain_slope_elevation.py`).
//!
//! Only elevation is read (continuous metres, `elevation_54034_v3.tif`, nearest). Slope is
//! DERIVED from it (`slope_percent_from_dem`), and both are binned into class codes
//! (`classify_continuous`). Shares are of the valid area; min and max are in metres from the
//! continuous DEM; the elevation label keys come from tbl_list_mapping_key, falling back to
//! ELEVATION_KEYS.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{
    Aoi, ClassShare, Code, Resampling, classify_continuous, dominant, load_raster_clipped,
    not_applicable, slope_percent_from_dem, tabulate_classes,
};
use crate::config::{
    ELEVATION_BREAKS, ELEVATION_CLASSES, ELEVATION_RASTER, SLOPE_BREAKS, SLOPE_CLASSES,
    elevation_key, slope_color, slope_key,
};
use crate::db::MappingKeys;
use crate::py::{round_int, round_ndigits, sentences};
use crate::settings::Settings;

fn code_of(row: &ClassShare) -> i64 {
    match row.code {
        Code::Int(c) => c,
        Code::Str(_) => 0,
    }
}

/// `_elevation_dict`: one frontend label object, or None when there is no class to name.
fn elevation_dict(keys: &MappingKeys, code: Option<i64>) -> Value {
    let Some(code) = code else { return Value::Null };
    let label = ELEVATION_CLASSES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("", |(_, l)| l);
    let key = keys.key(
        (
            "site_characteristic",
            Some("informasi_umum"),
            Some("elevation"),
            None,
            None,
        ),
        Some(label),
        true,
    );
    json!({"key": key.or_else(|| elevation_key(code).map(str::to_string)), "fallback": label})
}

/// `analyze_terrain`
pub fn analyze_terrain(
    settings: &Settings,
    keys: &MappingKeys,
    aoi: &Aoi,
) -> Result<(Value, Value)> {
    let elev_c = load_raster_clipped(
        settings,
        ELEVATION_RASTER,
        aoi,
        Resampling::Nearest,
        1,
        None,
    )?;
    if elev_c.valid_area_ha() <= 0.0 {
        let (narrative, flags, missing) =
            not_applicable("No elevation data is available for this project area.");
        let results = json!({"narrative": narrative, "tables": {"slope": [], "elevation": []},
                             "values": {}, "flags": flags, "missing": missing});
        let view = json!({"slopes": [], "elevations": [], "average_slope_percentage": null,
                          "min_elevation": null, "max_elevation": null,
                          "predominant_elevation_dict": null});
        return Ok((results, view));
    }

    let slope_c = slope_percent_from_dem(&elev_c, aoi)?;
    let slope = classify_continuous(&slope_c, &SLOPE_BREAKS);
    let elev = classify_continuous(&elev_c, &ELEVATION_BREAKS);

    // Mean of the continuous slope grid, over the same valid pixels the class shares use.
    let average_slope = slope_c.ma_mean().unwrap_or(f64::NAN);
    drop(slope_c);

    let slope_rows = tabulate_classes(&slope, &SLOPE_CLASSES, slope.valid_area_ha());
    let elev_rows = tabulate_classes(&elev, &ELEVATION_CLASSES, elev.valid_area_ha());
    drop((slope, elev));
    let dom_slope = dominant(&slope_rows);
    let dom_elev = dominant(&elev_rows);

    // Min and max in actual metres, from the continuous elevation raster.
    let vmin = round_int(elev_c.ma_min().unwrap_or(f64::NAN));
    let vmax = round_int(elev_c.ma_max().unwrap_or(f64::NAN));
    drop(elev_c);

    let elev_clause = match dom_elev {
        None => String::new(),
        Some(d) if vmin == vmax => {
            format!(
                "Elevation is {vmin} m above sea level (asl), predominantly {}.",
                d.label
            )
        }
        Some(d) => format!(
            "Elevation ranges from {vmin} to {vmax} m above sea level (asl), predominantly {}.",
            d.label
        ),
    };
    let slope_clause = dom_slope.map_or(String::new(), |d| {
        format!("Slopes are predominantly {}.", d.label)
    });

    let results = json!({
        "narrative": sentences(&[&elev_clause, &slope_clause]),
        "tables": {
            "slope": slope_rows.iter().map(ClassShare::to_json).collect::<Vec<_>>(),
            "elevation": elev_rows.iter().map(ClassShare::to_json).collect::<Vec<_>>(),
        },
        "values": {
            "dominant_slope": dom_slope.map(|d| d.code.to_json()),
            "dominant_elevation": dom_elev.map(|d| d.code.to_json()),
            "average_slope_pct": average_slope,
        },
        "flags": [],
    });

    let view = json!({
        "slopes": slope_rows.iter().map(|row| json!({
            "id": row.code.as_string(),
            "dict": {"key": slope_key(code_of(row)), "fallback": row.label},
            "area": row.area_ha,
            "percentage": row.pct,
            "color": slope_color(code_of(row)),
        })).collect::<Vec<_>>(),
        "elevations": elev_rows.iter().map(|row| json!({
            "id": row.code.as_string(),
            "dict": elevation_dict(keys, Some(code_of(row))),
            "area": row.area_ha,
            "percentage": row.pct,
        })).collect::<Vec<_>>(),
        "average_slope_percentage": round_ndigits(average_slope, 2),
        "min_elevation": vmin,
        "max_elevation": vmax,
        "predominant_elevation_dict": elevation_dict(keys, dom_elev.map(code_of)),
    });
    Ok((results, view))
}
