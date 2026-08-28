"""
config.py - v3 layer registry and locked constants for the site characterisation modules.

Port of the notebook `config.py` (nbstool_v3), trimmed to what F02-P2 General (components 1.1
to 1.8) needs and repointed from the local D:\\NBSTOOLV3 files to the v3 objects in the
assets-geo bucket. Rasters are read over /vsicurl by rasterio, the same way the v2 baseline
layers are read in utils/geos/current_condition.py.

Layer constants name the FILE, not the full path. Where the files live is per environment, so the
root comes from the `V3_BUCKET` row of tbl_master_settings and `settings.layer_path` joins the
two when `load_raster_clipped` opens the raster. That keeps this module a plain registry with no
database of its own, and it means switching between the bucket and a local copy of the layers is
a settings edit rather than a code change.

Two inputs of the notebook have no v3 raster and come from the GIS database instead:
administrative boundaries (`sea.adm_boundaries`) and protected areas (`sea.wdpa_wdoecm`). See
v3/db.py. `administrative_boundaries_v3.tif` exists in the bucket but is a 0/1 coverage mask,
not an admin code raster, so it cannot answer 1.2.
"""

# Name of the tbl_master_settings row holding the raster root. Its value is either the bucket
# prefix (https://storage.googleapis.com/assets-geo/v3) or a directory of downloaded layers.
V3_BUCKET_SETTING = "V3_BUCKET"

# ============================ REFERENCE CRS ============================
# The AOI is accepted in any CRS and reprojected to this equal-area CRS for all area work.
REFERENCE_CRS = "ESRI:54034"   # World Cylindrical Equal Area. Locked by the team.

# ============================ PRE-DEFINED LAYERS ============================
# Ecosystem type (1.1): derived from the pathway raster's ecosystem band (band 2), not a separate
# raster. Dryland forest and savanna are merged into one "Dryland" class per the team.
# pathway band-2 code -> Axis 3 class (1 Dryland, 2 Mangrove, 3 Peatland); band-2 0 (none) -> Other.
PATHWAY_RASTER = "sea_nbs_pathway_v3.tif"
PATHWAY_ECOSYSTEM_BAND = 2
PATHWAY_ECO_TO_AXIS3 = {1: 1, 4: 1, 2: 2, 3: 3}   # 1 dryland forest & 4 savanna both -> 1 Dryland

# ---- F02-P4 Pathway. The SAME raster, its other two bands. ----------------------------------
# 1.1 above reads band 2 only. F02-P4 reads all three, and the band layout is canonical_v3, NOT
# v2: v2's band 2 was a secondary pathway and its band 3 was the ecosystem. Reading a v2 raster
# with these constants silently swaps ecosystem and cat_code, so the two are not interchangeable.
PATHWAY_BAND = 1           # primary pathway, exactly one value per pixel
PATHWAY_CATCODE_BAND = 3   # 1..17 canonical_v3 category index, the activity join key

# Band 1. Codes 0..4 only: v2's code 5 "Not eligible for NBS" is gone, settlement folds into 4.
PATHWAY_CODES = {
    0: "No data",
    1: "Protect",
    2: "Manage",
    3: "Restore",
    4: "Ineligible",
}

# Codes 1 to 3 are the actual NBS pathways, and THE THREE ARE A PARTITION -- every pixel carries
# exactly one, so an ecosystem's Protect, Manage and Restore areas sum with its Ineligible area to
# the whole ecosystem. Confirmed with the team 2026-08-10 against the Pathway Selection design,
# whose mock showed overlapping shares (70/40/40) that this raster cannot produce.
# Code 4 is a screening outcome in the same band: no carbon credits, though non-carbon options may
# still exist.
PATHWAY_ELIGIBLE_CODES = (1, 2, 3)

# Band 2, reference ecosystem. FOUR classes plus none -- not the three-class layer 1.1 derives.
# This one carries savanna, which is what makes the savanna guardrail work at the activity level.
PATHWAY_ECOSYSTEM_CODES = {
    0: "None",
    1: "Dryland forest",
    2: "Mangrove",
    3: "Peatland",
    4: "Savanna",
}

# Band 3, canonical_v3 category index. 17 categories as a clean 1..17 index, 0 mask. Join key into
# ACTIVITY_TABLE together with the ecosystem band.
PATHWAY_CATCODE_LABELS = {
    0: "Mask",
    1: "Cat 1",  2: "Cat 2",  3: "Cat 3A", 4: "Cat 3B", 5: "Cat 4A", 6: "Cat 4B",
    7: "Cat 5",  8: "Cat 6",  9: "Cat 7",  10: "Cat 8A", 11: "Cat 8B", 12: "Cat 8C",
    13: "Cat 9A", 14: "Cat 9B", 15: "Cat 9C", 16: "Cat 9D", 17: "Cat 10",
}

PATHWAY_UNCLASSIFIED_WARN_PCT = 20.0   # 4.1 flags when this much of the AOI carries no pathway

# Primary pathway per category. Lets 4.2 label a category's pathway and know which categories are
# Ineligible (pathway 4) and therefore carry no activity by design.
PATHWAY_CATCODE_TO_PATHWAY = {
    1: 1,   # Cat 1  Protect
    2: 4,   # Cat 2  Ineligible
    3: 4,   # Cat 3A Ineligible (savanna)
    4: 3,   # Cat 3B Restore
    5: 4,   # Cat 4A Ineligible (savanna)
    6: 3,   # Cat 4B Restore
    7: 3,   # Cat 5  Restore
    8: 2,   # Cat 6  Manage
    9: 4,   # Cat 7  Ineligible
    10: 4,  # Cat 8A Ineligible (stable natural savanna)
    11: 2,  # Cat 8B Manage
    12: 3,  # Cat 8C Restore
    13: 2,  # Cat 9A Manage
    14: 3,  # Cat 9B Restore
    15: 2,  # Cat 9C Manage
    16: 4,  # Cat 9D Ineligible (settlement)
    17: 3,  # Cat 10 Restore
}

# The activity + Triple Win benefit + carbon QB catalog, joined to the raster on
# (cat_code, ecosystem). A direct export of the "NBS Pathway Logic" Sheet tab
# canonical_v3_activities. It sits BESIDE THE RASTERS under V3_BUCKET, like
# soil_class_lookup.csv, so it is read through settings.layer_path and not vendored in the repo.
ACTIVITY_TABLE = "canonical_v3_activities.csv"

# The "ELIGIBLE activities-longform" matrix: one row per (activity, benefit, indicator) with
# unit / frequency / method / reference / definition -- the monitoring-indicator catalog behind
# the F05 matrix. Exported from the team's longform sheet (via the NbS_Activities_Flow_v2 page's
# embedded data, 2026-08-27); sits beside ACTIVITY_TABLE under V3_BUCKET. Joined to activities
# BY TEXT: the sheet carries no activity id (3 of its 31 activities are newer than
# canonical_v3_activities and have no id anywhere yet).
LONGFORM_TABLE = "activities_longform_v3.csv"

# ============================ F02-P3 THREAT ============================
# Twelve rasters, ALL UNDER THE `threat/` PREFIX, and the prefix is load-bearing. Four of these
# filenames also exist at the v3 root as DIFFERENT, MUCH SMALLER PRODUCTS -- `risk_fire_v3.tif` is
# under 10 MB at the root (what 1.7 and 3.5 read) against 0.44 GB here, and flood, landslide and
# storm collide the same way. Dropping the prefix resolves silently to the wrong layer.
THREAT_ECOSYSTEM = "threat/ecosystem_v3.tif"
THREAT_HISTORICAL = "threat/historical_deforestation_v3.tif"
THREAT_FOREST_2024 = "threat/forest_2024_v3.tif"
THREAT_DISTURBANCE = "threat/forest_disturbance_v3.tif"
THREAT_FOREST_GAIN = "threat/forest_gain_v3.tif"
THREAT_DRIVERS = "threat/drivers_disturbance_v3.tif"
THREAT_FLOOD_RISK = "threat/risk_flood_v3.tif"
THREAT_LANDSLIDE_RISK = "threat/risk_landslide_v3.tif"
THREAT_STORM_RISK = "threat/risk_storm_v3.tif"
THREAT_FIRE_RISK = "threat/risk_fire_v3.tif"
THREAT_PEAT_CANALS_DENSITY = "threat/peat_canals_density_v3.tif"
THREAT_PEAT_CANAL = "threat/peat_canal.tif"

# `ecosystem_v3.tif`. NOT the pathway raster's band 2, and the two DISAGREE for the same AOI --
# 66,601 ha of dryland on AOI1 there against 67,506 ha here. Two differences matter:
#   - class 1 HERE ALREADY INCLUDES SAVANNA. Verified against the pathway raster on a Thai AOI:
#     100% of its savanna pixels (28,672 ha) land in class 1. So no folding is needed, unlike
#     F02-P4 where savanna is its own code that `by_ecosystem` folds into Dryland.
#   - class 4 is "Other" here and Savanna there. Same number, different meaning.
# Threat areas and pathway areas must therefore never be mixed in one figure.
# THREE CLASSES, NOT FOUR. The notebook's config defines this dict twice -- four classes including
# "Other" beside the threat rasters, then three classes further down -- and the LATER one wins, so
# the notebook as actually run screens on 1/2/3 only and never reports Other. Matching the run was
# chosen over matching the documented 4-class version (team decision 2026-08-18), which also makes
# this port byte-identical to the notebook's own saved output.
#
# Consequence: `total_ecosystem_area_ha` EXCLUDES land that is none of the three, so the three
# cards sum to exactly 100% of it, and the shortfall against the project area shows up in
# `total_ecosystem_percentage` rather than in an Other row.
THREAT_ECOSYSTEM_CLASSES = {
    1: "Dryland",
    2: "Mangrove",
    3: "Peatland",
}

