"""Map figures: a class raster clipped to the project area on an OpenStreetMap basemap, with the
AOI outline and a legend card. Pictures, not analysis -- rasters are read in their own EPSG:4326
grid, no reprojection and no area maths.

- `render_disturbance_map`  Figure 1 of the feasibility and monitoring documents: the four
                            `forest_change_v3.tif` classes (config.FOREST_CHANGE_CLASSES).
- `render_boundary_map`     Figure 2 of the feasibility document and the project-list card: the
                            1.1 ecosystem classes from the pathway raster's ecosystem band, with
                            everything else inside the AOI as Other.

Both return the PNG path, or None when the AOI has no raster coverage.
"""

from __future__ import annotations

import math

import matplotlib

matplotlib.use("agg")

import contextily as cx
import matplotlib.pyplot as plt
import numpy as np
import rasterio
from matplotlib.colors import to_rgba
from matplotlib.patches import Patch
from rasterio.features import geometry_mask
from rasterio.mask import mask

try:
    from .common import AOI
    from .config import (
        ECOSYSTEM_CLASSES,
        ECOSYSTEM_COLORS,
        FOREST_CHANGE_CLASSES,
        FOREST_CHANGE_RASTER,
        PATHWAY_ECO_TO_AXIS3,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
    )
    from .settings import layer_path
except ImportError:  # `python maps.py`: no package around it
    from common import AOI
    from config import (
        ECOSYSTEM_CLASSES,
        ECOSYSTEM_COLORS,
        FOREST_CHANGE_CLASSES,
        FOREST_CHANGE_RASTER,
        PATHWAY_ECO_TO_AXIS3,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
    )
    from settings import layer_path

# OpenStreetMap's tile policy wants an identifying user agent; contextily's default
# ("contextily-<random hex>") is refused with 403 "App is not following the tile usage policy".
cx.tile.USER_AGENT = "nbstool-be/1.0 (project map figures)"
# Fetched tiles are kept on disk, so a repeat render (same area, another document) never goes to
# the tile server again and survives it being down.
cx.set_cache_dir("generated-file/tile-cache")
# Tried in order; a provider that fails (down, refusing) hands over to the next, and when all
# fail the figure is drawn on a plain background rather than not at all. CartoDB is not here: it
# now needs an API key and watermarks anonymous tiles.
BASEMAPS = (cx.providers.OpenStreetMap.Mapnik, cx.providers.Esri.WorldStreetMap)
NO_BASEMAP_COLOUR = "#eef0ea"

# Wide frame like the F02-P3 screen's map card and the project-list card; the AOI sits in the
# middle third.
FIGURE_SIZE_IN = (11.3, 4.0)
FIGURE_DPI = 200
AOI_HEIGHT_SHARE = 0.75      # AOI height as a share of the frame height
AOI_OUTLINE = "#2fa35a"

OTHER = "other"
# 1.1's classes and colours; the figure spells Dryland out as the document does.
BOUNDARY_CLASSES = {
    **{code: (label.replace("Dryland", "Dryland forest"), ECOSYSTEM_COLORS[code])
       for code, label in ECOSYSTEM_CLASSES.items()},
    OTHER: ("Other", ECOSYSTEM_COLORS[OTHER]),
}


def _clip(layer: str, shapes: list, band: int = 1):
    """The band cut to the AOI's bounding box, unfilled (masked outside the polygon and on
    nodata), with its transform; None when the AOI lies outside the raster."""
    with rasterio.open(layer_path(layer)) as src:
        try:
            data, transform = mask(src, shapes, crop=True, filled=False, indexes=band)
        except ValueError:  # rasterio: "Input shapes do not overlap raster"
            return None
    return data, transform


