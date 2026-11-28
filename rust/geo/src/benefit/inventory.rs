//! `habitat_area._load_inventory()` as the three benefit species modules (5.9, 5.10, 5.11) use
//! it: the ONE loader 2.3 uses (`habitat_area::load_inventory`, the `parquet` crate), then
//! `inventory[inventory.intersects(aoi_4326)]` in file order.

use gdal::vector::Geometry;

use crate::common::{Aoi, polygons_geometry};
use crate::ffi::gdal_err;
use crate::settings::Settings;
use crate::site_characterisation::nature::habitat_area::load_inventory;
use crate::{Error, Result};

/// One inventory row, the columns the benefit modules read.
#[derive(Clone, Debug)]
pub struct Species {
    pub species: Option<String>,
    pub red_list_category: Option<String>,
    /// `class + "/" + basename(raster_path)` (the `_load_inventory` rewrite)
    pub raster_path: String,
}

/// `_load_inventory()` then `inventory[inventory.intersects(aoi.to_crs(inventory.crs).union_all())]`.
pub fn candidates(settings: &Settings, aoi: &Aoi) -> Result<Vec<Species>> {
    let (inventory, _) = load_inventory(settings)?;
    let aoi_geom = polygons_geometry(&super::util::aoi_polygons_4326(aoi)?, aoi.is_multi)?;
    let mut out = Vec::new();
    for sp in inventory {
        if sp.footprint.is_empty() {
            continue; // a missing geometry intersects nothing
        }
        let g = Geometry::from_wkb(&sp.footprint).map_err(gdal_err)?;
        if !g.intersects(&aoi_geom) {
            continue;
        }
        if sp.class.is_none() {
            return Err(Error::new(
                "TypeError",
                "unsupported operand type(s) for +: 'NoneType' and 'str'",
            ));
        }
        out.push(Species {
            species: sp.species,
            red_list_category: sp.redlist,
            raster_path: sp.raster_path,
        });
    }
    Ok(out)
}
