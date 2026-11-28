//! `config.py` constants only the benefit components read (F02-P5). The shared ones (raster
//! names, pathway bands, AGB, precipitation, elevation, threat layers) come from `crate::config`.

pub const CO2_PER_C: f64 = 44.0 / 12.0;
pub const INTERVENTION_DURATION_DEFAULT_YEARS: i64 = 30;
/// CARBON_RISK_DEFAULTS: leakage, uncertainty, buffer (percent)
pub const DEFAULT_LEAKAGE: f64 = 15.0;
pub const DEFAULT_UNCERTAINTY: f64 = 10.0;
pub const DEFAULT_BUFFER: f64 = 12.0;

pub const PROTECT_CODE: f64 = 1.0;
pub const MANAGE_CODE: i64 = 2;
pub const RESTORE_CODE: f64 = 3.0;

// 5.2
pub const BASELINE_RATE_MAX_YEARS: i64 = 10;
pub const PROTECT_RISK_COVERAGE_WARN_PCT: f64 = 90.0;
pub const PATHWAY_ECOSYSTEM_PEATLAND: i64 = 3;
pub const PROTECT_ECOSYSTEM_WORDS: [(i64, &str); 3] =
    [(1, "forest"), (2, "mangrove"), (3, "peatland")];
pub const BIOMASS_POOLS: [&str; 2] = ["Aboveground biomass", "Belowground biomass"];

// 5.3 ARR
/// sorted(ARR_SEQ_PAIRS)
pub const ARR_SEQ_PAIRS_SORTED: [(i64, i64); 16] = [
    (4, 1),
    (4, 2),
    (4, 3),
    (6, 1),
    (6, 2),
    (6, 3),
    (7, 1),
    (7, 2),
    (7, 3),
    (12, 1),
    (12, 2),
    (12, 3),
    (14, 2),
    (17, 1),
    (17, 2),
    (17, 3),
];
pub const ARR_YOUNG_END_YEAR: i64 = 20;
pub const ARR_OLD_END_YEAR: i64 = 40;
/// ARR_RATE_DM: non-dryland ecosystems (young, old)
pub fn arr_rate_dm(eco: i64) -> Option<(f64, f64)> {
    match eco {
        2 => Some((12.0, 7.0)),
        3 => Some((5.7, 3.5)),
        _ => None,
    }
}
/// ARR_RATE_DM_DRYLAND: by zone (young, old)
pub fn arr_rate_dm_dryland(zone: i64) -> Option<(f64, f64)> {
    match zone {
        1 => Some((3.4, 2.7)),
        2 => Some((2.4, 2.0)),
        3 => Some((2.4, 1.9)),
        _ => None,
    }
}
pub fn arr_root_to_shoot(eco: i64) -> Option<f64> {
    match eco {
        2 => Some(0.39),
        3 => Some(0.25),
        _ => None,
    }
}
pub fn arr_root_to_shoot_dryland(zone: i64) -> Option<f64> {
    match zone {
        1 => Some(0.21),
        2 => Some(0.44),
        3 => Some(0.32),
        _ => None,
    }
}
pub fn arr_carbon_fraction(eco: i64) -> Option<f64> {
    match eco {
        1 => Some(0.47),
        2 => Some(0.451),
        3 => Some(0.47),
        _ => None,
    }
}
pub const ARR_DRYLAND_ZONES: [(i64, &str); 3] = [
    (1, "humid lowland"),
    (2, "seasonal lowland"),
    (3, "humid montane"),
];
pub const ARR_ZONE_ELEV_MONTANE_M: f64 = 1000.0;
pub const ARR_ZONE_WET_ANNUAL_MM: f64 = 2000.0;
pub const ARR_ZONE_DRY_MONTH_MM: f64 = 100.0;
pub const ARR_ZONE_DRY_SEASON_MONTHS: i64 = 3;
pub const ARR_DRYLAND_DEFAULT_ZONE: i64 = 2;
pub const ARR_BASELINE_MODE: &str = "class";
/// ARR_BASELINE_CLASS_MGHA C4 / C5 / C6
pub const ARR_BASELINE_C4: f64 = 25.0;
pub const ARR_BASELINE_C5: f64 = 5.0;
pub const ARR_BASELINE_C6: f64 = 0.0;
/// ARR_RESTORE_CAT_CSTATE -> ARR_BASELINE_CLASS_MGHA.get(cstate, 0.0)
pub fn arr_baseline_class_mgha(cat: i64) -> f64 {
    match cat {
        4 | 12 => ARR_BASELINE_C4,
        6 | 14 => ARR_BASELINE_C5,
        7 | 17 => ARR_BASELINE_C6,
        _ => 0.0,
    }
}
pub const ARR_STOCKING_PLANTING: f64 = 1.0;
pub const ARR_STOCKING_ANR: f64 = 0.8;
pub const ARR_ANR_PAIRS: [(i64, i64); 5] = [(4, 1), (6, 2), (7, 2), (12, 2), (17, 2)];
pub const ARR_UNCERTAINTY_LOW: f64 = 0.7;
pub const ARR_UNCERTAINTY_HIGH: f64 = 1.2;
pub const ARR_CARBON_DEFERRED_ECO: [i64; 2] = [3, 4];

