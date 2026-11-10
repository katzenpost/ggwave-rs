use crate::math;
use crate::reed_solomon::{self, ecc_bytes_for_length};
use crate::resampler::Resampler;
use crate::{dss_magic, Config, Error, Parameters, Protocol, BITS_IN_MARKER};
use alloc::{vec, vec::Vec};
use core::f64::consts::PI;

/// Turns payloads into ggwave waveforms.
pub struct Encoder {
    cfg: Config,
}

impl Encoder {
    /// Builds an encoder; fails if `params` are out of range.
    pub fn new(params: Parameters) -> Result<Encoder, Error> {
        Ok(Encoder {
            cfg: Config::new(&params)?,
        })
    }

    /// Encodes `payload` with `protocol` at `volume` (0 to 100; C ggwave's
    /// default is 10) and returns mono `f32` samples in -1..1 at
    /// `sample_rate_out`, the same samples C ggwave produces for
    /// `GGWAVE_SAMPLE_FORMAT_F32`.
    ///
    /// In fixed-length mode a shorter payload is zero-padded, as in C.
    pub fn encode(
        &self,
        payload: &[u8],
        protocol: Protocol,
        volume: u8,
    ) -> Result<Vec<f32>, Error> {
        let c = &self.cfg;
        if volume > 100 {
            return Err(Error::InvalidVolume(volume));
        }
        if payload.is_empty() {
            return Err(Error::EmptyPayload);
        }
        if payload.len() > c.max_length() {
            return Err(Error::PayloadTooLong {
                len: payload.len(),
                max: c.max_length(),
            });
        }
        let p = protocol.params();
        if p.extra == 2 && c.fixed_len.is_none() {
            return Err(Error::MonoToneNeedsFixedLength);
        }

        let data_length = c.fixed_len.unwrap_or(payload.len());
        let send_volume = (volume as f64 / 100.0) as f32;
        let data: Vec<u8> = (0..data_length)
            .map(|i| payload.get(i).copied().unwrap_or(0) ^ if c.dss { dss_magic(i) } else { 0 })
            .collect();

        let mut encoded = Vec::new();
        if c.fixed_len.is_none() {
            encoded = reed_solomon::encode(&[data_length as u8], c.encoded_data_offset - 1);
        }
        encoded.extend(reed_solomon::encode(
            &data,
            ecc_bytes_for_length(data_length),
        ));

        let bpt = p.bytes_per_tx as usize;
        let fpt = p.frames_per_tx;
        let n_txs = encoded.len().div_ceil(bpt);
        // the last chunk reads past the coded bytes; C reads zeroed buffer space there
        encoded.resize(n_txs * bpt, 0);
        let total_data_frames = p.extra * n_txs as i32 * fpt;

        let spf = c.spf;
        let n_bits = (16 * bpt).max(BITS_IN_MARKER as usize);
        let cur_ihz = 1.0 / c.hz_per_sample as f64;
        let tone = |freq: f64, phase: f64| -> Vec<f32> {
            (0..spf)
                .map(|i| {
                    math::sin(
                        (2.0 * PI) * (i as f64 * c.isamples_per_frame as f64) * (freq * cur_ihz)
                            + phase,
                    ) as f32
                })
                .collect()
        };
        let mut bit1 = Vec::with_capacity(n_bits);
        let mut bit0 = Vec::with_capacity(n_bits);
        for k in 0..n_bits {
            let freq = c.bit_freq(p, k as i32);
            let phase = (PI * k as f64) / (8 * bpt) as f64;
            bit1.push(tone(freq, phase));
            bit0.push(tone(freq + c.hz_per_sample as f64, phase));
        }

        let nm = c.n_marker_frames;
        // C resamples (even at factor 1) when either side's rate differs
        let need_resampling =
            c.sample_rate_out != c.sample_rate || c.sample_rate_inp != c.sample_rate;
        let mut resampler = need_resampling.then(Resampler::new);
        let factor = c.sample_rate / c.sample_rate_out;
        let n_frames = 2 * nm + total_data_frames;
        let mut out = Vec::with_capacity((n_frames as f32 * spf as f32 / factor) as usize + spf);
        let mut output = vec![0.0f32; spf];
        let mut bits = vec![false; 2 * 16 * bpt];

        for frame_id in 0..n_frames {
            output.fill(0.0);
            let n_freq;
            if frame_id < nm {
                n_freq = BITS_IN_MARKER;
                for i in 0..BITS_IN_MARKER as usize {
                    let src = if i % 2 == 0 { &bit1[i] } else { &bit0[i] };
                    add_amplitude_smooth(src, &mut output, send_volume, frame_id, nm);
                }
            } else if frame_id < nm + total_data_frames {
                let d = frame_id - nm;
                let cycle_mod = d % fpt;
                let offset = (d / fpt) as usize * bpt;
                bits.fill(false);
                for j in 0..bpt {
                    if p.extra == 1 {
                        let b = encoded[offset + j];
                        bits[2 * j * 16 + (b & 15) as usize] = true;
                        bits[(2 * j + 1) * 16 + (b >> 4) as usize] = true;
                    } else {
                        let b = encoded[offset / 2 + j];
                        let nibble = if offset % 2 == 0 { b & 15 } else { b >> 4 };
                        bits[2 * j * 16 + nibble as usize] = true;
                    }
                }
                let mut n = 0;
                for (k, _) in bits.iter().enumerate().filter(|(_, &b)| b) {
                    n += 1;
                    let src = if k % 2 == 1 {
                        &bit0[k / 2]
                    } else {
                        &bit1[k / 2]
                    };
                    add_amplitude_smooth(src, &mut output, send_volume, cycle_mod, fpt);
                }
                n_freq = n;
            } else {
                n_freq = BITS_IN_MARKER;
                let f_id = frame_id - (nm + total_data_frames);
                for i in 0..BITS_IN_MARKER as usize {
                    let src = if i % 2 == 0 { &bit0[i] } else { &bit1[i] };
                    add_amplitude_smooth(src, &mut output, send_volume, f_id, nm);
                }
            }

            let scale = 1.0f32 / n_freq.max(1) as f32;
            for s in &mut output {
                *s *= scale;
            }
            match &mut resampler {
                Some(r) => r.resample(factor, &output, &mut out),
                None => out.extend_from_slice(&output),
            }
        }
        Ok(out)
    }
}

/// Converts `f32` samples to `i16` the way C ggwave produces
/// `GGWAVE_SAMPLE_FORMAT_I16` output (`32768 * x`, truncated).
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&x| (32768.0 * x) as i32 as i16)
        .collect()
}

// Adds `src` scaled by `scalar` with a linear fade over the first and last 15%
// of a `n_per_cycle`-frame symbol.
fn add_amplitude_smooth(
    src: &[f32],
    dst: &mut [f32],
    scalar: f32,
    cycle_mod: i32,
    n_per_cycle: i32,
) {
    let final_id = dst.len() as i32;
    let n_total = n_per_cycle * final_id;
    let frac = 0.15f32;
    let ds = frac * n_total as f32;
    let ids = 1.0f32 / ds;
    let n_begin = (frac * n_total as f32) as i32;
    let n_end = ((1.0f32 - frac) * n_total as f32) as i32;

    for (i, (d, &s)) in dst.iter_mut().zip(src).enumerate() {
        let k = (cycle_mod * final_id + i as i32) as f32;
        if k < n_begin as f32 {
            *d += scalar * s * (k * ids);
        } else if k > n_end as f32 {
            *d += scalar * s * ((n_total as f32 - k) * ids);
        } else {
            *d += scalar * s;
        }
    }
}
