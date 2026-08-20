"""
wrb_descriptions.py - one line descriptions of the WRB 2006 reference soil groups.

Used by component 3.6 to gloss the dominant soil group in the narrative.

SCOPE RULE, deliberate: every entry describes soil properties only. None of them says whether a
soil is good or bad for a Nature-Based Solution. Suitability for restoration, planting or
agroforestry depends on climate, current land cover, slope, hydrology and tenure, not on the
soil group alone, so the same soil group can be well suited on one site and unworkable on
another. That judgement belongs to the Pathway module, which sees the whole context. Keeping it
out of this file stops the tool from issuing a recommendation it cannot support.

NEEDS REVIEW: these glosses were drafted from the WRB 2006 reference soil group definitions and
should be checked by a soil scientist before the tool is published. They are written for a
non-specialist reader, so they simplify.
"""

WRB_DESCRIPTIONS: dict[str, str] = {
    "Acrisols": (
        "strongly weathered acidic soils with a clay-enriched subsoil and few available "
        "nutrients"
    ),
    "Albeluvisols": (
        "soils with a clay-enriched subsoil into which bleached, pale tongues extend from above"
    ),
    "Alisols": (
        "acidic soils that hold a swelling clay in the subsoil but release few nutrients"
    ),
    "Andosols": (
        "young soils formed in volcanic ash, with high organic matter content and strong water "
        "retention"
    ),
    "Arenosols": (
        "sandy soils that hold little water and few nutrients because they contain very little "
        "clay"
    ),
    "Calcisols": (
        "soils of dry climates with a marked build-up of secondary lime in the subsoil"
    ),
    "Cambisols": (
        "young soils at an early stage of development, with only weak subsoil formation"
    ),
    "Chernozems": (
        "very dark, humus-rich soils of temperate grassland, naturally high in plant nutrients"
    ),
    "Cryosols": "soils underlain by permanently frozen ground within the top metre",
    "Durisols": (
        "soils of arid and semi-arid areas containing a hardened, silica-cemented layer"
    ),
    "Ferralsols": (
        "deeply weathered red or yellow tropical soils dominated by iron and aluminium oxides, "
        "with low natural fertility but good physical structure"
    ),
    "Fluvisols": (
        "young soils formed in recent river, lake or marine deposits, often layered and fertile"
    ),
    "Gleysols": (
        "soils saturated by groundwater for long periods, recognisable by grey and mottled "
        "colours"
    ),
    "Gypsisols": "soils of dry climates with a marked build-up of secondary gypsum",
    "Histosols": (
        "organic soils built from accumulated plant remains, including peat, and holding very "
        "large amounts of carbon"
    ),
    "Kastanozems": (
        "dark brown, humus-rich soils of dry grassland, with lime accumulating in the subsoil"
    ),
    "Leptosols": (
        "very shallow soils over hard rock or coarse gravel, with little rooting depth"
    ),
    "Lixisols": (
        "weathered soils with a clay-enriched subsoil that still holds a reasonable nutrient "
        "supply"
    ),
    "Luvisols": (
        "soils with a clay-enriched subsoil, an active clay type and a good nutrient supply"
    ),
    "Nitisols": (
        "deep, well-drained red tropical soils with a strongly structured clay subsoil and good "
        "natural fertility"
    ),
    "Phaeozems": (
        "dark, humus-rich soils of moist grassland and forest, well leached but still nutrient "
        "rich"
    ),
    "Planosols": (
        "soils with an abrupt change to a slowly permeable subsoil, which causes water to sit "
        "near the surface in the wet season"
    ),
    "Plinthosols": (
        "soils containing an iron-rich material that hardens irreversibly once it is repeatedly "
        "dried"
    ),
    "Podzols": (
        "acidic sandy soils with an ash-grey leached layer over a darker layer enriched in "
        "organic matter, iron and aluminium"
    ),
    "Regosols": (
        "weakly developed soils in loose material, without clear layering"
    ),
    "Solonchaks": "soils carrying a high concentration of soluble salts",
    "Solonetz": (
        "soils with a dense, sodium-rich subsoil that restricts water movement and root growth"
    ),
    "Stagnosols": (
        "soils where rainwater perches above a dense layer, mottling the topsoil and upper "
        "subsoil"
    ),
    "Umbrisols": (
        "acidic soils with a thick, dark, humus-rich topsoil that holds few plant nutrients"
    ),
    "Vertisols": (
        "clay-rich soils that swell when wet and shrink when dry, opening deep cracks in the dry "
        "season"
    ),
}

WRB_DESCRIPTION_FALLBACK = "a soil group described by the World Reference Base for Soil Resources"


def describe_soil(group: str) -> str:
    """Gloss for one WRB group, with a neutral fallback so the narrative never breaks."""
    return WRB_DESCRIPTIONS.get(group, WRB_DESCRIPTION_FALLBACK)
