//! Live interop against C ggwave 060aec7, through the tool built by
//! `tools/build-c-ggwave.sh`. Point `GGWAVE_C` at the binary to run these;
//! without it they are skipped so `cargo test` needs no C toolchain.
//! CI always sets it.

mod common;

use common::*;
use ggwave::{to_i16, Decoder, Encoder, Protocol};
use std::io::Write;
use std::process::{Command, Stdio};

fn tool() -> Option<String> {
    let t = std::env::var("GGWAVE_C").ok();
    if t.is_none() {
        eprintln!("GGWAVE_C not set; skipping live C ggwave interop");
    }
    t
}

fn mode_args(fixed: Option<usize>, rate: f32) -> Vec<String> {
    let mut a = vec!["-r".to_string(), rate.to_string()];
    if let Some(l) = fixed {
        a.extend(["-l".to_string(), l.to_string()]);
    }
    a
}

fn c_encode(
    tool: &str,
    payload: &[u8],
    p: Protocol,
    volume: u8,
    fixed: Option<usize>,
    rate_out: f32,
) -> Vec<f32> {
    let out = Command::new(tool)
        .args([
            "encode",
            "-p",
            &(p as u8).to_string(),
            "-v",
            &volume.to_string(),
        ])
        .args(mode_args(fixed, rate_out))
        .arg(hex(payload))
        .output()
        .expect("run C ggwave");
    assert!(out.status.success(), "C encode failed");
    out.stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn c_decode(tool: &str, samples: &[f32], fixed: Option<usize>, rate_inp: f32) -> Vec<Vec<u8>> {
    let mut child = Command::new(tool)
        .arg("decode")
        .args(mode_args(fixed, rate_inp))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run C ggwave");
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(unhex)
        .collect()
}

// Same 4096-sample chunks as the C tool, so per-chunk results line up with C's.
fn rust_decode(samples: &[f32], fixed: Option<usize>, rate_inp: f32) -> Vec<Vec<u8>> {
    let mut d = Decoder::new(params(fixed, rate_inp, 48000.0)).unwrap();
    samples.chunks(4096).filter_map(|ch| d.push(ch)).collect()
}

const LENGTHS_VARIABLE: [usize; 5] = [1, 5, 16, 32, 140];
const LENGTHS_FIXED: [usize; 4] = [1, 8, 32, 64];
const NOISE: f32 = 0.1;
// a microphone-like noise floor; on exact digital silence C ggwave misses some
// DT messages (its marker thresholds compare against zero bins), and so do we
const QUIET: f32 = 0.002;

fn cases() -> Vec<(Option<usize>, Protocol, usize)> {
    let mut v = Vec::new();
    for p in protocols(false) {
        for len in LENGTHS_VARIABLE {
            v.push((None, p, len));
        }
    }
    for p in protocols(true) {
        for len in LENGTHS_FIXED {
            v.push((Some(len), p, len));
        }
    }
    v
}

#[test]
fn rust_encode_matches_c_samples() {
    let Some(tool) = tool() else { return };
    for (fixed, p, len) in cases() {
        let data = payload(len, len as u8);
        let c = c_encode(&tool, &data, p, 50, fixed, 48000.0);
        let r = Encoder::new(params(fixed, 48000.0, 48000.0))
            .unwrap()
            .encode(&data, p, 50)
            .unwrap();
        assert_eq!(r.len(), c.len(), "{p:?} len {len} fixed {fixed:?}");
        let max = r
            .iter()
            .zip(&c)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            max < 1e-6,
            "{p:?} len {len} fixed {fixed:?}: max diff {max}"
        );
        assert!(
            to_i16(&r)
                .iter()
                .zip(to_i16(&c))
                .filter(|(a, b)| a != &b)
                .count()
                <= r.len() / 1000
        );
    }
}

