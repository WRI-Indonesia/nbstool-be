//! The constants of `v3/config.py` the ported components read, copied verbatim.

pub const V3_BUCKET_SETTING: &str = "V3_BUCKET";
pub const AOH_RASTER_ROOT: &str = "habitat_area";
pub const AOH_RASTER_ROOT_SETTING: &str = "AOH_RASTER_ROOT";

/// REFERENCE_CRS = "ESRI:54034". rasterio's `CRS.from_user_input("ESRI:54034").to_wkt()`, verbatim:
/// the destination SRS of every warp and the `DST_SRS` of `calculate_default_transform`.
pub const REFERENCE_CRS_WKT: &str = r#"PROJCS["World_Cylindrical_Equal_Area",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0],UNIT["Degree",0.0174532925199433]],PROJECTION["Cylindrical_Equal_Area"],PARAMETER["standard_parallel_1",0],PARAMETER["central_meridian",0],PARAMETER["false_easting",0],PARAMETER["false_northing",0],UNIT["metre",1,AUTHORITY["EPSG","9001"]],AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["ESRI","54034"]]"#;

pub const PATHWAY_RASTER: &str = "sea_nbs_pathway_v3.tif";
pub const PATHWAY_ECOSYSTEM_BAND: usize = 2;
/// 1 dryland forest & 4 savanna both -> 1 Dryland
pub const PATHWAY_ECO_TO_AXIS3: [(i64, i64); 4] = [(1, 1), (4, 1), (2, 2), (3, 3)];

pub const FC2014_RASTER: &str = "SEA_FC2014.tif";
pub const FC2014_FOREST_CODES: [f64; 1] = [1.0];
pub const FC2024_RASTER: &str = "SEA_FC2024.tif";
pub const FC2024_FOREST_CODES: [f64; 1] = [1.0];

pub const LC2024_RASTER: &str = "SEA_LC2024.tif";

pub const RISK_RASTERS: [(&str, &str); 5] = [
    ("cyclone", "risk_cyclone_v3.tif"),
    ("drought", "risk_drought_v3.tif"),
    ("fire", "risk_fire_v3.tif"),
    ("flood", "risk_flood_v3.tif"),
    ("landslide", "risk_landslide_v3.tif"),
];

pub const ECOSYSTEM_CLASSES: [(i64, &str); 3] = [(1, "Dryland"), (2, "Mangrove"), (3, "Peatland")];

pub const LC2024_CLASSES: [(i64, &str); 20] = [
    (1, "Flooded forest"),
    (2, "Rubber"),
    (3, "Palm"),
    (4, "Plantation"),
    (5, "Crop plantation"),
    (6, "Mangrove"),
    (7, "Deciduous Forest"),
    (8, "Evergreen Forest"),
    (9, "Shrubland"),
    (10, "Mixed forest"),
    (11, "Snow"),
    (12, "Water"),
    (13, "Aquaculture"),
    (14, "Rice"),
    (15, "Building"),
    (16, "Cropland"),
    (17, "Grassland"),
    (18, "Wetland"),
    (19, "Bareland"),
    (20, "Other land"),
];
pub const LC_TOP_N: usize = 6;

pub const RISK_LEVELS: [(i64, &str); 4] =
    [(1, "Very Low"), (2, "Low"), (3, "Moderate"), (4, "High")];

pub const DEFOR_YEAR_START: i64 = 2014;
pub const DEFOR_YEAR_END: i64 = 2024;
pub const DEFOR_PERIOD_YEARS: i64 = DEFOR_YEAR_END - DEFOR_YEAR_START;
pub const RISK_PRESENCE_PCT: f64 = 20.0;

pub fn ecosystem_key(code: &str) -> Option<&'static str> {
    match code {
        "1" => Some("ecosystem_dryland"),
        "2" => Some("ecosystem_mangrove"),
        "3" => Some("ecosystem_peatland"),
        "other" => Some("ecosystem_other"),
        _ => None,
    }
}

pub fn ecosystem_color(code: &str) -> Option<&'static str> {
    match code {
        "1" => Some("#2c6639"),
        "2" => Some("#f1fc3e"),
        "3" => Some("#abc963"),
        "other" => Some("#dadada"),
        _ => None,
    }
}

pub fn risk_key(code: i64) -> Option<&'static str> {
    match code {
        1 => Some("very_low"),
        2 => Some("low"),
        3 => Some("moderate"),
        4 => Some("high"),
        5 => Some("very_high"),
        _ => None,
    }
}
pub const RISK_NO_DATA_KEY: &str = "no_risk";

