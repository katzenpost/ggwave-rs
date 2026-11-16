//! Float functions core lacks on stable: std's under the `std` feature, the platform C
//! library's under `extern-libm` (e.g. newlib on ESP-IDF), the libm crate otherwise.

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

#[cfg(all(not(feature = "std"), feature = "extern-libm"))]
#[allow(unsafe_code)]
mod imp {
    mod c {
        extern "C" {
            pub fn sin(x: f64) -> f64;
            pub fn cos(x: f64) -> f64;
            pub fn round(x: f64) -> f64;
            pub fn roundf(x: f32) -> f32;
            pub fn fabs(x: f64) -> f64;
        }
    }
    // SAFETY: pure C maths functions, defined for every input
    pub fn sin(x: f64) -> f64 {
        unsafe { c::sin(x) }
    }
    pub fn cos(x: f64) -> f64 {
        unsafe { c::cos(x) }
    }
    pub fn round(x: f64) -> f64 {
        unsafe { c::round(x) }
    }
    pub fn roundf(x: f32) -> f32 {
        unsafe { c::roundf(x) }
    }
    pub fn abs(x: f64) -> f64 {
        unsafe { c::fabs(x) }
    }
}

#[cfg(all(not(feature = "std"), not(feature = "extern-libm")))]
mod imp {
    pub use libm::{cos, fabs as abs, round, roundf, sin};
}

pub(crate) use imp::*;
