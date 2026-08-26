"""
db.py - GIS database access for the vector layers that have no file.

The notebook read WDPA from `WDPA_SEA.shp`. There is no WDPA object in the v3 bucket, and the
backend already holds the layer as `sea.wdpa_wdoecm` (the same source v2 reads for its
`protect_pct`), so 1.3 loads it from there.

`load_wdpa_intersecting` is a drop-in for `load_vector_intersecting`: it returns a GeoDataFrame in
REFERENCE_CRS with the WDPA SHAPEFILE column names, so the component's filtering and overlay code
is unchanged from the notebook. The renaming lives here, in the data access layer, which is the
seam the notebook was written around.

Two stages, same as the file path: the bbox operator `&&` first, so PostGIS uses the GiST index,
then an exact `ST_Intersects`. In v2 that ordering took the equivalent overlay from about 5 s to
about 0.05 s (see the comment at utils/geos/current_condition.py:437).

The AOI geometry is a bound parameter, never interpolated text. It comes from a polygon the user
drew, so it is untrusted input; the v2 helpers format it straight into the SQL string and that
pattern is deliberately not carried over. Only the table name is interpolated, and it is a module
constant from config.py.

Standalone by design: the engine is built from GIS_DB_SQLALCHEMY_URI in the repo .env, so a
component runs as `python protected_areas_wdpa.py` with no Flask app. Inside the app that is a
second small pool alongside `GeoUtils`.
"""

from __future__ import annotations

import os
import pathlib
import threading

import geopandas as gpd
from sqlalchemy import create_engine, text

try:
    from .config import (
        ADMIN_BOUNDARIES_TABLE,
        KBA_TABLE,
        KEY_SPECIES_TABLE,
        REFERENCE_CRS,
        SOCIAL_SCHEMA,
        WDPA_TABLE,
    )
except ImportError:  # imported as a top-level module by a component run as a script
    from config import (
        ADMIN_BOUNDARIES_TABLE,
        KBA_TABLE,
        KEY_SPECIES_TABLE,
        REFERENCE_CRS,
        SOCIAL_SCHEMA,
        WDPA_TABLE,
    )

STATEMENT_TIMEOUT_MS = 30_000

# PostGIS holds REFERENCE_CRS under this srid, with byte-identical proj4 to the one pyproj uses,
# which is what lets a clip performed in the database agree with one performed in geopandas.
REFERENCE_SRID = int(REFERENCE_CRS.split(":")[1])

# WDPA extract column -> the shapefile column name the notebook's 1.3 expects.
_WDPA_COLUMNS = {
    "name": "NAME",
    "design_eng": "DESIG_ENG",
    "iucn_categ": "IUCN_CAT",
    "status": "STATUS",
}

# POOL SIZE IS THE PER-INSTANCE CAP ON DATABASE WORK, and it is stated here rather than inherited.
# SQLAlchemy's default is pool_size=5, max_overflow=10, which is 15 checkouts that nobody chose --
# measured against the real endpoint it saturated at FOUR concurrent runs and then raised
# `QueuePool limit of size 5 overflow 10 reached` after a 30 s wait.
#
# One run holds ~4 of these at once: 1.2, 1.3, 2.2 and 2.5 each open their own connection and run
# in parallel on the component pool, plus 6.3's region lookup on the countries that need it. So the
# naive sizing is 4 x concurrency, which is the wrong shape -- it makes every added run cost the
# database four more connections.
#
# THE POOL IS ALLOWED TO BE THE QUEUE INSTEAD. Sized to 8, eight GIS queries run at a time per
# instance no matter how many requests are in flight, and the rest wait a moment for a connection.
# That is affordable because the DB work is a small part of a run: those four components cost about
# 5.8 s of the ~40 s a full run takes, and habitat area alone sets a 22 s floor that none of this
# is on the critical path of. Six concurrent runs ask for ~35 s of database work, which eight
# slots clear in under 5 s.
#
# `pool_timeout` is raised from the default 30 s for the same reason: waiting for a connection is
# now expected under load and is not a failure. It stays finite so a genuinely wedged database
# still surfaces as an error rather than hanging the stream.
_POOL_SIZE = 8
_SOCIAL_POOL_SIZE = 6
_MAX_OVERFLOW = 4
_POOL_TIMEOUT = 60

_engine = None
_engine_lock = threading.Lock()


