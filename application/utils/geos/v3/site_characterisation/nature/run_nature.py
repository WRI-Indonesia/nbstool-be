"""
run_nature.py - run every F02-P2 Nature component and stream what the endpoint renders.

Same contract as run_general.py: each component returns `(results, view_results)`, the NDJSON
envelope and the per component failure rule come from v3/pipeline.py, and what lives here is the
`processes` list and the wiring.

Scope. The notebook's Nature section has five headings; only two are functions, and three are
ported:

| heading                        | in the notebook                              | ported |
|--------------------------------|----------------------------------------------|--------|
| 2.1 Forest Landscape Integrity | `analyze_flii(aoi) -> ComponentResult`       | yes    |
| 2.2 Key Biodiversity Areas     | `analyze_kba(aoi) -> ComponentResult`        | yes    |
| 2.3 Habitat Area               | `find_overlapping_bands(paths...)`           | SET ASIDE |
| 2.5 Key Species Presence       | top-level script, hardcoded dummy AOI        | yes    |
| 2.6 Conservation Significance  | `analyse(name, shp)`, plus a radar PNG       | yes    |

None of 2.3, 2.5 and 2.6 is an `analyze_*(aoi)` function in the notebook: all three are desktop
scripts taking a hardcoded path rather than an AOI and writing .xlsx or .png instead of returning a
result. All three are ported anyway, because in each case the ANALYSIS is unambiguous and its data
is now published: 2.3 has a real function (`find_overlapping_bands`) whose body carries over
untouched, 2.5's occurrence layer landed in the GIS database as `sea.key_species`, and 2.6 was
rewritten upstream into a single-AOI function whose two blockers are gone. Each component's
docstring records exactly what was kept and what was dropped.

2.6 became portable at notebook commit `1883e6e`. The earlier version needed
`globalgrid_mollweide_10km.tif` for land fraction, which is published in ESRI:54009 while the three
priority layers were republished in EPSG:4326, so clipping the two returned arrays of different
shapes; the rewrite drops the grid entirely and computes each cell's true ground area from the 4326
transform instead. It also collapsed from two hardcoded dummy AOIs to one. The port reproduces it
EXACTLY -- zero difference across four AOIs, three axes and both budgets.

What has not changed is its RESOLUTION, and it is the thing to know about this component: 10 km
cells, kept whole wherever the polygon touches them, so every percentage is a share of a pixel
envelope one to two times larger than the site. Project-scale AOIs land on 9 to 13 cells against
the source's own 20-cell stability warning. 2.6 reports this in its flags rather than hiding it,
and there is still no Nature contract field for it -- see conservation_significance.py for the
shape it emits in the meantime.

A sixth component, `endangered trees`, has no notebook heading at all. It fills the endpoint's
`endangered_tree_number_of_species` from `tree_species_v3.tif`, carrying over the V2 backend's
logic (`get_nature_richness_data` in current_condition.py) because that is the only existing
implementation. See endangered_trees.py, and note what its number actually means.

STILL NO IUCN STATUS. 2.3's second script joins against a workbook whose own `data_source` column
reads DUMMY_PLACEHOLDER_NOT_REAL, and the mammal inventory published to the bucket has a `species`
column only, so no threatened-species breakdown is possible and nothing here invents one.

2.3 AND 2.5 ARE TWO SEPARATE CARDS and share no field, which is what the design asks for:

    HABITAT AREA                     "suitable habitat for a wide range of wildlife"
                                     2.3: the four `*_number_of_species` counts and their total,
                                     from the modelled AoH stacks
    INDICATIVE KEY SPECIES PRESENCE  "keystone species throughout the project area"
                                     2.5: a species list grouped by class, each with its
                                     occurrence count, from recorded GBIF points

They answer different questions -- "could live here" against "was recorded here" -- so neither
should be folded into the other, and the counts live with the card that shows them.

2.3 used to be excluded on cost -- 564 windowed band reads, 59 s on AOI1. That is fixed: the AoH
stacks are `interleave=pixel`, so one tile holds every band and reading a band at a time
decompresses the same tile once per band. Reading in chunks of 32 took the two largest stacks from
63 s and 49 s to about 0.1 s each on a warm handle, and the whole component to about 5 s cold.
Identical species lists either way.

Ordering. All six components are independent, so they simply run together.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

try:
    from ...common import AOI
    from ...pipeline import component_count, error_status, safe, stream
    from .conservation_significance import analyze_conservation_significance
    from .endangered_trees import analyze_endangered_trees
    from .forest_landscape_integrity import analyze_flii
    from .habitat_area import analyze_habitat_area
    from .key_biodiversity_areas import analyze_kba
    from .key_species_presence import analyze_key_species
except ImportError:  # `python run_nature.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI
    from conservation_significance import analyze_conservation_significance
    from endangered_trees import analyze_endangered_trees
    from forest_landscape_integrity import analyze_flii
    from habitat_area import analyze_habitat_area
    from key_biodiversity_areas import analyze_kba
    from key_species_presence import analyze_key_species
    from pipeline import component_count, error_status, safe, stream

# One entry per step, in emission order. `w` is a relative cost. These have not been profiled
# across several AOIs the way General's were, so treat them as an ordering, not a measurement:
# 2.1 reads two rasters plus the 2024 forest mask, 2.2 is one indexed PostGIS query, 2.5 is one
# PostGIS query whose cost scales with how many of the 69k occurrence points fall inside, and 2.6
# reads three rasters that are tiny -- 502x407 for the whole of SEA at 10 km.
# `end` is emitted last, with empty data and a null `next`, so a client can tell a finished
# run from a dropped connection.
processes = [
    {'name': 'preparation', 'w': 0.1},
    {'name': 'forest landscape integrity', 'w': 3.7},
    {'name': 'key biodiversity areas', 'w': 1.2},
    {'name': 'habitat area', 'w': 21.9},
    {'name': 'key species presence', 'w': 1.5},
    {'name': 'conservation significance', 'w': 1.4},
    {'name': 'endangered trees', 'w': 0.9},
    {'name': 'end', 'w': 0.1},
]


def _run_components(aoi: AOI):
    """Run all six live components concurrently, yielding view_results in emission order."""
    workers = component_count(processes)
    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix='nature') as pool:
        futures = (
            pool.submit(safe, 'forest landscape integrity', analyze_flii, aoi),
            pool.submit(safe, 'key biodiversity areas', analyze_kba, aoi),
            pool.submit(safe, 'habitat area', analyze_habitat_area, aoi),
            pool.submit(safe, 'key species presence', analyze_key_species, aoi),
            pool.submit(safe, 'conservation significance',
                        analyze_conservation_significance, aoi),
            pool.submit(safe, 'endangered trees', analyze_endangered_trees, aoi),
        )

        for future in futures:
            results, view = future.result()
            yield view, error_status(results)


def stream_nature(aoi: AOI):
    """NDJSON lines for one AOI: the plan, then one line per component."""
    return stream(processes, _run_components(aoi))


if __name__ == "__main__":
    # Run both components on a file and print the stream the endpoint sends, no Flask app:
    #     python run_nature.py [aoi path]
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from ...common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    for line in stream_nature(aoi):
        sys.stdout.write(line)
        sys.stdout.flush()
