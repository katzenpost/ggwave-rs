//! A pure safe Rust port of [ggwave](https://github.com/ggerganov/ggwave),
//! the data-over-sound modem by Georgi Gerganov.
//!
//! The waveform format, Reed-Solomon coding, markers and decoder follow C
//! ggwave at commit `060aec73dd7123ccac200442f75bdc7369795ffe`, so this crate
//! and C ggwave can talk to each other over sound.
//!
//! ```
//! use ggwave::{Decoder, Encoder, Parameters, Protocol};
//!
//! let params = Parameters::default(); // 48 kHz, variable-length payloads
//! let samples = Encoder::new(params.clone())?.encode(b"hello", Protocol::AudibleFast, 50)?;
//!
//! let mut decoder = Decoder::new(params)?;
//! assert_eq!(decoder.push(&samples).as_deref(), Some(&b"hello"[..]));
//! # Ok::<(), ggwave::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod decoder;
mod encoder;
mod fft;
mod protocol;
mod reed_solomon;
mod resampler;

pub use decoder::Decoder;
pub use encoder::{to_i16, Encoder};
pub use protocol::Protocol;

/// Largest variable-length payload, in bytes.
pub const MAX_LENGTH_VARIABLE: usize = 140;
/// Largest fixed-length payload, in bytes.
pub const MAX_LENGTH_FIXED: usize = 64;

/// Modem parameters, mirroring `ggwave_Parameters`.
///
/// Both ends must agree on `payload_length`, and on `sample_rate` /
/// `samples_per_frame` (their ratio sets the tone spacing). `sample_rate_inp`
/// and `sample_rate_out` are the rates of the audio you pass in and get out;
/// when they differ from `sample_rate` the samples are resampled as C ggwave does.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameters {
    /// `None` for variable-length payloads (with start/end markers), or the
    /// fixed payload length in bytes (no markers; up to [`MAX_LENGTH_FIXED`]).
    pub payload_length: Option<usize>,
    /// Sample rate of audio given to [`Decoder::push`].
    pub sample_rate_inp: f32,
    /// Sample rate of audio returned by [`Encoder::encode`].
    pub sample_rate_out: f32,
    /// Operating sample rate of the modem.
    pub sample_rate: f32,
    /// Samples per frame at the operating rate: a power of two, 128 to 1024.
    pub samples_per_frame: usize,
    /// Marker detection threshold.
    pub sound_marker_threshold: f32,
    /// XOR the payload with ggwave's DSS sequence (`GGWAVE_OPERATING_MODE_USE_DSS`).
    pub dss: bool,
}

impl Default for Parameters {
    /// The values of `ggwave_getDefaultParameters()`.
    fn default() -> Self {
        Parameters {
            payload_length: None,
            sample_rate_inp: 48000.0,
            sample_rate_out: 48000.0,
            sample_rate: 48000.0,
            samples_per_frame: 1024,
            sound_marker_threshold: 3.0,
            dss: false,
        }
    }
}

/// Errors from building an encoder or decoder, or from encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A [`Parameters`] field is out of range; the text names it.
    InvalidParameters(&'static str),
    /// The payload is empty.
    EmptyPayload,
    /// The payload is longer than the mode allows.
    PayloadTooLong {
        /// Given length.
        len: usize,
        /// Largest allowed length.
        max: usize,
    },
    /// Volume must be 0 to 100.
    InvalidVolume(u8),
    /// Mono-tone (MT) protocols need a fixed payload length.
    MonoToneNeedsFixedLength,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidParameters(what) => write!(f, "invalid parameters: {what}"),
            Error::EmptyPayload => write!(f, "empty payload"),
            Error::PayloadTooLong { len, max } => {
                write!(f, "payload of {len} bytes is longer than {max}")
            }
            Error::InvalidVolume(v) => write!(f, "volume {v} is not in 0..=100"),
            Error::MonoToneNeedsFixedLength => {
                write!(f, "mono-tone protocols need a fixed payload length")
            }
        }
    }
}

impl std::error::Error for Error {}

