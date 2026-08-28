# Builds loadtest/aoi_pool.json from the _test/nbs shapefiles: one {name, geometry}
# entry per source file, geometry dissolved to a single (multi)polygon in EPSG:4326.
# The k6 script's setup() picks randomly from this pool when creating sessions.
#
#   venv/Scripts/python loadtest/build_aoi_pool.py
import glob
import json
import os

import geopandas as gpd

BASE = os.path.join(os.path.dirname(__file__), '..', '..', '_test', 'nbs')
OUT = os.path.join(os.path.dirname(__file__), 'aoi_pool.json')

# 'not safe' is left out on purpose: those areas fail the coverage/size checks with a
# 400, which is its own test, not a load test.
SOURCES = (
    glob.glob(os.path.join(BASE, 'area', 'aoi', '*.shp'))
    + glob.glob(os.path.join(BASE, 'area', 'multi', '*.zip'))
    + glob.glob(os.path.join(BASE, 'area', 'safe', '*.zip'))
    + [
        os.path.join(BASE, 'AOI1.shp'),
        os.path.join(BASE, 'brunei_darussalam_peatland.zip'),
        os.path.join(BASE, 'muara_merang_4326.zip'),
    ]
)


def main():
    pool = []
    for path in SOURCES:
        name = os.path.relpath(path, BASE).replace(os.sep, '/')
        try:
            gdf = gpd.read_file(path)
            if gdf.crs is None:
                gdf = gdf.set_crs(4326)
            geom = gdf.to_crs(4326).union_all()
        except Exception as e:
            print(f"SKIP {name}: {e}")
            continue
        if geom.is_empty or geom.geom_type not in ('Polygon', 'MultiPolygon'):
            print(f"SKIP {name}: {geom.geom_type}")
            continue
        pool.append({'name': name, 'geometry': json.loads(gpd.GeoSeries([geom]).to_json())
                     ['features'][0]['geometry']})
        print(f"OK   {name}: {geom.geom_type}")

    with open(OUT, 'w') as f:
        json.dump(pool, f)
    print(f"\n{len(pool)} AOIs -> {OUT} ({os.path.getsize(OUT) // 1024} KB)")


if __name__ == '__main__':
    main()