// 1-5 byte payloads carry only 2-4 ECC bytes and C ggwave itself sometimes
// miscorrects them (the equivalence asserts above show we do exactly the same);
// longer payloads must always decode.
const SHORT: usize = 5;

fn check_found(got: &[Vec<u8>], data: &[u8], misses: &mut Vec<String>, what: String) {
    if !got.iter().any(|g| g == data) {
        assert!(
            data.len() <= SHORT,
            "{what}: payload not decoded, got {got:?}"
        );
        misses.push(what);
    }
}

fn assert_short_misses(misses: &[String]) {
    let short_trials = cases().iter().filter(|c| c.2 <= SHORT).count() * TRIALS.len();
    assert!(
        misses.len() * 10 <= short_trials,
        "too many short-payload misses: {misses:?}"
    );
}

// Noise levels and seeds each case is tried at.
const TRIALS: [(f32, u64); 3] = [(QUIET, 0), (NOISE, 1), (NOISE, 2)];

/// Rust-encoded audio decoded by C: C must return exactly what it returns for
/// its own waveform with the same noise, and find the payload.
#[test]
fn rust_encode_c_decode() {
    let Some(tool) = tool() else { return };
    let mut misses = Vec::new();
    for (fixed, p, len) in cases() {
        let data = payload(len, 3);
        let r = Encoder::new(params(fixed, 48000.0, 48000.0))
            .unwrap()
            .encode(&data, p, 50)
            .unwrap();
        let c = c_encode(&tool, &data, p, 50, fixed, 48000.0);
        for (noise, seed) in TRIALS {
            let (mut nr, mut nc) = (pad(&r, 12345, 48000), pad(&c, 12345, 48000));
            add_noise(&mut nr, noise, seed + len as u64);
            add_noise(&mut nc, noise, seed + len as u64);
            let got = c_decode(&tool, &nr, fixed, 48000.0);
            assert_eq!(
                got,
                c_decode(&tool, &nc, fixed, 48000.0),
                "{p:?} len {len} fixed {fixed:?}"
            );
            check_found(
                &got,
                &data,
                &mut misses,
                format!("{p:?}/{len}/{fixed:?}/{noise}/{seed}"),
            );
        }
    }
    assert_short_misses(&misses);
}

/// C-encoded audio decoded by Rust: Rust must return exactly what C's own
/// decoder returns for the same samples, and find the payload.
#[test]
fn c_encode_rust_decode() {
    let Some(tool) = tool() else { return };
    let mut misses = Vec::new();
    for (fixed, p, len) in cases() {
        let data = payload(len, 9);
        let c = c_encode(&tool, &data, p, 50, fixed, 48000.0);
        for (noise, seed) in TRIALS {
            let mut x = pad(&c, 4321, 48000);
            add_noise(&mut x, noise, seed + len as u64);
            let got = rust_decode(&x, fixed, 48000.0);
            assert_eq!(
                got,
                c_decode(&tool, &x, fixed, 48000.0),
                "{p:?} len {len} fixed {fixed:?}"
            );
            check_found(
                &got,
                &data,
                &mut misses,
                format!("{p:?}/{len}/{fixed:?}/{noise}/{seed}"),
            );
        }
    }
    assert_short_misses(&misses);
}

/// The T-Deck voucher case: AUDIBLE_FAST, variable length, 32 bytes, through
/// heavier noise and ragged chunking, in both directions.
#[test]
fn audible_fast_32_byte_voucher() {
    let Some(tool) = tool() else { return };
    let enc = Encoder::new(params(None, 48000.0, 48000.0)).unwrap();
    for seed in 0..8u8 {
        let data = payload(32, seed.wrapping_mul(29));
        let r = enc.encode(&data, Protocol::AudibleFast, 50).unwrap();
        let c = c_encode(&tool, &data, Protocol::AudibleFast, 50, None, 48000.0);
        for noise in [0.1, 0.3, 0.5] {
            let mut nr = pad(&r, 1000 + 97 * seed as usize, 48000);
            add_noise(&mut nr, noise, seed as u64);
            assert_eq!(
                c_decode(&tool, &nr, None, 48000.0),
                vec![data.clone()],
                "C, seed {seed} noise {noise}"
            );

            let mut nc = pad(&c, 1000 + 97 * seed as usize, 48000);
            add_noise(&mut nc, noise, seed as u64 + 100);
            let mut d = Decoder::new(params(None, 48000.0, 48000.0)).unwrap();
            let got: Vec<_> = nc
                .chunks(333 + seed as usize)
                .filter_map(|ch| d.push(ch))
                .collect();
            assert_eq!(got, vec![data.clone()], "Rust, seed {seed} noise {noise}");
        }
    }
}

