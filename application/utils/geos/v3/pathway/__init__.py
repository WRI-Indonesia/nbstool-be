"""F02-P4 Pathway: which NBS pathways the AOI qualifies for, and the activities under each.

Three components, ported from `F02-P4 Pathway.ipynb` with the bodies unchanged:

    4.1 Pathway Distribution   area and share of the AOI per pathway (bands 1, 2, 3)
    4.2 Activity List          (cat_code, ecosystem) joined to the canonical_v3 activity catalog
    4.3 By Ecosystem           4.2 regrouped with ECOSYSTEM as the primary axis

4.3 is what the Pathway Selection screen is built on: one card per ecosystem, its pathway mix, and
the activities under each pathway. `run_pathway` shapes it for that screen.

ONE RASTER, three bands, and it is already deployed -- 1.1 Ecosystem Type reads band 2 of the same
file. F02-P4 is the only reader of bands 1 and 3.
"""