# The class labels above are the notebook's and are what the analysis keys on. The design prints
# "Dryland forest" on the Overview card, so the two are kept apart -- same split as F02-P4's
# ECOSYSTEM_DISPLAY_NAMES.
THREAT_ECOSYSTEM_DISPLAY_NAMES = {
    "Dryland": "Dryland forest",
    "Mangrove": "Mangrove",
    "Peatland": "Peatland",
}

THREAT_DRYLAND = 1
THREAT_MANGROVE = 2
THREAT_PEATLAND = 3

# `historical_deforestation_v3.tif`
THREAT_REMAINING_FOREST = 1
THREAT_FOREST_LOSS = 2
# `forest_2024_v3.tif`
THREAT_CURRENT_FOREST = 1
# `forest_gain_v3.tif`
THREAT_FOREST_GAIN_VALUE = 1
# `forest_disturbance_v3.tif`: ANY value > 0 is disturbed.
THREAT_DISTURBANCE_THRESHOLD = 0

# `drivers_disturbance_v3.tif`. The notebook documents 1..11; only 1..8 are named, and 10 is Forest
# fire via THREAT_NATURAL_DRIVERS below. Codes 9 and 11 fall through to "Unknown".
THREAT_FOREST_DRIVER_CLASSES = {
    1: "Small-scale agriculture",
    2: "Small-scale agriculture (fire)",
    3: "Large-scale agriculture",
    4: "Large-scale agriculture (fire)",
    5: "Road development",
    6: "Selective logging",
    7: "Mining",
    8: "Non-productive conversion",
}

# Natural drivers come from the DISASTER RISK rasters, not from the driver raster, except forest
# fire. The values are the raster values that count as a risk.
#
# THE DESIGN ALSO LISTS DROUGHT AND TYPHOON. No layer supplies either, so they can never appear.
# The `raster` key spells it the notebook's way on purpose: 3.2's body reads `config["raster"]`, and
# renaming it here would mean editing that body. The value is a LAYER NAME, resolved by
# settings.layer_path inside each section's own reader.
THREAT_NATURAL_DRIVERS = {
    "Flooding": {"raster": THREAT_FLOOD_RISK, "values": [4]},
    "Forest fire": {"raster": THREAT_DRIVERS, "values": [10]},
    "Landslide": {"raster": THREAT_LANDSLIDE_RISK, "values": [4]},
    "Extreme climate event": {"raster": THREAT_STORM_RISK, "values": [4]},
}

# Mangrove reads the same driver raster but collapses it: 1-4 are one "Commodities" pressure.
THREAT_COMMODITY_CLASSES = [1, 2, 3, 4]
THREAT_SETTLEMENT_CLASS = 5
THREAT_STORM_RISK_CLASSES = [4, 5]

# The notebook computes pixel area GEODESICALLY on the WGS84 ellipsoid, row by row, because these
# rasters stay in EPSG:4326 rather than being reprojected to REFERENCE_CRS. Same deliberate
# exception as 2.3, 2.6 and burned area: the area maths is derived from the 4326 transform.
THREAT_GEOD_ELLPS = "WGS84"

# The years the disturbance record covers, for the narrative "From 2015 to 2024, ...". Not derived
# from the raster -- it carries no time dimension -- so it is stated here.
THREAT_PERIOD_FROM = 2015
THREAT_PERIOD_TO = 2024

# ---- Pathway Selection screen (F02-P4 endpoint) ---------------------------------------------
# 4.3 groups into Dryland / Mangrove / Peatland. The design calls the first one "Forest", so the
# payload carries both: `ecosystem` is the analysis key, `label` is what the card prints.
ECOSYSTEM_DISPLAY_NAMES = {"Dryland": "Forest", "Mangrove": "Mangrove", "Peatland": "Peatland"}

# DISTURBED AREA BELONGS TO F02-P3 THREAT PROFILE and has no constant here on purpose. F02-P4
# measures no disturbance of any kind, so `run_pathway` serves the three card fields as zeros until
# P3 is wired in. A previous build derived them from the pathway categories (everything that is not
# Cat 1) together with a badge threshold that had no analytical basis; both were removed once the
# real owner was identified, rather than left switched off where they could be switched back on.

# The duration slider and the carbon risk defaults. Product constants, not analysis: the screen
# needs them to render and F02-P5 needs them to calculate, so the endpoint serves them rather than
# the frontend hardcoding numbers the backend will later be asked to honour.
INTERVENTION_DURATION_DEFAULT_YEARS = 30
INTERVENTION_DURATION_MIN_YEARS = 10
INTERVENTION_DURATION_MAX_YEARS = 70
INTERVENTION_DURATION_STEP_YEARS = 10

CARBON_RISK_DEFAULTS = {
    "leakage_percentage": 15.0,      # emission displacement outside the project boundary
    "uncertainty_percentage": 10.0,  # discount for error in data and models
    "buffer_percentage": 12.0,       # buffer pool against emission release (reversal)
}

# Terrain (1.4). ONE continuous DEM in metres; slope is derived from it in code
# (common.slope_percent_from_dem), so no separate slope raster is read. 1.4 bins both itself.
# The earlier port read two PRE-CLASSIFIED rasters (srtm_elevation_v3 codes 1..7, srtm_slope_v3
# codes 1..5) because no metre DEM existed in the bucket. Those give a different answer - a
# different definition of "Flat" - so they are not a fallback, and 1.4 fails rather than
# silently switching legends.
# The bucket calls this `elevation_54034_v3.tif`; the notebook calls the same file
# `SEA_ELEVATION_54034.tif` on local disk. Same grid (253408x143684 uint16, ESRI:54034, 30 m,
# nodata 32767) and same byte count, so this is a naming difference, not a second product.
ELEVATION_RASTER = "elevation_54034_v3.tif"   # continuous metres
# Upper-exclusive bin edges: digitize(value, breaks) + 1 -> class code.
ELEVATION_BREAKS = [500, 1000, 2000]   # metres  -> elevation classes 1..4
SLOPE_BREAKS     = [8, 15, 25, 40]     # percent -> slope classes 1..5

# Historical deforestation (1.5). Both dates are binary forest cover, value 1 = forest, carrying
# the same Tier 1-2 definition applied upstream.
FC2014_RASTER = "SEA_FC2014.tif"
FC2014_FOREST_CODES = [1]
FC2024_RASTER = "SEA_FC2024.tif"
FC2024_FOREST_CODES = [1]

# Land cover (1.8). RLCMS 2024, 20 classes plus 0 = no data. This is NOT the 2024 forest mask;
# 1.5 reads FC2024_RASTER for that.
LC2024_RASTER = "SEA_LC2024.tif"

# Deforestation risk (1.6). Float raster on the 0-65535 scale, nodata 0, already masked to forest
# upstream, so its valid pixels inside the AOI are the forest to assess.
PROB_RASTER = "def_risk_v3.tif"

# Natural disaster RISK (1.7). These are the notebook's own five layers: it moved 1.7 off
# `disaster_risks/hazard_*.tif` onto `risk_*.tif`, which are exactly the objects the v3 bucket
# already published. The long-running mismatch between this port and the notebook (different
# products, non-uniform class offsets, a flashflood layer the bucket never had) is closed by that
# move rather than by anything here: both sides now read the same five files.
#
# True risk, not bare hazard: exposure and vulnerability are folded in upstream, so 1.7 reports
# risk. The five sit at very different native resolutions (flood, landslide ~100 m; fire ~1 km;
# cyclone ~11 km; drought ~28 km), so a small AOI can fall inside a single coarse cell for cyclone
# or drought and report one class. That is expected, not an error.
#
# The Climate module (3.5) still reads the older `disaster_risks/hazard_fire.tif` on a 1..5
# encoding, so 1.7 and 3.5 no longer share a fire layer. Nothing here needs to change for that;
# 3.5 is not ported yet and gets its own constant when it is.
RISK_RASTERS = {
    "cyclone":   "risk_cyclone_v3.tif",
    "drought":   "risk_drought_v3.tif",
    "fire":      "risk_fire_v3.tif",
    "flood":     "risk_flood_v3.tif",
    "landslide": "risk_landslide_v3.tif",
}

# 1.6 national comparison. Per-country percentile breakpoints of PROB_RASTER over national forest,
# built by the notebook's build_national_risk_reference.py. Columns: country, p10..p90.
# Resolved through settings.layer_path like the rasters, so it follows V3_BUCKET.
NATIONAL_FOREST_RISK_CSV = "national_forest_risk_reference.csv"

# ============================ VECTOR LAYERS ============================
# Administrative boundaries (1.2). ONE district-level shapefile carrying GADM-style fields for
# all three levels. 1.2 dissolves it per level to build country / province / district, so no
# separate L0/L1/L2 files are needed.
# NB: dissolve by the NAME columns, NOT the GID columns. In this "(revised)" file GID_1 and GID_2
# are BLANK for Indonesia (the province/district live in NAME_1/NAME_2), so grouping by GID drops
# every Indonesian unit. The ancestor names are included in each level's group so same-named
# units in different parents are not merged.
# NOT IN THE v3 BUCKET: administrative_boundaries_v3.tif is a 0/1 land mask with no attributes, and
# no vector object was published, so this reads the GIS database instead.
# db.load_admin_intersecting aliases country / province / district / sub_district to the
# COUNTRY / NAME_1 / NAME_2 / NAME_3 below, so 1.2 dissolves and intersects unchanged.
#
# public.adm_boundaries is a MATERIALIZED VIEW, 97k rows, mixed levels in one table (`loc_level` is
# 'village' for Indonesia, 'district' elsewhere). Two consequences to know:
#   - sub_district is populated for Indonesia ONLY. Everywhere else it is null, so `subdistrict`
#     falls back to null the way it did before.
#   - it covers 8 countries. sea.adm_boundaries covers 11: Brunei, Singapore and Timor-Leste are
#     absent here, and an AOI in one of those returns no admin unit at all.
# It also carries no GiST index, so the overlay is a sequential scan: ~260 ms against ~30 ms for
# sea.adm_boundaries. Switch this constant back to "sea.adm_boundaries" to trade `subdistrict`
# for that coverage and speed.
ADMIN_BOUNDARIES_TABLE = "public.adm_boundaries"
# per level: (columns to group on, name field to display, parent name field or None)
ADMIN_LEVELS = {
    "country":     (["COUNTRY"], "COUNTRY", None),
    "province":    (["COUNTRY", "NAME_1"], "NAME_1", "COUNTRY"),
    "district":    (["COUNTRY", "NAME_1", "NAME_2"], "NAME_2", "NAME_1"),
    "subdistrict": (["COUNTRY", "NAME_1", "NAME_2", "NAME_3"], "NAME_3", "NAME_2"),
    # Level 4 (desa). Populated by the matview for Indonesia only; every other country stores an
    # empty string there, which _admin_units' blank-key filter drops.
    "village":     (["COUNTRY", "NAME_1", "NAME_2", "NAME_3", "NAME_4"], "NAME_4", "NAME_3"),
}