// 5.7 MSPA
pub const MSPA_2024_RASTER: &str = "fc_2024_mspa_4326_v2.tif";
/// MSPA_CLASS_CODES in dict order
pub const MSPA_CLASS_CODES: [(i64, &str); 22] = [
    (17, "core"),
    (117, "core"),
    (9, "islet"),
    (109, "islet"),
    (5, "perforation"),
    (105, "perforation"),
    (3, "edge"),
    (103, "edge"),
    (65, "loop"),
    (165, "loop"),
    (67, "loop"),
    (167, "loop"),
    (69, "loop"),
    (169, "loop"),
    (33, "bridge"),
    (133, "bridge"),
    (35, "bridge"),
    (135, "bridge"),
    (37, "bridge"),
    (137, "bridge"),
    (1, "branch"),
    (101, "branch"),
];
pub const MSPA_BACKGROUND_CODES: [i64; 4] = [0, 100, 129, 220];
pub const MSPA_CONNECTOR_CLASSES: [&str; 2] = ["bridge", "loop"];
pub const MSPA_CORE_CLASSES: [&str; 1] = ["core"];
pub const MSPA_EDGE_CLASSES: [&str; 1] = ["edge"];
pub const MSPA_HABITAT_CLASSES: [&str; 7] = [
    "core",
    "perforation",
    "edge",
    "branch",
    "loop",
    "bridge",
    "islet",
];
pub const CONNECTIVITY_ECO_WORDS: [(i64, &str); 4] = [
    (1, "forest"),
    (2, "mangrove"),
    (3, "peatland"),
    (4, "savanna"),
];

// 5.9 - 5.13
pub const ECOSYSTEM_CLASS: i64 = 1;
/// ECOSYSTEM_NAMES (dict order)
pub const ECOSYSTEM_NAMES: [(i64, &str); 3] = [(1, "forest"), (2, "mangrove"), (3, "peatland")];
pub const BENEFIT_ECOSYSTEM_CLASSES: [(i64, &str); 3] =
    [(1, "Dryland"), (2, "Mangrove"), (3, "Peatland")];
/// AOH_GDAL_OPTIONS (GDAL_DISABLE_READDIR_ON_OPEN is set process-wide by `init_gdal`)
pub const AOH_GDAL_OPTIONS: [(&str, &str); 2] = [
    ("GDAL_DISABLE_READDIR_ON_OPEN", "EMPTY_DIR"),
    ("CPL_VSIL_CURL_ALLOWED_EXTENSIONS", ".tif"),
];
pub fn iucn_map(status: &str) -> Option<&'static str> {
    match status {
        "CRITICALLY ENDANGERED" | "CR" => Some("CR"),
        "ENDANGERED" | "EN" => Some("EN"),
        "VULNERABLE" | "VU" => Some("VU"),
        _ => None,
    }
}
pub const FOREST_CHANGE_YEARS: f64 = 10.0;
pub const FOREST_CHANGE_RASTER: &str = "threat/forest_change_v3.tif";
pub const GAIN_CODE: f64 = 4.0;
pub const DEGRADATION_CODE: f64 = 2.0;
pub const POP_BASE_YEAR: i64 = 2025;
pub const FUTUREPOP_RASTER: &str = "FuturePop_v3.tif";
/// FUTUREPOP_YEARS = range(2025, 2101, 5)
pub fn futurepop_years() -> Vec<i64> {
    (2025..2101).step_by(5).collect()
}

pub fn lookup<K: PartialEq + Copy, V: Copy>(table: &[(K, V)], key: K) -> Option<V> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}