def _get_engine():
    """One lazily built engine per process, so importing this module never opens a connection.

    Locked because the components run on a thread pool and 1.2 and 1.3 both arrive here at once
    on a cold process. Without it both would build an engine, and one pool would be orphaned with
    its connections still open.
    """
    global _engine
    if _engine is not None:
        return _engine

    with _engine_lock:
        if _engine is not None:   # built while this thread waited for the lock
            return _engine

        uri = os.environ.get("GIS_DB_SQLALCHEMY_URI") or os.environ.get("GIS_DB_CONSTRING")
        if not uri:
            # Running outside the app, so the repo .env is the only place the URI can come from.
            from dotenv import load_dotenv

            load_dotenv(pathlib.Path(__file__).resolve().parents[4] / ".env")
            uri = os.environ.get("GIS_DB_SQLALCHEMY_URI")

        if not uri:
            raise RuntimeError(
                "No GIS database URI. Set GIS_DB_SQLALCHEMY_URI, or run from a checkout whose "
                ".env carries it."
            )

        _engine = create_engine(uri, pool_pre_ping=True, pool_size=_POOL_SIZE,
                                max_overflow=_MAX_OVERFLOW, pool_timeout=_POOL_TIMEOUT,
                                connect_args={"connect_timeout": 15})
        return _engine


def _load_intersecting(select_sql: str, aoi, clipped: bool = False) -> gpd.GeoDataFrame:
    """Features of one table intersecting the AOI, reprojected to REFERENCE_CRS.

    `select_sql` names the columns and the table; it must alias `geom` and reference the `aoi` CTE
    defined here. The CTE exposes the AOI twice:

      `aoi.geom`      EPSG:4326, to be compared against the stored geometry. Use this and only
                      this in the WHERE clause: `&&` against the raw column is what lets PostGIS
                      use the GiST index, and wrapping the column in ST_Transform would defeat it.
      `aoi.geom_ref`  REFERENCE_CRS, for a SELECT that clips with ST_Intersection.

    The SRID= prefix is what carries the CRS through the cast: PostGIS parses EWKT and keeps the
    SRID (a bare WKT would cast to SRID 0 and ST_Intersects would refuse to mix it with 4326).

    `clipped` says the SELECT returns geometry already cut to the AOI and already in
    REFERENCE_CRS, so the result is read under that CRS rather than 4326.
    """
    aoi_ewkt = f"SRID=4326;{aoi.geometry.to_crs(4326).union_all().wkt}"
    aoi_ref_ewkt = f"SRID={REFERENCE_SRID};{aoi.geometry.union_all().wkt}"
    query = f"""
    with aoi as (
        select ST_MakeValid(CAST(:aoi_geom AS geometry)) as geom,
               ST_MakeValid(CAST(:aoi_geom_ref AS geometry)) as geom_ref
    )
    {select_sql}
    """

    with _get_engine().connect() as conn:
        conn.execute(text(f"SET statement_timeout = {STATEMENT_TIMEOUT_MS}"))
        # crs is stated rather than inferred so an empty result set still carries one, which
        # `to_crs` below needs.
        gdf = gpd.read_postgis(
            text(query), conn, geom_col="geom",
            params={"aoi_geom": aoi_ewkt, "aoi_geom_ref": aoi_ref_ewkt},
            crs=REFERENCE_CRS if clipped else "EPSG:4326",
        )

    # The geometry column is renamed to `geometry`, which is what gpd.read_file gives a shapefile
    # and therefore what the components expect: 1.2 reads `row.geometry` off a dissolved frame,
    # and a column still called `geom` makes that an AttributeError.
    gdf = gdf.rename_geometry("geometry")

    # Area work happens in REFERENCE_CRS, in geopandas, exactly as the notebook does it. PostGIS
    # only selects the features.
    return gdf.to_crs(REFERENCE_CRS)