# Protected areas, WDPA (1.3). No v3 bucket object, and the backend already holds the layer, so
# this one reads the GIS database. db.load_wdpa_intersecting renames the columns back to the
# shapefile's (NAME, DESIG_ENG, IUCN_CAT, STATUS, REALM), so 1.3 itself is unchanged.
WDPA_TABLE = "sea.wdpa_wdoecm"
WDPA_KEEP_STATUS = ("Designated", "Inscribed", "Established")
WDPA_DROP_REALM = "Marine"   # drop pure marine, keep coastal for mangrove
WDPA_STRICT_IUCN = {"Ia", "Ib", "II"}
WDPA_NO_DESIG = {"", "none", "not reported", "not applicable", "nan"}

# ============================ CLASS CODES AND LABELS ============================
ECOSYSTEM_CLASSES = {1: "Dryland", 2: "Mangrove", 3: "Peatland"}

SLOPE_CLASSES = {1: "Flat", 2: "Gently sloping", 3: "Moderately steep",
                 4: "Steep", 5: "Very steep"}

# Elevation classes produced by binning the metre DEM at ELEVATION_BREAKS, so these are codes
# 1..4 that 1.4 creates, not codes carried by a raster. The earlier port used the v2 seven-class
# legend (Lowland / Hill / Sub-montane / Lower montane / Middle montane / Upper montane /
# Subalpine) because it read a pre-classified layer; that legend is gone with that layer.
ELEVATION_CLASSES = {1: "Lowland", 2: "Submontane / hill", 3: "Montane", 4: "Upper montane"}

# RLCMS 2024 legend, 20 classes; 0 = no data. Names from the "Class Mapping" sheet of the NBS
# Pathway Logic workbook. Used by 1.8 Land Cover.
LC2024_CLASSES = {
    1: "Flooded forest", 2: "Rubber", 3: "Palm", 4: "Plantation", 5: "Crop plantation",
    6: "Mangrove", 7: "Deciduous Forest", 8: "Evergreen Forest", 9: "Shrubland", 10: "Mixed forest",
    11: "Snow", 12: "Water", 13: "Aquaculture", 14: "Rice", 15: "Building", 16: "Cropland",
    17: "Grassland", 18: "Wetland", 19: "Bareland", 20: "Other land",
}
LC_TOP_N = 6   # 1.8 subset table: the N largest land cover classes

# Risk level, FOUR classes, 0 = nodata. Settled by the notebook after it checked the `risk_*.tif`
# files directly: they are uint8 carrying 1..4. An earlier five-level reading here was inferred
# from the old hazard layers and is wrong for these; the top label is "High", not "Very High".
# (Full-extent counts do show a handful of 5s in the landslide and flood files -- 3 and 10 pixels
# out of ~2.4 billion -- which is stray encoding, not a fifth class. Such a pixel gets no label.)
RISK_LEVELS = {1: "Very Low", 2: "Low", 3: "Moderate", 4: "High"}

# ============================ THRESHOLDS AND PARAMETERS ============================
ADMIN_SLIVER_PCT = 1.0             # 1.2 report an admin unit only if >= 1% of the AOI
DEFOR_YEAR_START = 2014            # 1.5 t1
DEFOR_YEAR_END = 2024              # 1.5 t2
DEFOR_PERIOD_YEARS = DEFOR_YEAR_END - DEFOR_YEAR_START
RISK_HIGHER_PCTL = 60              # 1.6 above this national percentile is "higher than"
RISK_LOWER_PCTL = 40               # 1.6 below this national percentile is "lower than"
PROB_SCALE_MAX = 65535             # 1.6 prob raster encodes 0-100 as 0-65535
RISK_PRESENCE_PCT = 20             # 1.7 representative level = highest class covering >= this %

# ============================ FRONTEND PRESENTATION ============================
# Translation keys and colours for the endpoint payload. Nothing in the v2 backend emits either,
# so both are introduced here: `key` follows the snake_case slug convention of `tcl_classes` in
# current_condition.py, `fallback` is the English label above. Colours are placeholders until the
# frontend confirms its palette.
ECOSYSTEM_KEYS = {1: "ecosystem_dryland", 2: "ecosystem_mangrove", 3: "ecosystem_peatland",
                  "other": "ecosystem_other"}
ECOSYSTEM_COLORS = {
    1: "#2c6639",        # Dryland / forest
    2: "#f1fc3e",        # Mangrove
    3: "#abc963",        # Peatland
    "other": "#dadada",  # Other / Unclassified
}

SLOPE_KEYS = {1: "slope_flat", 2: "slope_gently_sloping", 3: "slope_moderately_steep",
              4: "slope_steep", 5: "slope_very_steep"}
SLOPE_COLORS = {1: "#1A9850", 2: "#A6D96A", 3: "#FEE08B", 4: "#FDAE61", 5: "#D73027"}

ELEVATION_KEYS = {
    1: "elevation_lowland",
    2: "elevation_submontane_hill",
    3: "elevation_montane",
    4: "elevation_upper_montane",
}

# The frontend's six-key risk vocabulary (2026-08-22). Code 5 cannot occur while the risk
# rasters carry 1..4 (see RISK_LEVELS); it is listed so a genuine fifth class would label
# rather than emit a null key.
RISK_KEYS = {1: "very_low", 2: "low", 3: "moderate", 4: "high", 5: "very_high"}
RISK_NO_DATA_KEY = "no_risk"

# 1.8. One key per RLCMS class, plus the No-data / other row that 1.8 appends as code 0.
LAND_COVER_KEYS = {
    0: "land_cover_no_data", 1: "land_cover_flooded_forest", 2: "land_cover_rubber",
    3: "land_cover_palm", 4: "land_cover_plantation", 5: "land_cover_crop_plantation",
    6: "land_cover_mangrove", 7: "land_cover_deciduous", 8: "land_cover_evergreen",
    9: "land_cover_shrubland", 10: "land_cover_mixed_forest", 11: "land_cover_snow",
    12: "land_cover_water", 13: "land_cover_aquaculture", 14: "land_cover_rice",
    15: "land_cover_building", 16: "land_cover_cropland", 17: "land_cover_grassland",
    18: "land_cover_wetland", 19: "land_cover_bareland", 20: "land_cover_other_land",
}
LAND_COVER_COLORS = {
    0: "#dadada", 1: "#1f6f4a", 2: "#8c6d3f", 3: "#c49a2f", 4: "#9c7a4a", 5: "#c9a86a",
    6: "#0f9b8e", 7: "#7fbf5f", 8: "#2c6639", 9: "#bcd35f", 10: "#4f8f3f", 11: "#f2f7fb",
    12: "#3a7bd5", 13: "#6fb3d2", 14: "#e6d24a", 15: "#b03a2e", 16: "#e8a33d",
    17: "#d4d84a", 18: "#57a0a8", 19: "#cdbfa3", 20: "#9aa0a6",
}

# =======================================================================================
# F02-P2 NATURE (components 2.x)
# =======================================================================================
# Forest landscape integrity, FLII (2.1). Grantham et al. 2020 concept, SEA-calibrated (pooled
# beta), landscape scale ~300 m, masked to forest upstream. The component reads BOTH: the
# continuous score for the headline mean, and the class raster for the High/Medium/Low shares.
#
# NEEDS UPLOADING: the GCS bucket carries only the continuous layer, as `flii_v3.tif` (which is
# byte-for-byte the notebook's flii_mosaic_SEA_300m.tif). The class mosaic exists in the
# D:\NBSTOOLV3 drop as `flii_class_mosaic_SEA_300m.tif` (6 MB) and has been copied into the local
# bucket folder as `v3_flii_class_v3.tif`, so 2.1 works locally. It still has to be uploaded to
# gs://assets-geo/v3/flii_class_v3.tif before this works on deploy.
# Deriving the classes from the paper's breaks (High >= 9.6, Low <= 6.0) instead is deliberately
# NOT done: `classify_continuous` bins upper-exclusive, so a pixel at exactly 6.0 would land in
# Medium where the paper puts it in Low. A silently different legend is worse than an absent card,
# the same rule 1.4 follows.
FLII_FOREST_RASTER = "flii_v3.tif"          # continuous 0-10, forest-masked
FLII_CLASS_RASTER  = "flii_class_v3.tif"    # 1=Low, 2=Medium, 3=High -- NOT PUBLISHED YET
FLII_CLASSES = {1: "Low", 2: "Medium", 3: "High"}

# Key Biodiversity Areas (2.2). World Database of KBAs (BirdLife / KBA Partnership). The notebook
# reads SouthEast_Asia_KBA.shp; no v3 bucket object was published and the backend already holds
# the layer, so this reads the GIS database. db.load_kba_intersecting renames `intname` back to
# the shapefile's `IntName`, so 2.2 itself is unchanged.
KBA_TABLE = "sea.key_biodiversity_area"

