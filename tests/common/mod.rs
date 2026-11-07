#![allow(dead_code)]

use ggwave::{Parameters, Protocol};

/// Deterministic test payload of `len` bytes.
pub fn payload(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed) ^ 0x5a)
        .collect()
}

pub fn params(fixed: Option<usize>, rate_inp: f32, rate_out: f32) -> Parameters {
    Parameters {
        payload_length: fixed,
        sample_rate_inp: rate_inp,
        sample_rate_out: rate_out,
        ..Parameters::default()
    }
}

/// Adds uniform noise in +-level/2, like C ggwave's test-ggwave.cpp, from a fixed seed.
pub fn add_noise(samples: &mut [f32], level: f32, seed: u64) {
    let mut x = seed | 1;
    for s in samples {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let r = (x >> 40) as f32 / (1u64 << 24) as f32;
        *s = (*s + (r - 0.5) * level).clamp(-1.0, 1.0);
    }
}

/// Silence before and after, as a real recording would have.
pub fn pad(samples: &[f32], lead: usize, tail: usize) -> Vec<f32> {
    let mut v = vec![0.0; lead];
    v.extend_from_slice(samples);
    v.resize(v.len() + tail, 0.0);
    v
}

/// Protocols usable with the given mode (MT needs fixed length).
pub fn protocols(fixed: bool) -> Vec<Protocol> {
    Protocol::ALL
        .into_iter()
        .filter(|p| fixed || !p.is_mono_tone())
        .collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}