/// Other playback/capture rates go through ggwave's resampler on both sides.
#[test]
fn resampled_rates_both_ways() {
    let Some(tool) = tool() else { return };
    let data = payload(32, 1);
    for rate in [16000.0, 44100.0] {
        for p in [Protocol::AudibleFast, Protocol::DtFast] {
            let r = Encoder::new(params(None, 48000.0, rate))
                .unwrap()
                .encode(&data, p, 50)
                .unwrap();
            let c = c_encode(&tool, &data, p, 50, None, rate);
            let max = r
                .iter()
                .zip(&c)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert_eq!(r.len(), c.len());
            assert!(max < 1e-5, "{p:?} at {rate}: max diff {max}");

            let mut noisy = pad(&r, 1000, rate as usize);
            add_noise(&mut noisy, NOISE, 5);
            assert!(
                c_decode(&tool, &noisy, None, rate).contains(&data),
                "C decode {p:?} at {rate}"
            );
            let mut noisy = pad(&c, 1000, rate as usize);
            add_noise(&mut noisy, NOISE, 6);
            let got = rust_decode(&noisy, None, rate);
            assert!(got.contains(&data), "Rust decode {p:?} at {rate}: {got:?}");
        }
    }
}

/// GGWAVE_OPERATING_MODE_USE_DSS on both sides.
#[test]
fn dss_both_ways() {
    let Some(tool) = tool() else { return };
    for fixed in [None, Some(32)] {
        let data = payload(32, 77);
        let p = ggwave::Parameters {
            dss: true,
            ..params(fixed, 48000.0, 48000.0)
        };
        let r = Encoder::new(p.clone())
            .unwrap()
            .encode(&data, Protocol::AudibleFast, 50)
            .unwrap();
        let mut args = vec!["encode", "-d", "-p", "1", "-v", "50"];
        let l = fixed.map(|l| l.to_string()).unwrap_or_default();
        if fixed.is_some() {
            args.extend(["-l", &l]);
        }
        let h = hex(&data);
        args.push(&h);
        let out = Command::new(&tool).args(&args).output().unwrap();
        let c: Vec<f32> = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let max = r
            .iter()
            .zip(&c)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            r.len() == c.len() && max < 1e-6,
            "DSS encode differs, fixed {fixed:?}"
        );

        let mut x = pad(&c, 3000, 48000);
        add_noise(&mut x, NOISE, 1);
        let mut d = Decoder::new(p).unwrap();
        let got: Vec<_> = x.chunks(4096).filter_map(|ch| d.push(ch)).collect();
        assert!(got.contains(&data), "DSS decode, fixed {fixed:?}: {got:?}");

        let mut dec = Command::new(&tool);
        dec.args(["decode", "-d"]);
        if fixed.is_some() {
            dec.args(["-l", &l]);
        }
        let mut child = dec
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut x = pad(&r, 3000, 48000);
        add_noise(&mut x, NOISE, 2);
        let bytes: Vec<u8> = x.iter().flat_map(|s| s.to_le_bytes()).collect();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        assert!(
            out.lines().any(|l| l == h),
            "C DSS decode, fixed {fixed:?}: {out}"
        );
    }
}
