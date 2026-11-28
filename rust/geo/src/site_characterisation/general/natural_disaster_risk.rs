//! Component 1.7 Natural Disaster Risks (`general/natural_disaster_risk.py`).
//!
//! Five pre-classified risk rasters (cyclone, drought, fire, flood, landslide), values 1..4 with
//! 0 as nodata. Representative level is conservative: the highest class covering at least
//! RISK_PRESENCE_PCT (20%) of the AOI's valid area for that layer. A risk with no coverage is left
//! out of `risk_cards`; team ruling 2026-08-27: no coverage reads as "No risk".

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, ClassShare, Code, Resampling, load_raster_clipped, tabulate_classes};
use crate::config::{RISK_LEVELS, RISK_NO_DATA_KEY, RISK_PRESENCE_PCT, RISK_RASTERS, risk_key};
use crate::py::fsum_py;
use crate::settings::Settings;

/// FE_RISK_FIELDS, in the Python dict's order.
const FE_RISK_FIELDS: [(&str, &str); 5] = [
    ("flood", "flood_risk_dict"),
    ("landslide", "landslide_risk_dict"),
    ("cyclone", "typhoon_risk_dict"),
    ("drought", "drought_risk_dict"),
    ("fire", "fire_risk_dict"),
];

struct RiskCard {
    risk: &'static str,
    level_code: Option<i64>,
    level_label: String,
    distribution: Vec<ClassShare>,
}

impl RiskCard {
    fn to_json(&self) -> Value {
        json!({
            "risk": self.risk,
            "level_code": self.level_code,
            "level_label": self.level_label,
            "distribution": self.distribution.iter().map(ClassShare::to_json).collect::<Vec<_>>(),
        })
    }
}

fn int_code(row: &ClassShare) -> i64 {
    match row.code {
        Code::Int(c) => c,
        Code::Str(_) => 0,
    }
}

/// `_representative_level`: highest risk class covering at least RISK_PRESENCE_PCT.
fn representative_level(rows: &[ClassShare]) -> Option<i64> {
    rows.iter()
        .filter(|r| r.pct >= RISK_PRESENCE_PCT)
        .map(int_code)
        .max()
}

fn level_label(code: i64) -> &'static str {
    RISK_LEVELS
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("", |(_, l)| l)
}

/// `analyze_natural_risk`
pub fn analyze_natural_risk(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let mut cards: Vec<RiskCard> = Vec::new();
    for &(risk, path) in &RISK_RASTERS {
        let raster = load_raster_clipped(settings, path, aoi, Resampling::Nearest, 1, None)?;
        if raster.valid_area_ha() <= 0.0 {
            // No coverage over this AOI: the risk is not "present", so it is left out of the list.
            continue;
        }
        let rows = tabulate_classes(&raster, &RISK_LEVELS, raster.valid_area_ha());
        let code = representative_level(&rows);
        cards.push(RiskCard {
            risk,
            level_code: code,
            // `RISK_LEVELS[code] if code else "No data"`
            level_label: match code {
                Some(c) if c != 0 => level_label(c).to_string(),
                _ => "No data".to_string(),
            },
            distribution: rows,
        });
    }

    // Any present risk is listed, ordered by level (highest first), then name for ties.
    let mut present: Vec<&RiskCard> = cards
        .iter()
        .filter(|c| c.level_code.unwrap_or(0) != 0)
        .collect();
    present.sort_by(|a, b| b.level_code.cmp(&a.level_code).then(a.risk.cmp(b.risk)));

    let narrative = if present.is_empty() {
        "The selected area is not exposed to any natural disaster risk."
    } else {
        "The selected area is susceptible to several natural disaster risks, including:"
    };

    let leveled = |risk: &str| {
        cards
            .iter()
            .find(|c| c.risk == risk && c.level_code.unwrap_or(0) != 0)
    };

    let mut values = serde_json::Map::new();
    for c in &present {
        values.insert(c.risk.to_string(), json!(c.level_code));
    }
    let results = json!({
        "narrative": narrative,
        "tables": {"risk_cards": present.iter().map(|c| c.to_json()).collect::<Vec<_>>()},
        "values": values,
        "flags": [],
    });

    let mut view = serde_json::Map::new();
    for (risk, field) in FE_RISK_FIELDS {
        let entry = match leveled(risk) {
            Some(c) => json!({"key": c.level_code.and_then(risk_key), "fallback": c.level_label}),
            None => json!({"key": RISK_NO_DATA_KEY, "fallback": "No risk"}),
        };
        view.insert(field.to_string(), entry);
    }

    // HOW MUCH OF THE PROJECT AREA OVERLAPS FIRE RISK: the area carrying ANY fire class, as a
    // share of the PROJECT area; each class row keeps the distribution's own share.
    let fire = leveled("fire");
    let fire_ha = match fire {
        Some(f) => fsum_py(f.distribution.iter().map(|r| r.area_ha)),
        None => 0.0,
    };
    view.insert("fire_risk_area_ha".into(), json!(fire_ha));
    view.insert(
        "fire_risk_percentage".into(),
        json!(if aoi.area_ha > 0.0 {
            100.0f64.min(fire_ha / aoi.area_ha * 100.0)
        } else {
            0.0
        }),
    );
    view.insert(
        "fire_risk_classes".into(),
        json!(fire.map_or(vec![], |f| {
            f.distribution
                .iter()
                .map(|row| {
                    json!({
                        "id": row.code.as_string(),
                        "dict": {"key": risk_key(int_code(row)), "fallback": row.label},
                        "area": row.area_ha,
                        "percentage": row.pct,
                    })
                })
                .collect::<Vec<_>>()
        })),
    );

    Ok((results, Value::Object(view)))
}
