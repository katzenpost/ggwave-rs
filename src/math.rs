//! Float functions core lacks on stable: std's under the `std` feature, libm otherwise.

#[cfg(feature = "std")]
mod imp {
    pub fn sin(x: f64) -> f64 {
        x.sin()
    }
    pub fn cos(x: f64) -> f64 {
        x.cos()
    }
    pub fn round(x: f64) -> f64 {
        x.round()
    }
    pub fn roundf(x: f32) -> f32 {
        x.round()
    }
    pub fn abs(x: f64) -> f64 {
        x.abs()
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    pub use libm::{cos, fabs as abs, round, roundf, sin};
}

pub(crate) use imp::*;