# NatureMap global priority ranks (2.6). Jung et al. 2021, Nat Ecol Evol 5:1499-1509, Zenodo
# 10.5281/zenodo.5006332. These are RANKED PRIORITY layers, 1 = the highest-priority percentile of
# the planet: the archive carries no raw carbon or water stocks, so nothing here is a quantity of
# anything. Three weighting scenarios, one per axis.
NATUREMAP_RASTERS = {
    "Biodiversity": "biodiversity_only_v3.tif",
    "Carbon": "biodiversity_carbon_v3.tif",
    "Water": "biodiversity_water_v3.tif",
}
# The global budgets reported: what share of the AOI falls inside the world's top 10% and top 30%
# ranked cells. 30% is the Kunming-Montreal target; 10% is the stricter comparison.
NATUREMAP_BUDGETS = (10, 30)
# Mean Earth radius, km. Only 2.6 uses it: its rasters stay in EPSG:4326 and it works out each
# cell's true ground area from the latitude band it spans, rather than reprojecting.
EARTH_RADIUS_KM = 6371.0088
# Below this many valid pixels the notebook warns that the percentages are unstable. At 10 km a
# project-scale AOI is a handful of cells, so this fires often and is passed through as a flag.
NATUREMAP_MIN_PIXELS = 20

# Key species occurrences (2.5). GBIF download, Darwin Core columns, geometry in "Shape".
# The notebook reads `key_species.shp` off a UNC share; this is the same data in the backend's
# own database, which is also why 2.5's `resolve_column` shapefile-truncation helper is not
# carried over -- see db.load_key_species_intersecting.
# COVERAGE: Aves, Mammalia and Reptilia only. There are NO Amphibia rows, so
# `amphibian_number_of_species` is a true zero from this source, not a missing value.
KEY_SPECIES_TABLE = "sea.key_species"
# Darwin Core class -> the endpoint's four species-count fields.
KEY_SPECIES_CLASS_FIELDS = {
    "Aves": "bird_number_of_species",
    "Mammalia": "mammal_number_of_species",
    "Reptilia": "reptile_number_of_species",
    "Amphibia": "amphibian_number_of_species",
}

# =======================================================================================
# F02-P2 CLIMATE (components 3.x)
# =======================================================================================
# Biomass (3.1). Continuous raster, DRY BIOMASS DENSITY in Mg/ha, not carbon. GEDI AGBD calibrated
# with Alpha Earth. The tool applies the carbon fraction and the CO2 conversion itself, so both
# conversions stay visible here rather than hidden upstream.
AGB_RASTER = "agbd_v3.tif"

# Belowground biomass is DERIVED from AGB by a fixed ratio, not read from a raster:
#     BGB_Mg/ha = AGB_Mg/ha * ROOT_TO_SHOOT_RATIO
# Consequence: the AGB/BGB split is constant by construction (about 78/22) and is NOT a
# site-specific finding. 3.1 flags this itself.
# NB: the bucket DOES carry `bgbd_v3.tif`, a mapped belowground layer the notebook does not use.
# Reading it instead would change 3.1's numbers, so it is left alone until the notebook switches.
ROOT_TO_SHOOT_RATIO = 0.28

# Soil organic carbon (3.2). Values are CARBON, tC/ha, not biomass and not CO2e. Five SoilGrids
# depth intervals: stock1..5 = 0-5, 5-15, 15-30, 30-60, 60-100 cm. 3.2 reports 0-30 cm, so it
# SUMS the top three per pixel.
SOIL_CARBON_STOCK_RASTERS = [f"soil_carbon_stock{i}_t_ha_v3.tif" for i in range(1, 6)]
SOIL_CARBON_0_30_RASTERS = SOIL_CARBON_STOCK_RASTERS[:3]
SOIL_CARBON_DEPTH_CM = 30

# Monthly climatology (3.3, 3.4). Each variable is ONE 12-band raster, band m = month m.
# VERIFY the source and period of these "_v3" files: the labels below are the notebook's own
# placeholders and are written into every 3.3/3.4 result, so a wrong label mislabels the output.
WORLDCLIM_VERSION    = "v3 (verify source)"
WORLDCLIM_PERIOD     = "verify"
WORLDCLIM_RESOLUTION = "verify"
WORLDCLIM_MONTHS     = 12
WORLDCLIM_TAVG_RASTER = "temperature_v3.tif"     # 12-band monthly mean temp, deg C (verify unit)
WORLDCLIM_PREC_RASTER = "precipitation_v3.tif"   # 12-band monthly precipitation, mm (verify unit)
MONTH_LABELS = ("Jan", "Feb", "Mar", "Apr", "May", "Jun",
                "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")

# 3.3 and 3.4: below this many valid pixels the spatial range is not meaningful, because a
# 30 arc-second grid gives a small AOI only a handful of cells.
CLIMATE_MIN_PIXELS = 5

# A month below this AOI-mean rainfall counts as dry, for the endpoint's `number_of_dry_months`.
# NOT a notebook 3.4 output: it is derived in the seam. The threshold is the notebook's own,
# documented on the dryland zone of benefit 5.3 ("dry month < 100 mm"), reused here rather than
# introducing a second definition of "dry". Depends on the precipitation unit really being mm.
DRY_MONTH_MM = 100

# Fire susceptibility (3.5). The notebook reads `disaster_risks/hazard_fire.tif` on a 1..5
# encoding. That file is NOT in the v3 bucket; the only fire layer published is the same
# `risk_fire_v3.tif` that 1.7 reads, on a 1..4 encoding. So this is an alias, and 3.5 reports the
# SAME raster 1.7 does, in a different way (largest class by area, not the conservative
# "highest class covering 20%" rule).
# WHAT THIS CHANGES: 3.5 shows four bars where the notebook shows five. The component logic is
# untouched; only the layer and its legend differ, because no other fire layer exists.
# Burned area history (endpoint fields `total_burned_area` and `historical_burned_areas`).
# Until 2026-08-22 this was NOT a notebook component -- it followed the V2 backend's
# `get_climate_burned_area_data` because the notebook had no burned-area section yet.
#
# SINCE 2026-08-22 the component is the notebook's 3.7 Historical Burned Area (team decision):
# GABAM annual burned maps, one binary raster per year under `gabam/` in the v3 bucket, value
# 1 = burned and 0 = NODATA, ~30 m, EPSG:4326. The headline is the UNION footprint (burned at
# least once); the chart is per year and sums to more wherever ground reburned. The template is a
# layer name with a {year} slot, resolved through `settings.layer_path` like every other v3 layer;
# the notebook's own GABAM_RASTER_TEMPLATE points at a local folder instead.
# The previous component reproduced v2's MODIS layers under `assets-geo/baseline/` (frequency
# raster 2011-2020 + ten annual masks at a nominal 250 m); see git history of burned_area.py if
# parity with v2's production figures is ever wanted again.
GABAM_YEARS = list(range(2014, 2025))          # 2014..2024 inclusive
GABAM_RASTER_TEMPLATE = "gabam/GABAM_{year}.tif"
FIRE_HAZARD_RASTER = RISK_RASTERS["fire"]
FIRE_LEVELS = RISK_LEVELS

# Soil classification, WRB 2006 (3.6). Class names are the WRB 2006 reference soil groups used by
# SoilGrids. Two spellings to watch in UI copy: Acrisols not "Aricsols", Nitisols not "Nitsols".
WRB_CLASSES = (
    "Acrisols", "Albeluvisols", "Alisols", "Andosols", "Arenosols", "Calcisols", "Cambisols",
    "Chernozems", "Cryosols", "Durisols", "Ferralsols", "Fluvisols", "Gleysols", "Gypsisols",
    "Histosols", "Kastanozems", "Leptosols", "Lixisols", "Luvisols", "Nitisols", "Phaeozems",
    "Planosols", "Plinthosols", "Podzols", "Regosols", "Solonchaks", "Solonetz", "Stagnosols",
    "Umbrisols", "Vertisols",
)
# Target path: one probability raster per group, summing to 100 at every pixel. None published.
WRB_PROBABILITY_RASTERS = {cls: f"wrb_{cls.lower()}_probability_v3.tif" for cls in WRB_CLASSES}

# Interim path: one categorical raster, values 0-29 (30 classes), nodata 255, plus the provider's
# code -> WRB name lookup. Supplied as `Soil Classification.xlsx` and converted to a CSV that sits
# beside the rasters under V3_BUCKET, so the backend needs no Excel dependency at runtime. The CSV
# also carries the provider's own hex colour and description per group, which the endpoint uses.
# The codes turn out to run 0..29 in the same alphabetical order as WRB_CLASSES above -- now
# CONFIRMED by the provider rather than assumed, which is why 3.6 waited for this file.
SOIL_CLASS_RASTER = "soil_groups_v3.tif"
SOIL_CLASS_TABLE = "soil_class_lookup.csv"

WRB_MODE = "categorical"        # "categorical" (share of area) or "probability" (mean prob)
WRB_MIN_PROBABILITY_PCT = 1.0   # 3.6 drop groups below this from the list
WRB_DISPLAY_TOP_N = 5           # 3.6 rows the frontend shows before "see the table"
WRB_SUM_TOLERANCE_PCT = 2.0     # 3.6 flag when the group probabilities do not sum to ~100

# Carbon conversions, shared by 3.1, 3.2 and the Benefit module.
CARBON_FRACTION = 0.47             # IPCC 2006 GL Vol 4 Ch 4, carbon fraction of dry matter
CO2_PER_C = 44.0 / 12.0            # molecular weight ratio, tCO2e per tC
CARBON_COVERAGE_WARN_PCT = 90.0    # 3.1/3.2 flag when the raster covers less of the AOI

