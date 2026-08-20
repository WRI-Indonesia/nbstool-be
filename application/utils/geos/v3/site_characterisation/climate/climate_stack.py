"""
Shared reading of the WorldClim monthly stack, used by 3.3 Annual Temperature and 3.4 Annual
Precipitation.

Both components read one 12-band raster (band m = month m) and reduce it two ways: a twelve point
chart series, and one annual value per pixel. What differs between them is only how the twelve
months combine into an annual figure -- mean for temperature, sum for precipitation -- so that is
a parameter rather than two near-identical readers.

The rule that matters: a pixel counts only when ALL TWELVE months are present. A pixel with
eleven valid months would otherwise contribute a sum over eleven months to an annual total, which
reads as a drier site rather than as missing data. The same pixel set feeds the chart and the
sentence, so the two always agree.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

try:
    from ...common import AOI, load_raster_clipped
    from ...config import CLIMATE_MIN_PIXELS, MONTH_LABELS, WORLDCLIM_MONTHS
except ImportError:  # imported as a top-level module by a component run as a script
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, load_raster_clipped
    from config import CLIMATE_MIN_PIXELS, MONTH_LABELS, WORLDCLIM_MONTHS


@dataclass(frozen=True)
class MonthlyValue:
    """One bar of a twelve month chart: the spatial mean over the AOI for that month."""

    month: int          # 1 to 12
    label: str          # Jan to Dec
    value: float


@dataclass(frozen=True)
class ClimateStack:
    """Twelve monthly rasters read over the AOI, restricted to fully valid pixels."""

    monthly: list[MonthlyValue]   # spatial mean per month, for the chart
    annual: np.ndarray            # one annual value per valid pixel, 1D
    n_pixels: int
    annual_grid: object = None    # 2D masked annual value, for saving as a raster
    transform: object = None
    crs: object = None
    pixel_area_ha: float = 0.0

    @property
    def has_range(self) -> bool:
        """Whether a spatial minimum and maximum are worth reporting."""
        return self.n_pixels >= CLIMATE_MIN_PIXELS and self.annual.max() > self.annual.min()


def read_monthly_stack(path: str, aoi: AOI, annual: str) -> ClimateStack | None:
    """Read a 12-band monthly raster (band m = month m) and reduce it to a chart series and an
    annual array.

    `annual` is "mean" for temperature or "sum" for precipitation. It is applied per pixel,
    across the twelve months, before any spatial statistic is taken.

    Returns None when no pixel has all twelve months, so the caller can report not applicable.
    """
    slices = [
        load_raster_clipped(path, aoi, resampling="nearest", band=m)
        for m in range(1, WORLDCLIM_MONTHS + 1)
    ]

    # All 12 bands come from one file, so they share a grid by construction.
    # (12, rows, cols), nodata as NaN so the all-months check is one operation.
    data = np.ma.stack([s.values.astype(float) for s in slices]).filled(np.nan)

    # A pixel counts only when every month is present. See the note above.
    all_valid = ~np.isnan(data).any(axis=0)
    n_pixels = int(all_valid.sum())
    if n_pixels == 0:
        return None

    annual_per_pixel = (
        data.mean(axis=0)[all_valid] if annual == "mean" else data.sum(axis=0)[all_valid]
    )

    # 2D annual grid, for saving as a raster (masked outside the all-months-valid pixels).
    annual_2d = data.mean(axis=0) if annual == "mean" else data.sum(axis=0)
    annual_grid = np.ma.masked_array(annual_2d, mask=~all_valid)
    _ref = slices[0]

    # Chart series over the same pixel set, so chart and sentence agree.
    monthly = [
        MonthlyValue(month=m + 1, label=MONTH_LABELS[m], value=float(data[m][all_valid].mean()))
        for m in range(12)
    ]

    return ClimateStack(
        monthly=monthly, annual=annual_per_pixel, n_pixels=n_pixels,
        annual_grid=annual_grid, transform=_ref.transform, crs=_ref.crs,
        pixel_area_ha=_ref.pixel_area_ha,
    )


def monthly_view(stack: ClimateStack) -> list[dict]:
    """The twelve chart bars, in the shape the other v3 distributions use."""
    return [
        {'id': str(m.month), 'name': m.label, 'value': m.value}
        for m in stack.monthly
    ]