def load_admin_intersecting(aoi) -> gpd.GeoDataFrame:
    """Administrative units intersecting the AOI, with the shapefile's column names.

    Columns are aliased to the COUNTRY / NAME_1 / NAME_2 / NAME_3 that ADMIN_LEVELS names, so 1.2
    dissolves and intersects unchanged.

    Two source shapes are supported, because the two candidate tables name their levels
    differently: the `public.adm_boundaries` materialized view uses country / province / district /
    sub_district, and `sea.adm_boundaries` uses country / name_1 / name_2 with no level 3. Which
    one is read is set by ADMIN_BOUNDARIES_TABLE in config.py.

    Names are returned as stored. Watch the casing: the matview holds 'Indonesia', while
    sea.adm_boundaries holds 'INDONESIA' for that one country and title case for the rest. v2
    papers over it with `initcap()` in SQL.

    The geometry is CLIPPED to the AOI in the database. Administrative units are large and
    detailed and the link to this database is the slow part of the whole module: a five district
    AOI in Thailand ships 457 KB of boundary as stored against 141 KB clipped, which measured
    0.94 s against 0.20 s. 1.2 only ever uses the part inside the AOI, and clipping does not
    change what it computes: `_admin_units` intersects each unit with the AOI itself, and
    intersecting an already clipped geometry with the same AOI returns it unchanged. Dissolve is
    likewise unaffected, because union distributes over intersection.

    The clip runs in REFERENCE_CRS, not in 4326. Clipping in degrees and reprojecting afterwards
    leaves a cut edge that is straight in the wrong CRS, which cost up to 316 m2 on the Thailand
    AOI. Clipping after ST_Transform reproduces what geopandas would have done to 0.000002 m2.
    """
    if ADMIN_BOUNDARIES_TABLE.endswith("adm_boundaries") and "public." in ADMIN_BOUNDARIES_TABLE:
        columns = """
            adm.country::text      as "COUNTRY",
            adm.province::text     as "NAME_1",
            adm.district::text     as "NAME_2",
            adm.sub_district::text as "NAME_3",
            adm.village::text      as "NAME_4",
        """
    else:
        columns = """
            adm.country::text as "COUNTRY",
            adm.name_1::text  as "NAME_1",
            adm.name_2::text  as "NAME_2",
            null::text        as "NAME_3",
            null::text        as "NAME_4",
        """

    # ST_CollectionExtract keeps the polygonal parts only. A unit that merely touches the AOI
    # edge intersects in a line, and a GeometryCollection is awkward for `dissolve`; dropping the
    # line changes no area, because a line has none.
    return _load_intersecting(
        f"""
        select
            {columns}
            ST_CollectionExtract(
                ST_Intersection(ST_Transform(adm.geom, {REFERENCE_SRID}), aoi.geom_ref), 3
            ) as geom
        from {ADMIN_BOUNDARIES_TABLE} adm, aoi
        where adm.geom && aoi.geom and ST_Intersects(adm.geom, aoi.geom)
        """,
        aoi,
        clipped=True,
    )


def load_wdpa_intersecting(aoi) -> gpd.GeoDataFrame:
    """WDPA sites intersecting the AOI, with the shapefile's column names.

    `REALM` does not exist in this table; the marine / terrestrial split lives in `ecosystem`, so
    it is mapped back to the shapefile's vocabulary here. Everything except "Marine Protected
    Areas" is reported as Terrestrial, which keeps the notebook's filter ("drop pure marine, keep
    coastal") working unchanged. Note the extract has no separate Coastal value, so a coastal site
    is indistinguishable from an inland one.

    Deliberately NOT clipped, unlike load_admin_intersecting. WDPA carries point records for sites
    with no mapped boundary, and 1.3 drops them with a `geom_type` filter on the loaded frame.
    Clipping would turn every such record into an empty MultiPolygon, which passes that filter and
    so would put a zero hectare site into the table and its name into
    `overlapping_protected_name`. Clipping here needs the type filter moved into the SQL first,
    and it buys little: 1.3 costs about 0.2 s and is not the slowest component in a parallel run.
    """
    # The AOI arrives in REFERENCE_CRS and the table stores EPSG:4326, so it goes back to 4326 to
    # be compared. The SRID= prefix is what carries that through the cast: PostGIS parses EWKT and
    # keeps the SRID (a bare WKT would cast to SRID 0 and ST_Intersects would refuse to mix it
    # with the table's 4326).
    gdf = _load_intersecting(
        f"""
        select
            w.name, w.design_eng, w.iucn_categ, w.status,
            case when w.ecosystem = 'Marine Protected Areas' then 'Marine' else 'Terrestrial' end
                as realm,
            w.geom
        from {WDPA_TABLE} w, aoi
        where w.geom && aoi.geom and ST_Intersects(w.geom, aoi.geom)
        """,
        aoi,
    )
    return gdf.rename(columns={**_WDPA_COLUMNS, "realm": "REALM"})


