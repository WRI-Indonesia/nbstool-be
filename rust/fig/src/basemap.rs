//! contextily 1.6.1 `add_basemap(ax, crs="EPSG:4326", source=..., attribution=False)` for the
//! figure frame: the zoom contextily picks, mercantile's tile list, the same URLs and user
//! agent, an on-disk tile cache, and the mosaic drawn into the axes (one bilinear sample per
//! output pixel; contextily warps to EPSG:4326 bilinearly and imshow resamples bilinearly again).

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use crate::Frame;

pub struct Provider {
    pub url: &'static str,
    /// xyzservices max_zoom; None = unknown (contextily then allows up to 30, no clamp)
    pub max_zoom: Option<i32>,
}

/// maps.BASEMAPS: OpenStreetMap.Mapnik, then Esri.WorldStreetMap
pub const BASEMAPS: [Provider; 2] = [
    Provider {
        url: "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
        max_zoom: Some(19),
    },
    Provider {
        url: "https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/tile/{z}/{y}/{x}",
        max_zoom: None,
    },
];

/// maps.py: OpenStreetMap refuses contextily's default agent
pub const USER_AGENT: &str = "nbstool-be/1.0 (project map figures)";

const RE: f64 = 6378137.0;
const EPSILON: f64 = 1e-14;
const LL_EPSILON: f64 = 1e-11;

pub(crate) struct Basemap {
    pub zoom: i32,
    pub provider_url: String,
    pub urls: Vec<String>,
    /// premultiplied = straight (opaque) RGBA, the frame's pixel size
    pub rgba: Vec<u8>,
}

/// mercantile.xy
fn merc(lon: f64, lat: f64) -> (f64, f64) {
    (
        RE * lon.to_radians(),
        RE * (PI * 0.25 + 0.5 * lat.to_radians()).tan().ln(),
    )
}

/// contextily.tile._sm2ll
fn sm2ll(x: f64, y: f64) -> (f64, f64) {
    let shift = PI * RE;
    let lon = x / shift * 180.0;
    let lat = y / shift * 180.0;
    (
        lon,
        180.0 / PI * (2.0 * (lat * PI / 180.0).exp().atan() - PI / 2.0),
    )
}

/// mercantile.tile
fn tile(lng: f64, lat: f64, z: i32) -> (i64, i64) {
    let x = lng / 360.0 + 0.5;
    let s = lat.to_radians().sin();
    let y = 0.5 - 0.25 * ((1.0 + s) / (1.0 - s)).ln() / PI;
    let z2 = 2f64.powi(z);
    let f = |v: f64| {
        if v <= 0.0 {
            0
        } else if v >= 1.0 {
            (z2 - 1.0) as i64
        } else {
            ((v + EPSILON) * z2).floor() as i64
        }
    };
    (f(x), f(y))
}

/// mercantile.bounds -> (west, south, east, north)
fn tile_bounds(x: i64, y: i64, z: i32) -> (f64, f64, f64, f64) {
    let z2 = 2f64.powi(z);
    let lon = |x: f64| x / z2 * 360.0 - 180.0;
    let lat = |y: f64| (PI * (1.0 - 2.0 * y / z2)).sinh().atan().to_degrees();
    (
        lon(x as f64),
        lat(y as f64 + 1.0),
        lon(x as f64 + 1.0),
        lat(y as f64),
    )
}

/// contextily `_calculate_zoom` + `_validate_zoom(auto=True)`; None where contextily raises.
fn zoom_for(w: f64, s: f64, e: f64, n: f64, p: &Provider) -> Option<i32> {
    let zl = (360.0 * 2.0 / (e - w).abs()).log2().ceil();
    let zt = (360.0 * 2.0 / (n - s).abs()).log2().ceil();
    let z = zl.min(zt) as i32;
    match p.max_zoom {
        Some(m) if z > m => Some(m),
        m if z >= 0 && z <= m.unwrap_or(30) => Some(z),
        _ => None,
    }
}

fn tile_file(cache: &Path, url: &str) -> PathBuf {
    // <cache>/<host>/<z>/<x>/<y>.png, the layout of proof/tiles
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    let parts: Vec<&str> = rest.trim_end_matches(".png").rsplitn(4, '/').collect();
    let (z, a, b) = (parts[2], parts[1], parts[0]);
    let (x, y) = if host.contains("openstreetmap") {
        (a, b)
    } else {
        (b, a)
    };
    cache.join(host).join(z).join(x).join(format!("{y}.png"))
}

fn fetch(client: &reqwest::blocking::Client, cache: &Path, url: &str) -> Option<image::RgbaImage> {
    let file = tile_file(cache, url);
    if let Ok(bytes) = std::fs::read(&file)
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        return Some(img.to_rgba8());
    }
    // contextily _retryer: max_retries=2 on non-404 failures, then give up
    for _ in 0..3 {
        let Ok(resp) = client.get(url).send() else {
            continue;
        };
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return None;
        }
        let Ok(resp) = resp.error_for_status() else {
            continue;
        };
        let Ok(bytes) = resp.bytes() else { continue };
        let Ok(img) = image::load_from_memory(&bytes) else {
            continue;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&file, &bytes));
        }
        return Some(img.to_rgba8());
    }
    None
}