# Area of Habitat, AoH (2.3). ONE RASTER PER SPECIES under `habitat_area/<Class>/<species>.tif`,
# uint8, EPSG:4326, DN 1 = suitable habitat. A GeoParquet inventory carries one bbox footprint per
# species and drives everything: which rasters exist, where they are, and their IUCN status.
#
# THIS REPLACED THE FOUR STACKED RASTERS in notebook commit e2f4f49/b4dc460 (2026-08-08). The old
# design was three multiband stacks with one band per species; reptiles had no stack at all, so
# `reptile_number_of_species` was null. The inventory now carries 1,112 reptiles and the field is
# a real count. `mamals_habitat_v3.tif` and friends are still in the bucket but nothing reads them.
#
# The footprint is a PREFILTER, not the answer: it is the raster's bounding box, so roughly half
# the species whose footprint intersects an AOI turn out to have no DN-1 pixel inside it (193 of
# 383 on the Indonesian test AOI). Only the survivors are counted.
AOH_INVENTORY = "habitat_area/species_iucn_v3.geoparquet"
AOH_RASTER_ROOT = "habitat_area"
AOH_TARGET_DN = 1   # DN marking suitable habitat
# Geodesic area, per the notebook: the rasters are EPSG:4326, where a pixel's ground area shrinks
# with latitude, so habitat area is summed row by row from the ellipsoid rather than from a single
# nominal cell size.
AOH_GEOD_ELLPS = "WGS84"
# How many species rasters 2.3 opens at once. This is the seam's one addition to the notebook's
# loop, and it is I/O concurrency only -- each raster is still read exactly as the notebook reads
# it. ~400 candidates on a typical AOI at 0.6 s per HTTP open is 4 minutes serially; at 16 it is
# ~25 s cold. Verified to return an identical species table to the serial loop.
AOH_MAX_WORKERS = 16
# GDAL options for those opens, and they are worth more than the thread pool is. By default GDAL
# probes for sidecars (.aux.xml, .ovr, .msk, ...) before reading a file, which over /vsicurl is
# several extra HTTP round trips PER RASTER -- times ~400 rasters. Telling it the directory is
# empty and that only .tif exists skips all of it: 157.6 s to 12.5 s on the Indonesian test AOI,
# with a byte-identical species table (same sha1 over species, pixel count and area). These change
# what is LOOKED FOR, never what is read, and are applied through a `rasterio.Env` scoped to this
# component rather than the process, so no other component's reads are affected.
AOH_GDAL_OPTIONS = {
    "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
    "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif",
}
# Inventory `class` -> the endpoint's species-count field. These four counts are the HABITAT AREA
# card, and 2.3 owns them: the card reads "suitable habitat for a wide range of wildlife", which is
# what an AoH raster answers. 2.5's recorded occurrences are a separate card and share no field
# with these. Keys are the inventory's own labels, so a class it stops publishing goes null rather
# than silently reading 0.
AOH_TAXON_FIELDS = {
    "Mammal":    "mammal_number_of_species",
    "Bird":      "bird_number_of_species",
    "Amphibian": "amphibian_number_of_species",
    "Reptile":   "reptile_number_of_species",
}

# Endangered tree species richness (2.x, endpoint field `endangered_tree_number_of_species`).
# Continuous raster: each pixel holds a COUNT of endangered tree species. Not a notebook
# component -- the v3 notebook has no endangered-tree section -- so the logic follows the v2
# backend's `get_nature_richness_data` in utils/geos/current_condition.py, which clips the layer
# and reports `round(mean of non-nodata pixels)`.
# NB the reported figure is a per-pixel MEAN, i.e. typical richness at a point in the AOI. It is
# not the number of distinct species across the whole area, which would need the union of the
# species behind each pixel and is not recoverable from a richness raster.
TREE_SPECIES_RASTER = "tree_species_v3.tif"

# =======================================================================================
# F02-P2 PEOPLE (components 6.x)
# =======================================================================================
# Two sources, and they answer different questions. The RASTERS (6.1, 6.2) describe the polygon
# the user drew: population and vulnerability are gridded, so they can be clipped to the AOI
# exactly. The SOCIAL DATABASE describes the ADMINISTRATIVE UNIT the AOI sits in -- a province, a
# district, sometimes a whole country -- because national statistics offices publish nothing
# finer. A payload therefore mixes a site figure with a regional one, and the regional sections
# each carry `administrative_area_name` so a reader can see which area a number is about.

# Population (6.1). WorldPop. The sex rasters carry 20 five-year age bands each, as BANDS, named
# f_00_2025 .. f_90_2025 and m_00_2025 .. m_90_2025. Since notebook commit `ab308c9` (2026-08-22)
# the total is male + female and the total raster `gridded_population_v3.tif` is no longer read.
POP_FEMALE_RASTER = "female_pop_v3.tif"
POP_MALE_RASTER = "male_pop_v3.tif"

# The 20 source bands collapse to these 14 display ranges. Values are 1-based BAND POSITIONS, so
# "0-4" sums bands 1 (age 0) and 2 (ages 1-4), and "65+" sums the six bands from 65 to 90+.
PEOPLE_AGE_GROUPS = {
    "0-4": (1, 2),
    "5-9": (3,),
    "10-14": (4,),
    "15-19": (5,),
    "20-24": (6,),
    "25-29": (7,),
    "30-34": (8,),
    "35-39": (9,),
    "40-44": (10,),
    "45-49": (11,),
    "50-54": (12,),
    "55-59": (13,),
    "60-64": (14,),
    "65+": (15, 16, 17, 18, 19, 20),
}

# Vulnerability (6.2). Four independent categorical rasters, 1..5. There is deliberately NO
# composite score: the notebook reports four separate cards and does not combine them.
# Note the environmental dimension reads `vulnerability_natural_v3.tif`; the endpoint calls the
# same thing "environmental", so the key here is the endpoint's word and the file keeps its own.
PEOPLE_VULNERABILITY_RASTERS = {
    "physical": "vulnerability_physical_v3.tif",
    "environmental": "vulnerability_natural_v3.tif",
    "economic": "vulnerability_economic_v3.tif",
    "social": "vulnerability_social_v3.tif",
}
PEOPLE_VULNERABILITY_LEVELS = {
    1: "Very Low",
    2: "Low",
    3: "Moderate",
    4: "High",
    5: "Very High",
}

# ---------------------------------------------------------------------------------------
# Social statistics (6.3). NOT a notebook component -- the People notebook covers 6.1 and 6.2
# only, and says so ("Household estimation is excluded"). Everything below is the endpoint's own
# contract, filled from the `se_v3` foreign tables in the app database, whose contents were
# specified by the sample queries in _test/nbs/sosial_query/*.sql.
#
# Every table shares one schema: name_1..name_4, year, category, subgroup, unit, value. What
# differs is which admin level carries the data, and which category/subgroup selects the row the
# contract wants. That, and nothing else, is what this table records.
#
# THE PAYLOAD SHAPE IS PER COUNTRY, on purpose. The frontend has eleven separate types, because
# eleven statistics offices publish different things. A field the country has no table for is
# absent from the spec and comes out null, with a flag naming it -- never a zero, which would read
# as a measurement.
#
# YEAR IS NOT HARDCODED. The sample queries pin a year per table, and all 39 of them turned out to
# be that table's newest year, so nothing here names a year: db.load_social_rows takes the newest
# year that actually carries the rows asked for. See its docstring for why that is not the same as
# `se_v3.data_max_year`, which this deliberately does not read.
SOCIAL_SCHEMA = "se_v3"

# `public.adm_boundaries.country` -> the prefix every one of that country's tables carries.
COUNTRY_ISO3 = {
    "Brunei": "brn",
    "Cambodia": "khm",
    "Indonesia": "idn",
    "Laos": "lao",
    "Malaysia": "mys",
    "Myanmar": "mmr",
    "Philippines": "phl",
    "Singapore": "sgp",
    "Thailand": "tha",
    "Timor-Leste": "tls",
    "Vietnam": "vnm",
}


def _ind(table, level, read="value", exclude=(), **where):
    """One contract field, and where its number comes from.

    table   suffix after the ISO3 prefix, e.g. "unemployment_rate" -> se_v3.tha_unemployment_rate
    level   which admin column the AOI is matched on: 0 country-wide (no name filter), 1 name_1,
            2 name_2, 3 name_3. A table finer than the level given is AGGREGATED to it --
            Indonesia publishes education per village, and level 3 sums those villages to the
            sub-district.
    where   category / subgroup filters. A key set to None means the column IS NULL, which is how
            several tables mark their own total row; omitting the key means no filter at all, and
            then every matching row contributes. That distinction matters: brn_toilet_facility_
            categories carries both a 60% and a 37% category row AND a 97% total row.
    read    value            sum of the matched rows, the plain number case
            shares           [{id: category, percentage}], each category's share of the matched
                             total, or its own value when the table is already in percent
            category_values  [{id: category, value}], for a breakdown whose unit is money
            diseases         [{id: category, contagious_id}], contagious_id always null -- the
                             table has no such column, see the health note below
            top_id           the category with the largest value
            top_value        that category's value
            coverage_pct     100 * included / all, for a table that lists every facility type in
                             counts and is asked for a single "has access" percentage
    exclude categories dropped before any of the above.
    """
    return {"table": table, "level": level, "read": read, "exclude": exclude, "where": where}