pub fn land_cover_key(code: i64) -> Option<&'static str> {
    const KEYS: [&str; 21] = [
        "land_cover_no_data",
        "land_cover_flooded_forest",
        "land_cover_rubber",
        "land_cover_palm",
        "land_cover_plantation",
        "land_cover_crop_plantation",
        "land_cover_mangrove",
        "land_cover_deciduous",
        "land_cover_evergreen",
        "land_cover_shrubland",
        "land_cover_mixed_forest",
        "land_cover_snow",
        "land_cover_water",
        "land_cover_aquaculture",
        "land_cover_rice",
        "land_cover_building",
        "land_cover_cropland",
        "land_cover_grassland",
        "land_cover_wetland",
        "land_cover_bareland",
        "land_cover_other_land",
    ];
    usize::try_from(code)
        .ok()
        .and_then(|i| KEYS.get(i).copied())
}

pub fn land_cover_color(code: i64) -> Option<&'static str> {
    const COLORS: [&str; 21] = [
        "#dadada", "#1f6f4a", "#8c6d3f", "#c49a2f", "#9c7a4a", "#c9a86a", "#0f9b8e", "#7fbf5f",
        "#2c6639", "#bcd35f", "#4f8f3f", "#f2f7fb", "#3a7bd5", "#6fb3d2", "#e6d24a", "#b03a2e",
        "#e8a33d", "#d4d84a", "#57a0a8", "#cdbfa3", "#9aa0a6",
    ];
    usize::try_from(code)
        .ok()
        .and_then(|i| COLORS.get(i).copied())
}

// ---- F02-P4 Pathway: the same raster, its other two bands ----
pub const PATHWAY_BAND: usize = 1;
pub const PATHWAY_CATCODE_BAND: usize = 3;
pub const PATHWAY_CODES: [(i64, &str); 5] = [
    (0, "No data"),
    (1, "Protect"),
    (2, "Manage"),
    (3, "Restore"),
    (4, "Ineligible"),
];
pub const PATHWAY_ELIGIBLE_CODES: [i64; 3] = [1, 2, 3];
pub const PATHWAY_ECOSYSTEM_CODES: [(i64, &str); 5] = [
    (0, "None"),
    (1, "Dryland forest"),
    (2, "Mangrove"),
    (3, "Peatland"),
    (4, "Savanna"),
];
pub const PATHWAY_CATCODE_LABELS: [(i64, &str); 18] = [
    (0, "Mask"),
    (1, "Cat 1"),
    (2, "Cat 2"),
    (3, "Cat 3A"),
    (4, "Cat 3B"),
    (5, "Cat 4A"),
    (6, "Cat 4B"),
    (7, "Cat 5"),
    (8, "Cat 6"),
    (9, "Cat 7"),
    (10, "Cat 8A"),
    (11, "Cat 8B"),
    (12, "Cat 8C"),
    (13, "Cat 9A"),
    (14, "Cat 9B"),
    (15, "Cat 9C"),
    (16, "Cat 9D"),
    (17, "Cat 10"),
];
pub const PATHWAY_UNCLASSIFIED_WARN_PCT: f64 = 20.0;

// ---- 3.5 Fire susceptibility: aliases of the 1.7 fire layer and legend ----
pub const FIRE_HAZARD_RASTER: &str = "risk_fire_v3.tif";
pub const FIRE_LEVELS: [(i64, &str); 4] = RISK_LEVELS;

// ---- 3.1 Current carbon storage ----
pub const AGB_RASTER: &str = "agbd_v3.tif";
pub const ROOT_TO_SHOOT_RATIO: f64 = 0.28;
pub const CARBON_FRACTION: f64 = 0.47;
pub const CARBON_COVERAGE_WARN_PCT: f64 = 90.0;

// ---- Endangered tree species richness ----
pub const TREE_SPECIES_RASTER: &str = "tree_species_v3.tif";

// ---- 3.2 Soil organic carbon: the 0-30 cm SoilGrids intervals ----
pub const SOIL_CARBON_0_30_RASTERS: [&str; 3] = [
    "soil_carbon_stock1_t_ha_v3.tif",
    "soil_carbon_stock2_t_ha_v3.tif",
    "soil_carbon_stock3_t_ha_v3.tif",
];
pub const SOIL_CARBON_DEPTH_CM: i64 = 30;