/// The first provider that serves every tile; None when all fail (maps.py then paints the
/// plain background).
pub(crate) fn basemap(frame: &Frame, cache: &Path, providers: &[Provider]) -> Option<Basemap> {
    // ax.axis() in EPSG:4326 -> _reproj_bb to EPSG:3857 -> bounds2img back to lon/lat
    let (l, b) = merc(frame.xlim.0, frame.ylim.0);
    let (r, t) = merc(frame.xlim.1, frame.ylim.1);
    let (w, s) = sm2ll(l, b);
    let (e, n) = sm2ll(r, t);
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    'providers: for p in providers {
        let Some(z) = zoom_for(w, s, e, n, p) else {
            continue;
        };
        // mercantile.tiles (no antimeridian split: a v3 AOI never crosses it)
        let (ws, ss, es, ns) = (
            w.max(-180.0),
            s.max(-85.051129),
            e.min(180.0),
            n.min(85.051129),
        );
        let ul = tile(ws, ns, z);
        let lr = tile(es - LL_EPSILON, ss + LL_EPSILON, z);
        let mut tiles = Vec::new();
        for x in ul.0..=lr.0 {
            for y in ul.1..=lr.1 {
                tiles.push((x, y));
            }
        }
        let urls: Vec<String> = tiles
            .iter()
            .map(|(x, y)| {
                p.url
                    .replace("{z}", &z.to_string())
                    .replace("{x}", &x.to_string())
                    .replace("{y}", &y.to_string())
            })
            .collect();
        let mut images = Vec::with_capacity(urls.len());
        for u in &urls {
            match fetch(&client, cache, u) {
                Some(img) => images.push(img),
                None => continue 'providers,
            }
        }
        // _merge_tiles
        let (th, tw) = (images[0].height() as usize, images[0].width() as usize);
        let (nx, ny) = ((lr.0 - ul.0 + 1) as usize, (lr.1 - ul.1 + 1) as usize);
        let (mw, mh) = (tw * nx, th * ny);
        let mut mosaic = vec![0u8; mw * mh * 4];
        for ((x, y), img) in tiles.iter().zip(&images) {
            let (ox, oy) = ((x - ul.0) as usize * tw, (y - ul.1) as usize * th);
            for (row, chunk) in img
                .as_raw()
                .chunks(img.width() as usize * 4)
                .enumerate()
                .take(th)
            {
                let start = ((oy + row) * mw + ox) * 4;
                let n = chunk.len().min(tw * 4);
                mosaic[start..start + n].copy_from_slice(&chunk[..n]);
            }
        }
        let (west, _, _, north) = tile_bounds(ul.0, ul.1, z);
        let (_, south, east, _) = tile_bounds(lr.0, lr.1, z);
        let (min_x, min_y) = merc(west, south);
        let (max_x, max_y) = merc(east, north);
        // warp_tiles: from_origin(minX - resX/2, maxY + resY/2, resX, resY)
        let res_x = (max_x - min_x) / mw as f64;
        let res_y = (max_y - min_y) / mh as f64;
        let (ox, oy) = (min_x - res_x / 2.0, max_y + res_y / 2.0);

        let (fw, fh) = (frame.width as usize, frame.height as usize);
        let mut rgba = vec![0u8; fw * fh * 4];
        for py in 0..fh {
            for px in 0..fw {
                let (lon, lat) = frame.to_data(px as f64 + 0.5, py as f64 + 0.5);
                let (mx, my) = merc(lon, lat);
                let c = ((mx - ox) / res_x - 0.5).clamp(0.0, (mw - 1) as f64);
                let r = ((oy - my) / res_y - 0.5).clamp(0.0, (mh - 1) as f64);
                let (c0, r0) = (c.floor() as usize, r.floor() as usize);
                let (c1, r1) = ((c0 + 1).min(mw - 1), (r0 + 1).min(mh - 1));
                let (fx, fy) = (c - c0 as f64, r - r0 as f64);
                let at = |cc: usize, rr: usize, k: usize| mosaic[(rr * mw + cc) * 4 + k] as f64;
                let o = (py * fw + px) * 4;
                for k in 0..3 {
                    let v = at(c0, r0, k) * (1.0 - fx) * (1.0 - fy)
                        + at(c1, r0, k) * fx * (1.0 - fy)
                        + at(c0, r1, k) * (1.0 - fx) * fy
                        + at(c1, r1, k) * fx * fy;
                    rgba[o + k] = v.round() as u8;
                }
                rgba[o + 3] = 255;
            }
        }
        return Some(Basemap {
            zoom: z,
            provider_url: p.url.to_string(),
            urls,
            rgba,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_cache_layout() {
        let c = Path::new("c");
        assert_eq!(
            tile_file(c, "https://tile.openstreetmap.org/13/6954/3904.png"),
            c.join("tile.openstreetmap.org")
                .join("13")
                .join("6954")
                .join("3904.png")
        );
        assert_eq!(
            tile_file(
                c,
                "https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/tile/13/3904/6954"
            ),
            c.join("server.arcgisonline.com")
                .join("13")
                .join("6954")
                .join("3904.png")
        );
    }

    #[test]
    fn mercantile_tiles() {
        assert_eq!(tile(0.0, 0.0, 1), (1, 1));
        let (w, s, e, n) = tile_bounds(1, 1, 1);
        assert!(
            (w - 0.0).abs() < 1e-12 && (e - 180.0).abs() < 1e-12 && n.abs() < 1e-12 && s < -85.0
        );
    }
}