const MARKER_FRAMES: i32 = 16;
const ENCODED_DATA_OFFSET: usize = 3;
const BITS_IN_MARKER: i32 = 16;

/// Values derived from [`Parameters`], named as in ggwave.cpp.
#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub fixed_len: Option<usize>,
    pub sample_rate_inp: f32,
    pub sample_rate_out: f32,
    pub sample_rate: f32,
    pub spf: usize,
    pub hz_per_sample: f32,
    pub ihz_per_sample: f32,
    pub isamples_per_frame: f32,
    pub freq_delta_hz: f32,
    pub n_marker_frames: i32,
    pub encoded_data_offset: usize,
    pub threshold: f32,
    pub dss: bool,
}

impl Config {
    pub fn new(p: &Parameters) -> Result<Config, Error> {
        let rate_ok = |r: f32| (1000.0..=96000.0).contains(&r);
        if !rate_ok(p.sample_rate) || !rate_ok(p.sample_rate_inp) || !rate_ok(p.sample_rate_out) {
            return Err(Error::InvalidParameters(
                "sample rates must be 1000 to 96000 Hz",
            ));
        }
        if !p.samples_per_frame.is_power_of_two() || !(128..=1024).contains(&p.samples_per_frame) {
            return Err(Error::InvalidParameters(
                "samples_per_frame must be a power of two from 128 to 1024",
            ));
        }
        if let Some(len) = p.payload_length {
            if len == 0 || len > MAX_LENGTH_FIXED {
                return Err(Error::InvalidParameters("payload_length must be 1 to 64"));
            }
        }
        let hz_per_sample = p.sample_rate / p.samples_per_frame as f32;
        let fixed = p.payload_length.is_some();
        Ok(Config {
            fixed_len: p.payload_length,
            sample_rate_inp: p.sample_rate_inp,
            sample_rate_out: p.sample_rate_out,
            sample_rate: p.sample_rate,
            spf: p.samples_per_frame,
            hz_per_sample,
            ihz_per_sample: 1.0 / hz_per_sample,
            isamples_per_frame: 1.0 / p.samples_per_frame as f32,
            freq_delta_hz: 2.0 * hz_per_sample,
            n_marker_frames: if fixed { 0 } else { MARKER_FRAMES },
            encoded_data_offset: if fixed { 0 } else { ENCODED_DATA_OFFSET },
            threshold: p.sound_marker_threshold,
            dss: p.dss,
        })
    }

    pub fn max_length(&self) -> usize {
        self.fixed_len.unwrap_or(MAX_LENGTH_VARIABLE)
    }

    /// `GGWave::bitFreq`: float arithmetic, widened to double at the end.
    pub fn bit_freq(&self, p: protocol::ProtocolParams, bit: i32) -> f64 {
        (self.hz_per_sample * p.freq_start as f32 + self.freq_delta_hz * bit as f32) as f64
    }
}

// XORed into payloads when DSS is on (kDSSMagic in ggwave.cpp).
const DSS_MAGIC: [u8; 64] = [
    0x96, 0x9f, 0xb4, 0xaf, 0x1b, 0x91, 0xde, 0xc5, 0x45, 0x75, 0xe8, 0x2e, 0x0f, 0x32, 0x4a, 0x5f,
    0xb4, 0x56, 0x95, 0xcb, 0x7f, 0x6a, 0x54, 0x6a, 0x48, 0xf2, 0x0b, 0x7b, 0xcd, 0xfb, 0x93, 0x6d,
    0x3c, 0x77, 0x5e, 0xc3, 0x33, 0x47, 0xc0, 0xf1, 0x71, 0x32, 0x33, 0x27, 0x35, 0x68, 0x47, 0x1f,
    0x4e, 0xac, 0x23, 0x42, 0x5f, 0x00, 0x37, 0xa4, 0x50, 0x6d, 0x48, 0x24, 0x91, 0x7c, 0xa1, 0x4e,
];

pub(crate) fn dss_magic(i: usize) -> u8 {
    DSS_MAGIC[i % DSS_MAGIC.len()]
}