// ---- 3.3 / 3.4 WorldClim monthly stacks ----
pub const WORLDCLIM_VERSION: &str = "v3 (verify source)";
pub const WORLDCLIM_PERIOD: &str = "verify";
pub const WORLDCLIM_RESOLUTION: &str = "verify";
pub const WORLDCLIM_MONTHS: usize = 12;
pub const WORLDCLIM_TAVG_RASTER: &str = "temperature_v3.tif";
pub const WORLDCLIM_PREC_RASTER: &str = "precipitation_v3.tif";
pub const MONTH_LABELS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const CLIMATE_MIN_PIXELS: usize = 5;
pub const DRY_MONTH_MM: f64 = 100.0;

// ---- 1.4 Terrain ----
pub const ELEVATION_RASTER: &str = "elevation_54034_v3.tif";
pub const ELEVATION_BREAKS: [f64; 3] = [500.0, 1000.0, 2000.0];
pub const SLOPE_BREAKS: [f64; 4] = [8.0, 15.0, 25.0, 40.0];
pub const SLOPE_CLASSES: [(i64, &str); 5] = [
    (1, "Flat"),
    (2, "Gently sloping"),
    (3, "Moderately steep"),
    (4, "Steep"),
    (5, "Very steep"),
];
pub const ELEVATION_CLASSES: [(i64, &str); 4] = [
    (1, "Lowland"),
    (2, "Submontane / hill"),
    (3, "Montane"),
    (4, "Upper montane"),
];

pub fn slope_key(code: i64) -> Option<&'static str> {
    [
        "slope_flat",
        "slope_gently_sloping",
        "slope_moderately_steep",
        "slope_steep",
        "slope_very_steep",
    ]
    .get(usize::try_from(code - 1).ok()?)
    .copied()
}

pub fn slope_color(code: i64) -> Option<&'static str> {
    ["#1A9850", "#A6D96A", "#FEE08B", "#FDAE61", "#D73027"]
        .get(usize::try_from(code - 1).ok()?)
        .copied()
}

pub fn elevation_key(code: i64) -> Option<&'static str> {
    [
        "elevation_lowland",
        "elevation_submontane_hill",
        "elevation_montane",
        "elevation_upper_montane",
    ]
    .get(usize::try_from(code - 1).ok()?)
    .copied()
}

// ---- 6.1 People demography ----
pub const POP_FEMALE_RASTER: &str = "female_pop_v3.tif";
pub const POP_MALE_RASTER: &str = "male_pop_v3.tif";
/// The 20 source bands collapse to these 14 display ranges (1-based band positions).
pub const PEOPLE_AGE_GROUPS: [(&str, &[usize]); 14] = [
    ("0-4", &[1, 2]),
    ("5-9", &[3]),
    ("10-14", &[4]),
    ("15-19", &[5]),
    ("20-24", &[6]),
    ("25-29", &[7]),
    ("30-34", &[8]),
    ("35-39", &[9]),
    ("40-44", &[10]),
    ("45-49", &[11]),
    ("50-54", &[12]),
    ("55-59", &[13]),
    ("60-64", &[14]),
    ("65+", &[15, 16, 17, 18, 19, 20]),
];

// ---- 6.2 Vulnerability ----
pub const PEOPLE_VULNERABILITY_RASTERS: [(&str, &str); 4] = [
    ("physical", "vulnerability_physical_v3.tif"),
    ("environmental", "vulnerability_natural_v3.tif"),
    ("economic", "vulnerability_economic_v3.tif"),
    ("social", "vulnerability_social_v3.tif"),
];
pub const PEOPLE_VULNERABILITY_LEVELS: [(i64, &str); 5] = [
    (1, "Very Low"),
    (2, "Low"),
    (3, "Moderate"),
    (4, "High"),
    (5, "Very High"),
];

// ---- 1.6 Deforestation risk ----
pub const PROB_RASTER: &str = "def_risk_v3.tif";
pub const PROB_SCALE_MAX: f64 = 65535.0;
pub const RISK_HIGHER_PCTL: f64 = 60.0;
pub const RISK_LOWER_PCTL: f64 = 40.0;
pub const NATIONAL_FOREST_RISK_CSV: &str = "national_forest_risk_reference.csv";