def load_kba_intersecting(aoi) -> gpd.GeoDataFrame:
    """Key Biodiversity Areas intersecting the AOI, with the shapefile's column names.

    2.2 reads the site name from `IntName`, the field of the notebook's SouthEast_Asia_KBA.shp.
    This table stores it lowercased as `intname`, so it is renamed back here and the component is
    unchanged. `natname` (the national name) is carried through unused, so a component that later
    wants the local-language name has it without another query.

    Deliberately NOT clipped, for the same reason as load_wdpa_intersecting: 2.2 measures overlap
    itself through union_overlap_ha and per_feature_overlap_ha, which intersect against the AOI
    anyway, so clipping in SQL would only do the same work twice.
    """
    # The AOI arrives in REFERENCE_CRS and the table stores EPSG:4326, so it goes back to 4326 to
    # be compared. See load_wdpa_intersecting for why the SRID= prefix matters on the cast.
    gdf = _load_intersecting(
        f"""
        select k.intname, k.natname, k.geom
        from {KBA_TABLE} k, aoi
        where k.geom && aoi.geom and ST_Intersects(k.geom, aoi.geom)
        """,
        aoi,
    )
    return gdf.rename(columns={"intname": "IntName", "natname": "NatName"})


def load_key_species_intersecting(aoi) -> gpd.GeoDataFrame:
    """GBIF occurrence points inside the AOI, with the Darwin Core columns 2.5 aggregates.

    The notebook reads `key_species.shp` and then runs `resolve_column` over it, because a
    shapefile truncates field names to 10 characters and `individualCount` arrives as
    `individual`. This table stores the full Darwin Core names, so the resolver has nothing to
    resolve and the columns are selected by their real names instead.

    `class` and `order` are reserved words and the columns are mixed case, hence the quoting.
    `class` is aliased to `taxon_class` so pandas `groupby("class")` is not shadowed by the
    builtin in a comprehension, and because `.class` is not a valid attribute access.

    The spatial filter is ST_Intersects in PostGIS rather than the notebook's `gpd.sjoin` with
    predicate="intersects". Same predicate, same boundary behaviour, and it means 69k points are
    never loaded to throw almost all of them away.
    """
    return _load_intersecting(
        f'''
        select
            k.species,
            k."class"           as taxon_class,
            k.family            as family,
            k."individualCount" as individual_count,
            k."eventDate"       as event_date,
            k."basisOfRecord"   as basis_of_record,
            k."Shape"           as geom
        from {KEY_SPECIES_TABLE} k, aoi
        where k."Shape" && aoi.geom and ST_Intersects(k."Shape", aoi.geom)
        ''',
        aoi,
    )


def load_statistical_area(aoi, table: str, column: str) -> str | None:
    """The name of the statistical unit holding the largest share of the AOI, or None.

    For countries where the statistics office publishes at a different administrative tier than
    the boundary layer 1.2 reads -- the Philippines, so far -- 6.3 cannot key its lookups on 1.2's
    province name. This resolves the AOI against a second layer whose names ARE the ones the
    statistics tables use. See SOCIAL_LEVEL_SOURCES for why and when.

    Largest share, not first hit: an AOI straddling two regions gets the one it mostly sits in,
    which is the same dominance rule 1.2 and 6.3 already apply everywhere else.

    `table` and `column` are interpolated because they are identifiers and cannot be bound; both
    come from SOCIAL_LEVEL_SOURCES, a module constant, and are checked against it by the caller.
    The AOI stays a bound parameter, as everywhere else in this module.
    """
    aoi_ewkt = f"SRID=4326;{aoi.geometry.to_crs(4326).union_all().wkt}"
    query = f"""
    with aoi as (
        select ST_MakeValid(CAST(:aoi_geom AS geometry)) as geom
    )
    select t.{column}::text
    from {table} t, aoi
    where t.geom && aoi.geom and ST_Intersects(t.geom, aoi.geom)
    order by ST_Area(ST_Intersection(t.geom, aoi.geom)) desc
    limit 1
    """

    with _get_engine().connect() as conn:
        conn.execute(text(f"SET statement_timeout = {STATEMENT_TIMEOUT_MS}"))
        row = conn.execute(text(query), {"aoi_geom": aoi_ewkt}).fetchone()

    return row[0] if row else None


# ============================ SOCIAL STATISTICS (6.3) ============================
# A SECOND database, and a second engine. The social tables live in the APP database rather than
# the GIS database, as foreign tables under `se_v3`, so nothing here can be joined to the admin
# boundaries in SQL: the AOI is resolved to an administrative NAME by 1.2 against the GIS database
# first, and that name is then carried across as a bound parameter.
#
# The URI is DB_SQLALCHEMY_URI, the same one the app uses for its own tables. This is a second
# ENGINE on the same database, not a second database: the components run outside the Flask request
# context on a thread pool, so they cannot borrow the app's session, and the pool sizing here is
# independent of the app's.

_social_engine = None
_social_engine_lock = threading.Lock()

