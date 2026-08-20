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

RISK_KEYS = {1: "risk_very_low", 2: "risk_low", 3: "risk_moderate", 4: "risk_high"}
RISK_NO_DATA_KEY = "risk_no_data"

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
# Burned area history (3.x, endpoint fields `total_burned_area` and `historical_burned_areas`).
# NOT a notebook component: the v3 notebook has no burned-area section, so this follows the V2
# backend's `get_climate_burned_area_data` in utils/geos/current_condition.py, on the team's
# instruction to use the v2 version.
#
# TWO DIFFERENT LAYERS, and they are not interchangeable:
#   the FREQUENCY raster is one band holding, per cell, how many times it burned over the record.
#   Total burned area is the area of every cell that burned at least once -- so a cell that burned
#   four times contributes its area once, not four times.
#   the ANNUAL rasters are ten 0/1 masks, one per year, 2011-2020. Their per-year areas are the
#   chart series, and they SUM TO MORE than the total above whenever a cell burned in two
#   different years. That is not an inconsistency: one counts places, the other counts place-years.
#
# These live under `assets-geo/baseline/`, not the v3 root, so they are full URLs rather than
# layer names -- `settings.layer_path` is not involved and V3_BUCKET does not move them.
BURNED_BASELINE_ROOT = "https://storage.googleapis.com/assets-geo/baseline"
BURNED_FREQUENCY_RASTER = f"{BURNED_BASELINE_ROOT}/2024-05-14_MCD64A1_BurnArea_2011_2020_Frequency_EPSG4326.tif"
BURNED_ANNUAL_RASTER = BURNED_BASELINE_ROOT + "/burned_area_{year}.tif"
BURNED_YEARS = range(2011, 2021)
# The annual masks are 250 m in EPSG:4326 and v2 converts a pixel count to hectares with this
# nominal cell size rather than by reprojecting. Kept because the figures must match v2's.
BURNED_ANNUAL_PIXEL_M = 250
# NB the v3 bucket also carries `burned_area_v3.tif`: the same MODIS product as the frequency
# raster above but at 250 m rather than 500 m, matching the annual rasters' grid, and covering a
# longer record (on indonesia_3 it counts 906 burn events against the annual series' 710, so its
# period is not 2011-2020). Switching to it is a one-line change here, but it would move the
# reported total away from v2's and is not what was asked for.
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

# Population (6.1). WorldPop. The total raster is a plain count per cell; the sex rasters carry 20
# five-year age bands each, as BANDS, named f_00_2025 .. f_90_2025 and m_00_2025 .. m_90_2025.
POP_TOTAL_RASTER = "gridded_population_v3.tif"
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
            "unemployment_rate_percentage": _ind("unemployment_rate", 1, subgroup="Total"),
            "underemployment_rate_percentage": _ind("underemployment_rate", 1, subgroup="Total"),
            "employment_rate_percentage": _ind("employment_rate", 1, subgroup="Total"),
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
            "employment_sectors": _ind("employment_by_sector", 1, "shares", subgroup="Total"),
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