# HEALTH, all three countries that have it: the contract asks each disease for a `contagious_id`,
# and no `*_top5_common_diseases` table carries contagiousness in any column. It is emitted as
# null rather than guessed from the disease name.
SOCIAL_INDICATORS = {
    # -- Brunei -------------------------------------------------------- no households table
    "brn": {
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 0, subgroup="Total"),
            "underemployment_rate_percentage": _ind("underemployment_rate", 1, subgroup="Total"),
            "employment_rate_percentage": _ind("employment_rate", 0, subgroup="Total"),
            "employment_sectors": _ind("employment_by_sector", 1, "shares", subgroup="Total"),
            "industries_occupations": _ind("top3_industries", 0, "shares", subgroup="Total"),
        },
        "education": {
            "students_enrolled_number": _ind("students_enrolled", 1, subgroup="Total"),
            "literacy_rate_percentage": _ind("literacy_rate", 0, subgroup="Total"),
        },
        "economy": {
            # Categories are household SIZE bands (2, 3, ... 10 & Above). The single average is
            # the row with no category, which is why category=None is spelled out.
            "avg_household_income": _ind("average_household_income", 0,
                                         category=None, subgroup="Total"),
        },
        "housing_settlements": {
            "household_with_water_access_percentage": _ind("households_water_access", 0,
                                                           subgroup="Total"),
            "household_with_toilet_access_percentage": _ind("toilet_facility_categories", 0,
                                                            category=None, subgroup="Total"),
            # This table carries BOTH its five category rows (subgroup null) and its own total
            # row (subgroup "Total"), so the two fields must filter oppositely. Reading it with
            # no filter at all would count every hectare twice.
            "permanent_reserved_forest_area": _ind("permanent_reserved_forests", 0,
                                                   subgroup="Total"),
            "permanent_reserved_forests": _ind("permanent_reserved_forests", 0, "shares",
                                               subgroup=None),
        },
    },
    # -- Cambodia ------------------------------------------------------------------ complete
    "khm": {
        "social_demography": {
            "household_number": _ind("number_of_households", 1, subgroup="Total"),
        },
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 0, subgroup="Total"),
            "employment_rate_percentage": _ind("employment_rate", 0, subgroup="Total"),
            "employment_sectors": _ind("employment_by_sector", 0, "shares", subgroup="Total"),
            "industries_occupations": _ind("top3_industries", 0, "shares", subgroup="Total"),
        },
        "education": {
            "students_enrolled_number": _ind("students_enrolled", 1, subgroup="Total"),
        },
        "housing_settlements": {
            # Dry and rainy season are separate rows; the sample query takes the dry season, the
            # lower of the two and the one a screening tool should show.
            "household_with_water_access_percentage": _ind("households_water_access", 1,
                                                           subgroup="Dry Season"),
            "household_with_toilet_access_percentage": _ind("toilet_facility_categories", 1,
                                                            subgroup="Total"),
        },
    },
    # -- Indonesia --------------------------- no households table, no students-enrolled table
    "idn": {
        "employment": {
            # Indonesia publishes twice a year; there is no "Total" subgroup, only February and
            # August. August is the later round and the one the sample income query pins.
            "unemployment_rate_percentage": _ind("unemployment_rate", 1, subgroup="August"),
            "underemployment_rate_percentage": _ind("underemployment_rate", 1, subgroup="Total"),
            # NOTE the categories here are employment STATUS (Employee, Own-account worker,
            # Family/unpaid worker...), not occupation. It is the only table that could fill
            # dominant_occupation_id, so the field carries a status name.
            "dominant_occupation_id": _ind("employment_by_sector", 1, "top_id", subgroup="August"),
            "employment_sectors": _ind("employment_by_sector", 1, "shares", subgroup="August"),
        },
        "education": {
            "literacy_rate_percentage": _ind("literacy_rate", 1, subgroup="Total"),
            "literacy_rate_percentage_male": _ind("literacy_rate", 1, subgroup="Male"),
            "literacy_rate_percentage_female": _ind("literacy_rate", 1, subgroup="Female"),
            # Published per VILLAGE; level 3 sums the villages of the AOI's sub-district, which is
            # the level the contract names (subdistrict_name).
            "population_education_levels": _ind("population_educated", 3, "shares",
                                                subgroup="Total"),
        },
        "economy": {
            "household_incomes_by_sector": _ind("average_household_income", 1, "category_values",
                                                subgroup="August"),
        },
        "health": {
            "common_diseases": _ind("top5_common_diseases", 1, "diseases", subgroup="Total"),
        },
        "housing_settlements": {
            "household_with_water_access_percentage": _ind("households_water_access", 1,
                                                           subgroup="Total"),
        },
    },
    # -- Laos ---------------------------------------------------------------------- complete
    "lao": {
        "social_demography": {
            "household_number": _ind("number_of_households", 1, subgroup="Total"),
        },
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 0, subgroup="Total"),
        },
        "education": {
            "literacy_rate_percentage": _ind("literacy_rate", 0, subgroup="Total"),
        },
        "housing_settlements": {
            # "Basic" and "Safely Managed" are the two SDG service levels; the sample takes Basic.
            "household_with_water_access_percentage": _ind("households_water_access", 0,
                                                           subgroup="Basic"),
            # The only toilet table published as HOUSEHOLD COUNTS per facility type rather than a
            # percentage, so the single "has access" figure is derived: every facility type except
            # the two that record its absence or omission.
            "household_with_toilet_access_percentage": _ind(
                "toilet_facility_categories", 1, "coverage_pct",
                exclude=("Other (incl. no facility)", "Not stated"), subgroup="Total"),
            "permanent_reserved_forest_area": _ind("permanent_reserved_forests", 1,
                                                   subgroup="Total"),
        },
    },
    # -- Malaysia ------------------------------------------------------------------ complete
    "mys": {
        "social_demography": {
            "household_number": _ind("number_of_households", 1, subgroup="Total"),
        },
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 1, subgroup="Total"),
            "industries_occupations": _ind("top3_industries", 1, "shares", subgroup="Total"),
            "employment_sectors": _ind("employment_by_sector", 1, "shares", subgroup="Total"),
        },
        "education": {
            "students_enrolled_number": _ind("students_enrolled", 1, subgroup="Total"),
        },
        "economy": {
            "avg_household_income": _ind("average_household_income", 1, subgroup="Total"),
        },
        "housing_settlements": {
            "household_with_water_access_percentage": _ind("households_water_access", 1,
                                                           category="Treated Piped Water",
                                                           subgroup="Total"),
            "household_with_toilet_access_percentage": _ind("toilet_facility_access", 1,
                                                            category="Sanitary Latrine",
                                                            subgroup="Total"),
            "permanent_reserved_forest_area": _ind("permanent_reserved_forests", 1,
                                                   subgroup="Total"),
        },
    },
    # -- Myanmar ------------------------------ only two tables exist; the contract asks for two
    "mmr": {
        "social_demography": {
            "household_number": _ind("number_of_households", 2,
                                     category="Households", subgroup="Total"),
        },
        "education": {
            "students_enrolled_number": _ind("students_enrolled", 2,
                                             category="Currently attending", subgroup="Total"),
        },
    },
    # -- Philippines --------------------------------------------------------------- complete
    "phl": {
        "social_demography": {
            "household_number": _ind("number_of_households", 2,
                                     category="Number of Households", subgroup="Total"),
        },
        "employment": {
            # One table, three contract fields: the rate is the CATEGORY, not the table.
            "unemployment_rate_percentage": _ind("employment_rate", 1,
                                                 category="Unemployment Rate", subgroup="Total"),
            "employment_rate_percentage": _ind("employment_rate", 1,
                                               category="Employment Rate", subgroup="Total"),
            "underemployment_rate_percentage": _ind("employment_rate", 1,
                                                    category="Underemployment Rate",
                                                    subgroup="Total"),
            "employment_sectors": _ind("employment_by_sector", 1, "shares", subgroup="Total"),
            "industries_occupations": _ind("top5_industries_occupations", 1, "shares",
                                           subgroup="Total"),
        },
        "education": {
            # NO `population_education_levels`. `phl_population_educated` and
            # `phl_students_enrolled` were withdrawn by the data team (2026-08-08): there is no
            # Philippine data behind them, and what the tables held was Vietnamese provinces.
            "literacy_rate_percentage": _ind("literacy_rate", 1, subgroup="Total"),
            "literacy_rate_percentage_male": _ind("literacy_rate", 1, subgroup="Male"),
            "literacy_rate_percentage_female": _ind("literacy_rate", 1, subgroup="Female"),
        },
        "economy": {
            "avg_household_income": _ind("average_household_income", 1,
                                         category="Average Family Income", subgroup="Total"),
        },
        "housing_settlements": {
            # Both published as household COUNTS across every source / facility type, so the
            # contract's percentages are each type's share of all households.
            "water_access": _ind("households_water_access", 1, "shares", subgroup="Total"),
            "toilet_categories": _ind("toilet_facility_categories", 1, "shares",
                                      subgroup="Total"),
        },
    },
    # -- Singapore --------------------------- no forest AREA; the table is a percentage instead
    "sgp": {
        "social_demography": {
            "household_number": _ind("number_of_households", 0, subgroup="Total"),
        },
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 0, subgroup="Total"),
            "underemployment_rate_percentage": _ind("underemployment_rate", 0, subgroup="Total"),
            "employment_rate_percentage": _ind("employment_rate", 0, subgroup="Total"),
        },
        "education": {
            # Eight institution types and no total row, so this sums them: primary through
            # university plus the arts institutions and the ITE.
            "students_enrolled_number": _ind("students_enrolled", 0, subgroup="Total"),
            "literacy_rate_percentage": _ind("literacy_rate", 0, subgroup="Total"),
        },
        "economy": {
            "avg_household_income": _ind("average_household_income", 0,
                                         category="Resident Households", subgroup="Average"),
        },
        "health": {
            "common_diseases": _ind("top5_common_diseases", 0, "diseases", subgroup="Total"),
        },
        "housing_settlements": {
            "permanent_reserved_forest_percentage": _ind("permanent_reserved_forests", 0,
                                                         subgroup="Total"),
        },
    },
    # -- Thailand ------------------------------------------------------------------ complete
    "tha": {
        "social_demography": {
            "household_number": _ind("number_of_households", 1,
                                     category="Registered households", subgroup="Total"),
        },
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 1, subgroup="Total"),
            "employment_sectors": _ind("employment_by_sector", 1, "shares", subgroup="Total"),
            "industries_occupations": _ind("top3_industries", 1, "shares", subgroup="Total"),
        },
        "education": {
            # Four schooling levels, no total row: pre-primary through upper secondary, summed.
            "students_enrolled_number": _ind("students_enrolled", 1, subgroup="Total"),
            # "Never Enrolled" is dropped, as in the sample query: the field describes the
            # education levels people reached, and never enrolling is not one of them.
            "population_education_levels": _ind("population_educated", 1, "shares",
                                                exclude=("Never Enrolled",), subgroup="Total"),
        },
        "economy": {
            "avg_household_income": _ind("average_household_income", 1, subgroup="Total"),
        },
        "health": {
            "common_diseases": _ind("top5_common_diseases", 1, "diseases", subgroup="Total"),
        },
    },
    # -- Timor-Leste --------------------------------------------------------------- complete
    "tls": {
        "social_demography": {
            "household_number": _ind("number_of_households", 1, subgroup="Total"),
        },
        "employment": {
            "employment_rate_percentage": _ind("employment_rate", 1, subgroup="Total"),
        },
        "education": {
            "students_enrolled_number": _ind("students_enrolled", 1),
        },
        "housing_settlements": {
            "household_with_water_access_percentage": _ind("households_water_access", 1,
                                                           category="Safe Drinking Water",
                                                           subgroup="Total"),
        },
    },
    # -- Vietnam -------------------- no households table, no permanent reserved forests table
    "vnm": {
        "employment": {
            "unemployment_rate_percentage": _ind("unemployment_rate", 1, subgroup="Total"),
            "underemployment_rate_percentage": _ind("underemployment_rate", 1, subgroup="Total"),
            # Vietnam's employment_by_sector carries no categories at all -- it is a headcount of
            # the employed, which is exactly the field the contract asks for.
            "employment_number": _ind("employment_by_sector", 1, subgroup="Total"),
        },
        "education": {
            "students_enrolled_number": _ind("students_enrolled", 1, subgroup="Total"),
            "literacy_rate_percentage": _ind("literacy_rate", 1, subgroup="Total"),
            "most_common_education_people_level_id": _ind("population_educated", 1, "top_id",
                                                          subgroup="Total"),
            "most_common_education_people_number": _ind("population_educated", 1, "top_value",
                                                        subgroup="Total"),
        },
        "economy": {
            "avg_household_income": _ind("average_household_income", 1, subgroup="Total"),
        },
        "housing_settlements": {
            "household_with_water_access_percentage": _ind("households_water_access", 1,
                                                           subgroup="Total"),
            "household_with_toilet_access_percentage": _ind("toilet_facility_categories", 1,
                                                            subgroup="Total"),
        },
    },
}

