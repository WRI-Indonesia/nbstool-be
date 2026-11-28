//! The `affine` package (2.4.0) operations the v3 code uses, with its exact float operation order.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    /// `Affine.from_gdal(c, a, b, f, d, e)`
    pub fn from_gdal(gt: [f64; 6]) -> Self {
        Self {
            a: gt[1],
            b: gt[2],
            c: gt[0],
            d: gt[4],
            e: gt[5],
            f: gt[3],
        }
    }

    pub fn to_gdal(&self) -> [f64; 6] {
        [self.c, self.a, self.b, self.f, self.d, self.e]
    }

    pub fn translation(x: f64, y: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: x,
            d: 0.0,
            e: 1.0,
            f: y,
        }
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: sy,
            f: 0.0,
        }
    }

    /// `self * other` for two transforms.
    pub fn compose(&self, o: &Affine) -> Self {
        Self {
            a: self.a * o.a + self.b * o.d,
            b: self.a * o.b + self.b * o.e,
            c: self.a * o.c + self.b * o.f + self.c,
            d: self.d * o.a + self.e * o.d,
            e: self.d * o.b + self.e * o.e,
            f: self.d * o.c + self.e * o.f + self.f,
        }
    }

    /// `self * (x, y)`
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x * self.a + y * self.b + self.c,
            x * self.d + y * self.e + self.f,
        )
    }

    /// `~self`. The v3 transforms are never degenerate (they come from GDAL grids).
    pub fn inverse(&self) -> Self {
        let idet = 1.0 / (self.a * self.e - self.b * self.d);
        let ra = self.e * idet;
        let rb = -self.b * idet;
        let rd = -self.d * idet;
        let re = self.a * idet;
        Self {
            a: ra,
            b: rb,
            c: -self.c * ra - self.f * rb,
            d: rd,
            e: re,
            f: -self.c * rd - self.f * re,
        }
    }
}

/// `rasterio.transform.from_bounds`
pub fn from_bounds(
    west: f64,
    south: f64,
    east: f64,
    north: f64,
    width: usize,
    height: usize,
) -> Affine {
    Affine::translation(west, north).compose(&Affine::scale(
        (east - west) / width as f64,
        (south - north) / height as f64,
    ))
}
