"""F02-P3 Threat: where the project area is most disturbed, and what is driving it.

Four sections, ported from `F02-P3 Threat.ipynb` with the bodies unchanged, one per tab of the
Threat Profile screen:

    3.1 All Ecosystem   Overview tab   total and per-ecosystem area + disturbed area
    3.2 Dryland Forest  Forest tab     remaining / disturbed / loss / gain, plus drivers
    3.3 Mangrove        Mangrove tab   remaining / disturbed, plus drivers and main pressure
    3.4 Peatland        Peatland tab   remaining / disturbed / converted, plus canal and fire

TWELVE RASTERS, all under the bucket's `threat/` prefix. That prefix is not cosmetic: four of the
filenames also exist at the v3 root as different, much smaller products.

THIS MODULE'S ECOSYSTEM LAYER IS NOT F02-P4's. It reads `threat/ecosystem_v3.tif`, whose class 1
already includes savanna, where the pathway raster keeps savanna as its own class 4 and calls
class 4 "Other" instead. The two disagree on the same AOI, so their areas must not be mixed --
see `config.THREAT_ECOSYSTEM_CLASSES`.

THE NOTEBOOK IS NOT RUNNABLE AS PUBLISHED, and not only because of paths: its `config.py` never
imports `Path` while calling it, has no `AOI` name for cell 6 to import, leaves `GEOD` undefined,
and cells 8 and 10 reference `USER_AOI` and the mangrove class constants without importing them.
Every one of those is a wiring problem outside the analysis, which is what makes hoisting the
bodies into `analyze_*(aoi)` functions safe: nothing inside them changes.
"""