# Which admin name each section reports, as the contract spells it, and at which level.
#
# Most sections carry one `administrative_area_name`, and social_statistics derives its level from
# the section's own indicators -- the level most of them use -- so nothing has to be listed here.
# Only Indonesia differs, and it differs in a way no rule could infer: its education section names
# TWO areas, because the literacy figures are provincial while the education-level breakdown is
# per sub-district, and the contract asks for both names side by side.
#
# Singapore is listed as having none. Every Singaporean table is country-wide and its contract
# carries no area name in any section.
SOCIAL_AREA_NAME_FIELDS = {
    "idn": {
        "education": {"province_name": 1, "subdistrict_name": 3},
        "health": {"province_name": 1},
    },
}
SOCIAL_NO_AREA_NAME = ("sgp",)

# Countries whose statistical hierarchy does not line up with the boundary layer's.
#
# 6.3 keys `name_1` on 1.2's `dominant_province` and `name_2` on its `dominant_district`, which is
# right for ten of the eleven countries. THE PHILIPPINES IS SHIFTED ONE WHOLE TIER: the statistics
# office publishes by REGION at level 1 and by PROVINCE at level 2, while GADM has no region at
# all and starts at the province. An AOI in Palawan therefore looked up 'Palawan' in tables keyed
# on 'MIMAROPA', and 'Puerto Princesa City' in a table keyed on 'Palawan'. Not a spelling
# difference that folding accents or casing could bridge -- a different tier, so nothing matched:
# 0 of 13 fields.
#
#     se_v3 column   Philippine tier   where the name comes from
#     name_1         region            phl.regions_1{7,8}.adm1_name, resolved spatially
#     name_2         province          1.2's `dominant_province`, one tier up from its usual slot
#
# Both sides were checked against the data: the region layers' `adm1_name` covers every `name_1`
# value in the phl tables, and GADM's Philippine provinces cover all 81 `name_2` values in
# `phl_number_of_households`. Exactly, in both directions.
#
# A LIST of (table, column) pairs means "resolve spatially against these, in priority order"; a
# string means "use this key of 1.2's values". Only the spatial form costs a query.
#
# TWO REGIONAL VINTAGES, AND THE TABLE DECIDES WHICH. The Philippines created the Negros Island
# Region in 2024 by carving Negros Occidental out of Region VI and Negros Oriental out of Region
# VII. Some statistics tables were compiled before that and know 17 regions; some know 18. So the
# GIS database carries both boundary layers, `phl.regions_17` and `phl.regions_18`, and the rule is
# the data team's: use the 18-region layer for a table that has NIR rows, the 17-region layer for
# one that does not.
#
# That is implemented by preferring the NIR vintage and falling back: if a table has no row for
# 'Negros Island Region (NIR)' it is a 17-region table, and the 17-region name is what it answers
# to. Same outcome as testing for NIR directly, and it keeps working when the data team adds NIR
# to another table -- nothing here has to be re-edited.
#
# The fallback only ever engages ON NEGROS. Everywhere else the two layers return the same name,
# the first lookup succeeds and the second is never made.
#
# WHEN: the layers are resolved once per request, before the indicator loop, and only for a country
# listed here. Each is one GiST-indexed intersect against 17 or 18 polygons.
# WHERE: db.load_statistical_area, beside the other spatial loaders, so the AOI still reaches the
# database as a bound parameter and 1.2 is left exactly as it is. Overriding `dominant_province`
# in 1.2 instead would change what the ADMINISTRATIVE BOUNDARIES card says the AOI sits in, which
# is a GADM question with a GADM answer; this is only about which row of a statistics table to
# read, and the two now disagree on purpose. 6.3 flags that disagreement on every PH request.
SOCIAL_LEVEL_SOURCES = {
    "phl": {
        1: [("phl.regions_18", "adm1_name"), ("phl.regions_17", "adm1_name")],
        2: "dominant_province",
    },
}

# ---------------------------------------------------------------------------------------
# Benefit quantification (F02-P5) -- constants copied verbatim from the notebook config
# ---------------------------------------------------------------------------------------
PROTECT_CODE = 1   # named because F02-P5 selects on it; the other codes are only tabulated
RESTORE_CODE = 3   # named because 5.3 selects Restore pixels on it

# Apply carbon risk assumption
# User inputs the leakage, uncertainty, and buffer values in the GUI. The tool applies them to the
# carbon quantification results. The default values are set here, and can be overridden by the
DEFAULT_LEAKAGE = 15.0
DEFAULT_UNCERTAINTY = 10.0
DEFAULT_BUFFER = 12.0

# ---------------------------------------------------------------------------------------
# Benefit quantification (F02-P5)
# ---------------------------------------------------------------------------------------

# 5.1 General Benefit. The three ASEAN Triple Win pillars, in the order the tool reports them.
# The keys match the three benefit columns of canonical_v3_activities, mapped to the pillar name
# used across the GUI Phase 5 (Triple Win adoption, May 2026). 5.1 collects the benefit phrases
# each activity declares, groups them under these pillars, and merges the duplicates.
TRIPLE_WIN_PILLARS = {
    "benefit_nature":  "Forestry, Ecosystem Health and Biodiversity",
    "benefit_people":  "People and Communities",
    "benefit_climate": "Climate Resilience and Mitigation",
}

# 5.1 reports every benefit that occurs, however small the area behind it, and ranks them by
# supporting area instead of dropping any. This is the denominator warning threshold only: a
# flag, not a filter, when a benefit is carried by less than this share of the AOI.
BENEFIT_SLIVER_WARN_PCT = 1.0

# 5.2 Avoided Emissions from Unplanned Deforestation reads no new layer. It combines three
# layers that other components already declare: PATHWAY_RASTER (which pixels are Protect),
# PROB_RASTER (how the projected loss is placed), and AGB_RASTER + the derived BGB (how much carbon
# each of those pixels holds).

# The historical rate in 1.5 is measured over 2014 to 2024. Projecting it further than the
# window it was measured in is the largest assumption in the whole calculation, so 5.2 flags a
# project duration above this. VM0048 requires a baseline to be reassessed every six years for
# the same reason. The tool still returns a full figure; it does not truncate.
BASELINE_RATE_MAX_YEARS = 10

# 5.2 flag when the risk layer covers less than this share of the Protect area. Protect pixels
# without a risk value cannot receive projected loss, so they drop out of the estimate.
PROTECT_RISK_COVERAGE_WARN_PCT = 90.0

# Reference ecosystem code (pathway band 3) whose carbon is dominated by a pool 5.2 cannot see.
PATHWAY_ECOSYSTEM_PEATLAND = 3

# The word 5.2 puts in its narrative for each reference ecosystem. Only three of the five band 3
# classes appear, and that is not an omission: prob.tif is forest masked upstream, so Protect
# pixels on grassland or savanna (code 4) and on water or other (code 0) carry no risk value and
# never enter the Protect pool. A pool pixel outside this mapping means the risk layer and the
# ecosystem band disagree about what is forest, which 5.2 raises as a flag.
PROTECT_ECOSYSTEM_WORDS = {1: "forest", 2: "mangrove", 3: "peatland"}