def _draw(aoi_4326, rgba: np.ndarray, transform, classes: dict, legend_title: str,
          output_path: str) -> str:
    minx, miny, maxx, maxy = aoi_4326.total_bounds
    mid_lat = (miny + maxy) / 2
    # Degrees of longitude shrink with latitude; keep the frame square on the ground.
    lon_per_lat = 1 / math.cos(math.radians(mid_lat))
    frame_aspect = FIGURE_SIZE_IN[0] / FIGURE_SIZE_IN[1]
    height = (maxy - miny) / AOI_HEIGHT_SHARE
    width = height * frame_aspect * lon_per_lat
    if (maxx - minx) > width * AOI_HEIGHT_SHARE:
        width = (maxx - minx) / AOI_HEIGHT_SHARE
        height = width / (frame_aspect * lon_per_lat)
    cx_, cy_ = (minx + maxx) / 2, mid_lat

    fig, ax = plt.subplots(figsize=FIGURE_SIZE_IN)
    ax.set_xlim(cx_ - width / 2, cx_ + width / 2)
    ax.set_ylim(cy_ - height / 2, cy_ + height / 2)
    ax.set_aspect(lon_per_lat)
    ax.set_axis_off()

    rows, cols = rgba.shape[:2]
    left, top = transform * (0, 0)
    right, bottom = transform * (cols, rows)
    ax.imshow(rgba, extent=(left, right, bottom, top), interpolation="nearest", zorder=2)
    aoi_4326.boundary.plot(ax=ax, color=AOI_OUTLINE, linewidth=2, zorder=3)
    for source in BASEMAPS:
        try:
            cx.add_basemap(ax, crs="EPSG:4326", source=source, attribution=False, zorder=1)
            break
        except Exception:  # tile server down or refusing: next provider
            continue
    else:
        ax.set_facecolor(NO_BASEMAP_COLOUR)
        ax.set_axis_on()
        ax.set_xticks([])
        ax.set_yticks([])

    handles = [Patch(facecolor=colour, edgecolor="none", label=label)
               for label, colour in classes.values()]
    legend = ax.legend(handles=handles, title=legend_title, loc="upper left",
                       frameon=True, fancybox=True, framealpha=1, edgecolor="none",
                       borderpad=1, labelspacing=0.9, handlelength=1.2, handleheight=1.2,
                       fontsize=11, title_fontsize=12)
    legend.get_title().set_fontweight("bold")
    legend._legend_box.align = "left"

    fig.savefig(output_path, dpi=FIGURE_DPI, bbox_inches="tight", pad_inches=0)
    plt.close(fig)
    return output_path


def _shapes(aoi_4326) -> list:
    return [geom.__geo_interface__ for geom in aoi_4326 if geom is not None and not geom.is_empty]


def _paint(codes: np.ndarray, classes: dict) -> np.ndarray:
    rgba = np.zeros(codes.shape + (4,), dtype=np.float32)
    for code, (_, colour) in classes.items():
        rgba[codes == code] = to_rgba(colour)
    return rgba


def render_disturbance_map(aoi: AOI, output_path: str) -> str | None:
    aoi_4326 = aoi.geometry.to_crs(4326)
    clipped = _clip(FOREST_CHANGE_RASTER, _shapes(aoi_4326))
    if clipped is None:
        return None
    data, transform = clipped
    codes = data.filled(0)
    if not codes.any():
        return None
    return _draw(aoi_4326, _paint(codes, FOREST_CHANGE_CLASSES), transform,
                 FOREST_CHANGE_CLASSES, "Disturbance class", output_path)


def render_boundary_map(aoi: AOI, output_path: str) -> str | None:
    aoi_4326 = aoi.geometry.to_crs(4326)
    shapes = _shapes(aoi_4326)
    clipped = _clip(PATHWAY_RASTER, shapes, band=PATHWAY_ECOSYSTEM_BAND)
    if clipped is None:
        return None
    data, transform = clipped
    # Same remap as 1.1: dryland forest and savanna are one Dryland class. Every pixel inside the
    # polygon that is not one of the three ecosystems -- nodata included -- is Other, so the
    # whole project area is painted.
    inside = geometry_mask(shapes, out_shape=data.shape, transform=transform, invert=True)
    codes = np.zeros(data.shape, dtype=np.uint8)
    for src_code, axis3 in PATHWAY_ECO_TO_AXIS3.items():
        codes[data.filled(0) == src_code] = axis3
    rgba = _paint(codes, BOUNDARY_CLASSES)
    rgba[inside & (codes == 0)] = to_rgba(BOUNDARY_CLASSES[OTHER][1])
    rgba[~inside] = 0
    return _draw(aoi_4326, rgba, transform, BOUNDARY_CLASSES, "Ecosystem type", output_path)


if __name__ == "__main__":
    # Render both figures on their own, no Flask app:
    #     python maps.py [aoi path] [output folder]
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from .common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    folder = sys.argv[2] if len(sys.argv) > 2 else "."
    aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(render_disturbance_map(aoi, os.path.join(folder, "disturbance_map.png")))
    print(render_boundary_map(aoi, os.path.join(folder, "boundary_map.png")))
