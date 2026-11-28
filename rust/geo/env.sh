# Source from anywhere: GDAL 3.8.4 from the venv's osgeo wheel for building and running (gdal-sdk/README.md).
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OSGEO="$REPO/venv/Lib/site-packages/osgeo"
export GDAL_LIB_DIR="$(cygpath -w "$REPO/rust/gdal-sdk")" GDAL_VERSION=3.8.4
export PATH="$OSGEO:$PATH" GDAL_DATA="$(cygpath -w "$OSGEO/data/gdal")" PROJ_DATA="$(cygpath -w "$OSGEO/data/proj")"