// ---- F02-P3 Threat ----
pub const THREAT_ECOSYSTEM: &str = "threat/ecosystem_v3.tif";
pub const THREAT_DISTURBANCE: &str = "threat/forest_disturbance_v3.tif";
pub const THREAT_ECOSYSTEM_CLASSES: [(i64, &str); 4] = [
    (0, "Other"),
    (1, "Dryland forest"),
    (2, "Mangrove"),
    (3, "Peatland"),
];
pub const THREAT_HISTORICAL: &str = "threat/historical_deforestation_v3.tif";
pub const THREAT_FOREST_2024: &str = "threat/forest_2024_v3.tif";
pub const THREAT_FOREST_GAIN: &str = "threat/forest_gain_v3.tif";
pub const THREAT_DRIVERS: &str = "threat/drivers_disturbance_v3.tif";
pub const THREAT_FLOOD_RISK: &str = "threat/risk_flood_v3.tif";
pub const THREAT_LANDSLIDE_RISK: &str = "threat/risk_landslide_v3.tif";
pub const THREAT_STORM_RISK: &str = "threat/risk_storm_v3.tif";
pub const THREAT_FIRE_RISK: &str = "threat/risk_fire_v3.tif";
pub const THREAT_PEAT_CANALS_DENSITY: &str = "threat/peat_canals_density_v3.tif";
pub const THREAT_PEAT_CANAL: &str = "threat/peat_canal.tif";
pub const THREAT_LAND_DEGRADATION: &str = "land_deg_54034_v3.tif";
pub const THREAT_FOREST_2024_54034: &str = "FC_2024_SEA_54034.tif";
pub const THREAT_DRYLAND: f64 = 1.0;
pub const THREAT_MANGROVE: f64 = 2.0;
pub const THREAT_PEATLAND: f64 = 3.0;
pub const THREAT_REMAINING_FOREST: f64 = 1.0;
pub const THREAT_FOREST_LOSS: f64 = 2.0;
pub const THREAT_CURRENT_FOREST: f64 = 1.0;
pub const THREAT_FOREST_GAIN_VALUE: f64 = 1.0;
pub const THREAT_DISTURBANCE_THRESHOLD: f64 = 0.0;
/// `THREAT_FOREST_DRIVER_CLASSES`, in dict order.
pub const THREAT_FOREST_DRIVER_CLASSES: [(i64, &str); 8] = [
    (1, "Small-scale agriculture"),
    (2, "Small-scale agriculture (fire)"),
    (3, "Large-scale agriculture"),
    (4, "Large-scale agriculture (fire)"),
    (5, "Road development"),
    (6, "Selective logging"),
    (7, "Mining"),
    (8, "Non-productive conversion"),
];
/// `THREAT_NATURAL_DRIVERS`, in dict order: (name, raster, values).
pub const THREAT_NATURAL_DRIVERS: [(&str, &str, &[f64]); 4] = [
    ("Flooding", THREAT_FLOOD_RISK, &[4.0]),
    ("Forest fire", THREAT_DRIVERS, &[10.0]),
    ("Landslide", THREAT_LANDSLIDE_RISK, &[4.0]),
    ("Extreme climate event", THREAT_STORM_RISK, &[4.0]),
];
pub const THREAT_COMMODITY_CLASSES: [f64; 4] = [1.0, 2.0, 3.0, 4.0];
pub const THREAT_SETTLEMENT_CLASS: f64 = 5.0;
pub const THREAT_STORM_RISK_CLASSES: [f64; 2] = [4.0, 5.0];

// ---- 3.6 Soil classification ----
pub const SOIL_CLASS_RASTER: &str = "soil_groups_v3.tif";
pub const SOIL_CLASS_TABLE: &str = "soil_class_lookup.csv";
pub const WRB_MODE: &str = "categorical";
pub const WRB_MIN_PROBABILITY_PCT: f64 = 1.0;
pub const WRB_DISPLAY_TOP_N: i64 = 5;

// ---- Peat depth, 7.2 burned area ----
pub const PEAT_DEPTH_RASTER: &str = "peat_depth_54034_v3.tif";
pub const GABAM_YEARS: std::ops::Range<i64> = 2014..2024;
pub const GABAM_BURNED_VALUE: f64 = 1.0;
pub fn gabam_raster(year: i64) -> String {
    format!("gabam/GABAM_{year}.tif")
}

// ---- 2.3 Habitat area (AoH) ----
pub const AOH_INVENTORY: &str = "habitat_area/species_iucn_v3.geoparquet";
pub const AOH_MAX_WORKERS: usize = 16;
pub const AOH_MAX_WORKERS_SETTING: &str = "AOH_MAX_WORKERS";
pub const AOH_TARGET_DN: f64 = 1.0;
pub const OVERLAP_THRESHOLD: f64 = 90.0;
/// `AOH_TAXON_FIELDS`, in dict order.
pub const AOH_TAXON_FIELDS: [(&str, &str); 4] = [
    ("Mammal", "mammal_number_of_species"),
    ("Bird", "bird_number_of_species"),
    ("Amphibian", "amphibian_number_of_species"),
    ("Reptile", "reptile_number_of_species"),
];