# Only these two columns may be filtered on. The values come from config, but the KEYS are
# interpolated into SQL as column names, so they are checked against a whitelist rather than
# trusted -- one typo in config would otherwise be a syntax error at best.
_SOCIAL_FILTER_COLUMNS = ("category", "subgroup")

# level -> the admin column it matches. 0 means the table is country-wide and nothing is matched.
_SOCIAL_LEVEL_COLUMNS = {0: None, 1: "name_1", 2: "name_2", 3: "name_3"}


def _get_social_engine():
    """One lazily built engine per process for the app database. Same pattern as _get_engine."""
    global _social_engine
    if _social_engine is not None:
        return _social_engine

    with _social_engine_lock:
        if _social_engine is not None:
            return _social_engine

        uri = os.environ.get("DB_SQLALCHEMY_URI")
        if not uri:
            from dotenv import load_dotenv

            load_dotenv(pathlib.Path(__file__).resolve().parents[4] / ".env")
            uri = os.environ.get("DB_SQLALCHEMY_URI")

        if not uri:
            raise RuntimeError(
                "No app database URI. Set DB_SQLALCHEMY_URI, or run from a checkout whose .env "
                "carries it."
            )

        # Smaller than the GIS pool because 6.3 IS ONE COMPONENT: it issues its queries in
        # sequence on its own thread, so a run holds exactly one of these at a time, measured.
        # One per concurrent run plus a little slack, rather than the GIS pool's 4-per-run.
        _social_engine = create_engine(uri, pool_pre_ping=True, pool_size=_SOCIAL_POOL_SIZE,
                                       max_overflow=_MAX_OVERFLOW, pool_timeout=_POOL_TIMEOUT,
                                       connect_args={"connect_timeout": 15})
        return _social_engine


def load_social_rows(iso3: str, table: str, level: int, area_name: str | None,
                     where: dict) -> tuple[int | None, list[tuple]]:
    """One indicator, for one administrative area, at the newest year it was published.

    Returns `(year, [(category, unit, value), ...])`, one entry per category, already summed.
    Grouping in SQL is what lets one function serve two shapes: a table published at the level
    asked for returns its rows unchanged, and a table published FINER than the level asked for is
    aggregated to it -- Indonesia's education table holds a row per village, and a sub-district
    query adds up the villages inside it without the caller knowing.

    THE YEAR IS THE NEWEST ONE THAT MATCHES THE FILTERS, not the newest in the table. There is a
    `data_max_year` table and the sample queries agree with it, but it answers a coarser question
    than this needs. Indonesia publishes twice a year and its newest year often holds only one of
    the two rounds, so pinning the table's newest year and then asking for August returns nothing;
    likewise `idn_literacy_rate` stopped publishing a male/female split in 2023 while the combined
    figure continues. Selecting the newest year that actually has the rows asked for gives a real
    number in both cases, at the price of letting one section mix years -- which is why the year
    comes back with the rows and the caller reports it.

    `iso3` and `table` are interpolated because they name the table; both come from config.
    `area_name` never is -- it originates in the GIS database and is bound.
    """
    column = _SOCIAL_LEVEL_COLUMNS[level]
    params: dict = {}
    conditions = ["value is not null"]

    if column is not None:
        if area_name is None:
            # A province-level indicator with no province to ask about has no answer. Returning
            # nothing is right: the caller reports the field as null, not as zero.
            return None, []
        conditions.append(f"{column} = :area_name")
        params["area_name"] = area_name

    for key, value in where.items():
        if key not in _SOCIAL_FILTER_COLUMNS:
            raise ValueError(f"{iso3}_{table}: cannot filter on {key!r}")
        if value is None:
            # An explicit None means the column IS NULL, which is how several tables mark their
            # own total row. It is NOT the same as omitting the key, which filters on nothing.
            conditions.append(f"{key} is null")
        else:
            conditions.append(f"{key} = :{key}")
            params[key] = value

    where_sql = "\n              and ".join(conditions)
    query = f"""
        with matched as (
            select year, category, unit, value
            from {SOCIAL_SCHEMA}.{iso3}_{table}
            where {where_sql}
        )
        select year, category, min(unit) as unit, sum(value) as value
        from matched
        where year = (select max(year) from matched)
        group by year, category
        order by category
    """

    with _get_social_engine().connect() as conn:
        conn.execute(text(f"SET statement_timeout = {STATEMENT_TIMEOUT_MS}"))
        rows = conn.execute(text(query), params).fetchall()

    if not rows:
        return None, []

    # Decimal out of the driver; float is what the payload carries and what the arithmetic in
    # social_statistics expects.
    return int(rows[0].year), [(r.category, r.unit, float(r.value)) for r in rows]