# ---------------------------------------------------------------------------------------
# ARR carbon sequestration (Benefit module 5.3), per NBS-v3-ANX-B v2 (2026-07-28)
# ---------------------------------------------------------------------------------------
# Reference-rate / yield-curve method: accumulate living biomass (AGB + BGB) on a restoring
# stand over the project years, deduct the biomass already on site, scale by a stocking factor,
# convert to tCO2e. Rates and parameters are from ANX-B Section 4; that doc carries the sources
# and confidence levels. Biomass ONLY: no soil, no peat soil, no avoided emissions, no dead wood
# or litter. This is an ex-ante, pre-feasibility estimate, not project-grade MRV.

# Which (cat_code, ecosystem) pairs get carbon quantified. Encodes ANX-B Section 3.2
# "Sequestration calculated", keyed on the pathway raster's band-3 cat_code and band-2 ecosystem.
# Deliberately NOT the sheet's QB Carbon Sequestration flag: the sheet currently contradicts the
# method on peat (sheet No, method Yes biomass-only) and savanna (sheet Yes, method defers), and
# is flagged for reconciliation. Savanna (eco 4) is absent here = deferred; Cat 9B peat (14, 3)
# is absent = rewetting only, no planting.
ARR_SEQ_PAIRS = frozenset({
    (4, 1), (4, 2), (4, 3),     # Cat 3B  dryland, mangrove, peat
    (6, 1), (6, 2), (6, 3),     # Cat 4B
    (7, 1), (7, 2), (7, 3),     # Cat 5   (savanna 7,4 excluded)
    (12, 1), (12, 2), (12, 3),  # Cat 8C
    (14, 2),                    # Cat 9B  mangrove only (peat 14,3 is rewetting only)
    (17, 1), (17, 2), (17, 3),  # Cat 10  (savanna 17,4 excluded)
})

# Growth phases, ANX-B Section 4.4. Young Y1-20, Old Y21-40. The curve is defined only to Y40;
# beyond that no further accumulation is credited.
ARR_YOUNG_END_YEAR = 20
ARR_OLD_END_YEAR = 40

# Reference accumulation rates, Mg DRY MATTER per ha per year, ANX-B Section 4.6. Mangrove and
# peatland use one rate each; dryland is split into three zones, derived per pixel (below).
ARR_RATE_DM = {                        # non-dryland ecosystems, keyed on ecosystem code
    2: {"young": 12.0, "old": 7.0},    # mangrove
    3: {"young": 5.7, "old": 3.5},     # peatland, biomass only
}
ARR_RATE_DM_DRYLAND = {                # dryland, keyed on zone code (see ARR_DRYLAND_ZONES)
    1: {"young": 3.4, "old": 2.7},     # humid lowland (rainforest)
    2: {"young": 2.4, "old": 2.0},     # seasonal lowland (conservative, wide range)
    3: {"young": 2.4, "old": 1.9},     # humid montane
}

# Root-to-shoot ratio R (BGB / AGB), ANX-B Section 4.7, low-biomass classes.
ARR_ROOT_TO_SHOOT = {2: 0.39, 3: 0.25}                    # mangrove, peat
ARR_ROOT_TO_SHOOT_DRYLAND = {1: 0.21, 2: 0.44, 3: 0.32}   # humid lowland, seasonal, montane

# Dryland zone derivation, ANX-B Section 4.5. Derived per pixel from elevation (metres,
# ELEVATION_RASTER) and the 12-band monthly precipitation raster (WORLDCLIM_PREC_RASTER). A month
# below ARR_ZONE_DRY_MONTH_MM counts as a dry month (Walsh 1996, tropical dry-season standard).
# Rule: humid montane if elevation above 1000 m; else humid lowland if annual rainfall above
# 2000 mm AND fewer than 3 dry months; else seasonal lowland. Boundary and missing-data pixels
# fall to seasonal lowland, the lower-productivity zone, for a conservative estimate.
ARR_DRYLAND_ZONES = {1: "humid lowland", 2: "seasonal lowland", 3: "humid montane"}
ARR_ZONE_ELEV_MONTANE_M = 1000.0
ARR_ZONE_WET_ANNUAL_MM = 2000.0
ARR_ZONE_DRY_MONTH_MM = 100.0
ARR_ZONE_DRY_SEASON_MONTHS = 3
ARR_DRYLAND_DEFAULT_ZONE = 2   # seasonal lowland, used when zone inputs are missing

# Baseline mode for the primary 5.3 result (team decision, 2026-07-29):
#   "class"         - small assumed standing biomass per current LC state (ARR_BASELINE_CLASS_MGHA).
#                     This is the OFFICIAL mode. It matches the "small but non-zero" baseline the
#                     doc's Section 4.8 assumes for degraded classes, and avoids the GEDI problem.
#   "per_pixel_agb" - the per-pixel AGB raster baseline. Kept as a diagnostic only: on vegetated
#                     Restore land GEDI reads a high baseline (Section 4.9) and zeroes the result.
#   "none"          - no baseline deduction (gross). Upper-bound scenario.
# Whatever the mode, 5.3 reports all three totals in `values` for comparison.
ARR_BASELINE_MODE = "class"

# Small class-based baseline, AGB Mg/ha per current LC state. VALUES ARE PLACEHOLDERS pending
# literature references (team is sourcing them); the number scales with these, so treat the
# result as indicative until they are set. They are not from the doc.
ARR_BASELINE_CLASS_MGHA = {"C4": 25.0, "C5": 5.0, "C6": 0.0}   # AGB Mg/ha per current LC state
ARR_RESTORE_CAT_CSTATE = {                                     # Restore cat_code -> current state
    4: "C4",   # Cat 3B  Forest -> shrub / vegetation
    6: "C5",   # Cat 4B  Forest -> active use
    7: "C6",   # Cat 5   Forest -> barren
    12: "C4",  # Cat 8C  Non-forest -> vegetation
    14: "C5",  # Cat 9B  Non-forest -> active use
    17: "C6",  # Cat 10  Non-forest -> barren
}

# Carbon fraction, dry matter to carbon, ANX-B Section 4.6. Mangrove 0.451, others 0.47.
ARR_CARBON_FRACTION = {1: 0.47, 2: 0.451, 3: 0.47}

# Stocking factor, ANX-B Section 4.9. Active planting reaches full stocking; ANR and mangrove EMR
# rely on natural recruitment. The 0.8 for ANR/EMR is UNCALIBRATED (doc range 0.7 to 0.85, open
# item). ARR_ANR_PAIRS lists the (cat_code, ecosystem) whose activity is ANR or mangrove EMR:
# 3B dryland is literally ANR, and the ARR method treats Cat 4B/5/8C/10 mangrove as EMR. All
# other pairs are treated as planting. The split itself is uncalibrated and flagged.
ARR_STOCKING_PLANTING = 1.0
ARR_STOCKING_ANR = 0.8
ARR_ANR_PAIRS = frozenset({(4, 1), (6, 2), (7, 2), (12, 2), (17, 2)})

# Uncertainty band, ANX-B Section 4.11. Indicative screening range, NOT a confidence interval.
ARR_UNCERTAINTY_LOW = 0.7
ARR_UNCERTAINTY_HIGH = 1.2

# Ecosystem codes whose ARR carbon is NOT quantified: the activity and benefits still apply, but
# no carbon number is produced. 5.3 skips any (cat_code, ecosystem) in ARR_SEQ_PAIRS whose
# ecosystem is listed here, and reports the area as deferred instead.
#   4 savanna    : methodological deferral. Savanna stores carbon mainly in soil and roots,
#                  outside the biomass scope, and has no biomass rates.
#   3 peatland   : TEMPORARY exclusion (team decision, 2026-07-29). The biomass method and the
#                  peat rates exist and work; peat is held out for now. Remove 3 from this set to
#                  re-enable peat biomass quantification (it is flagged biomass-only, since peat
#                  soil and avoided emissions are out of scope).
ARR_CARBON_DEFERRED_ECO = frozenset({3, 4})

# ---------------------------------------------------------------------------------------
# Enhanced Biodiversity (5.9) and Threatened Species Habitat (5.10) -- notebook constants
# ---------------------------------------------------------------------------------------
# The notebook's DEF_RISK / ECOSYSTEM / HABITAT_ROOT / INVENTORY paths map to PROB_RASTER,
# THREAT_ECOSYSTEM, AOH_RASTER_ROOT and AOH_INVENTORY (same objects in the bucket).
ECOSYSTEM_CLASS = 1         # 1 Forest, 2 Mangrove, 3 Peatland -- the notebook's default run
ECOSYSTEM_NAMES = {
    1: "forest",
    2: "mangrove",
    3: "peatland"
}

# Change only these if your GeoParquet uses different column names
SPECIES_COL = "species"
STATUS_COL = "redlistCategory"
RASTER_COL = "raster_path"

IUCN_MAP = {
    "CRITICALLY ENDANGERED": "CR",
    "ENDANGERED": "EN",
    "VULNERABLE": "VU",
    "CR": "CR",
    "EN": "EN",
    "VU": "VU",
}

# ---------------------------------------------------------------------------------------
# 5.11 Improved forest productivity / 5.12 climate resilience / 5.13 microclimate
# (Benefit modules, notebook constants; local paths map to the bucket objects noted in the
#  notebook's own comments)
# ---------------------------------------------------------------------------------------
FOREST_CHANGE_YEARS = 10    # Defalut value is 10 years based on historical forest change data (2014-2024)
FOREST_CHANGE_RASTER = "threat/forest_change_v3.tif"

GAIN_CODE = 4
DEGRADATION_CODE = 2

# 5.12. FuturePop is one band per 5 years, 2025..2100; the flood risk layer is the reference
# grid, and exposure counts classes 4 (High) and 5 (Very High) of the threat risk layers.
POP_BASE_YEAR = 2025                #<SET: baseline year based on project duration>
FUTUREPOP_RASTER = "FuturePop_v3.tif"
FUTUREPOP_YEARS = list(range(2025, 2101, 5))

