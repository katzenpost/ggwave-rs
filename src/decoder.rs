use crate::fft::Fft;
use crate::protocol::ProtocolParams;
use crate::reed_solomon::{self, ecc_bytes_for_length};
use crate::resampler::Resampler;
use crate::{dss_magic, Config, Error, Parameters, Protocol, BITS_IN_MARKER, MAX_LENGTH_VARIABLE};

const MAX_SPECTRUM_HISTORY: usize = 4;
const MAX_RECORDED_FRAMES: usize = 2048;
const STEPS_PER_FRAME: i32 = 16;
// input samples fed to the resampler per call; must exceed its 64-sample width
const RESAMPLE_CHUNK: usize = 1024;

/// Streaming ggwave receiver.
///
/// Feed it audio with [`push`](Decoder::push) in chunks of any size; it
/// returns a payload when one has been received.
pub struct Decoder {
    cfg: Config,
    protocols: Vec<(Protocol, ProtocolParams)>,
    fft: Fft,
    spectrum: Vec<f32>,
    frame: Vec<f32>,
    fft_in: Vec<f32>,
    pending: Vec<f32>,
    resampler: Option<(Resampler, Vec<f32>)>,
    encoded: Vec<u8>,
    // variable-length state
    receiving: bool,
    analyzing: bool,
    marker_freq_start: i32,
    recv_duration_frames: i32,
    frames_left_to_record: i32,
    frames_to_record: i32,
    history_id: usize,
    history: Vec<Vec<f32>>,
    average: Vec<f32>,
    recorded: Vec<f32>,
    // fixed-length state
    min_freq_start: usize,
    history_fixed: Vec<Vec<u8>>,
    history_id_fixed: usize,
}

impl Decoder {
    /// A decoder listening for every protocol.
    pub fn new(params: Parameters) -> Result<Decoder, Error> {
        Self::with_protocols(params, &Protocol::ALL)
    }

    /// A decoder listening only for `protocols` (C's `ggwave_rxToggleProtocol`).
    pub fn with_protocols(params: Parameters, protocols: &[Protocol]) -> Result<Decoder, Error> {
        let cfg = Config::new(&params)?;
        let spf = cfg.spf;
        // C keeps protocol order and reads garbage for bins past the frame; skip those instead
        let protocols: Vec<_> = Protocol::ALL
            .iter()
            .filter(|p| protocols.contains(p))
            .map(|&p| (p, p.params()))
            .filter(|(_, pp)| (pp.freq_start + 32 * pp.bytes_per_tx) as usize <= spf / 2)
            .collect();
        if protocols.is_empty() {
            return Err(Error::InvalidParameters(
                "no protocol fits samples_per_frame",
            ));
        }

        let max_length = cfg.max_length();
        let total_length = max_length + ecc_bytes_for_length(max_length);
        let history_fixed = match cfg.fixed_len {
            Some(_) => {
                let max_fpt = max_frames_per_tx(&protocols, false) as usize;
                vec![vec![0u8; spf]; total_length * max_fpt]
            }
            None => Vec::new(),
        };
        let min_freq_start = protocols
            .iter()
            .map(|(_, pp)| pp.freq_start as usize)
            .fold(spf, usize::min);
        let need_resampling =
            cfg.sample_rate_inp != cfg.sample_rate || cfg.sample_rate_out != cfg.sample_rate;

        Ok(Decoder {
            protocols,
            fft: Fft::new(spf),
            spectrum: vec![0.0; spf],
            frame: vec![0.0; spf],
            fft_in: vec![0.0; spf],
            pending: Vec::new(),
            resampler: need_resampling.then(|| (Resampler::new(), Vec::new())),
            encoded: vec![0; total_length + cfg.encoded_data_offset],
            receiving: false,
            analyzing: false,
            marker_freq_start: 0,
            recv_duration_frames: 0,
            frames_left_to_record: 0,
            frames_to_record: 0,
            history_id: 0,
            history: vec![vec![0.0; spf]; MAX_SPECTRUM_HISTORY],
            average: vec![0.0; spf],
            recorded: Vec::new(),
            min_freq_start,
            history_fixed,
            history_id_fixed: 0,
            cfg,
        })
    }

    /// Feeds mono `f32` samples (-1..1) at `sample_rate_inp`. Returns the
    /// payload if one was completed by these samples (the last one, if several).
    ///
    /// In fixed-length mode, as in C ggwave, the same transmission can be
    /// reported again on the following frames.
    pub fn push(&mut self, samples: &[f32]) -> Option<Vec<u8>> {
        let mut pending = std::mem::take(&mut self.pending);
        match &mut self.resampler {
            Some((resampler, input)) => {
                input.extend_from_slice(samples);
                let factor = self.cfg.sample_rate_inp / self.cfg.sample_rate;
                let mut start = 0;
                while input.len() - start >= RESAMPLE_CHUNK {
                    // C resets the resampler about once a minute while idle
                    if !self.receiving
                        && resampler.n_samples_total() as f32 > 60.0 * factor * self.cfg.sample_rate
                    {
                        resampler.reset();
                    }
                    resampler.resample(factor, &input[start..start + RESAMPLE_CHUNK], &mut pending);
                    start += RESAMPLE_CHUNK;
                }
                input.drain(..start);
            }
            None => pending.extend_from_slice(samples),
        }

        let spf = self.cfg.spf;
        let mut result = None;
        let mut start = 0;
        while pending.len() - start >= spf {
            self.frame.copy_from_slice(&pending[start..start + spf]);
            start += spf;
            let r = if self.cfg.fixed_len.is_some() {
                self.decode_fixed()
            } else {
                self.decode_variable()
            };
            if r.is_some() {
                result = r;
            }
        }
        pending.drain(..start);
        self.pending = pending;
        result
    }

    /// Feeds `i16` samples, scaled by 1/32768 as C ggwave does.
    pub fn push_i16(&mut self, samples: &[i16]) -> Option<Vec<u8>> {
        let f: Vec<f32> = samples
            .iter()
            .map(|&s| s as f32 * (1.0 / 32768.0))
            .collect();
        self.push(&f)
    }

    /// True between a start marker and the end of its analysis.
    pub fn is_receiving(&self) -> bool {
        self.receiving
    }

    fn marker_bin(&self, p: ProtocolParams, i: i32) -> usize {
        (self.cfg.bit_freq(p, i) * self.cfg.ihz_per_sample as f64).round() as usize
    }

    // true if every marker bit has the expected tone (`start`) or the inverted one
    fn marker_detected(&self, p: ProtocolParams, start: bool) -> bool {
        let s = &self.spectrum;
        let thr = self.cfg.threshold;
        (0..BITS_IN_MARKER).all(|i| {
            let bin = self.marker_bin(p, i);
            let (a, b) = (s[bin], thr * s[bin + 1]);
            match (i % 2 == 0, start) {
                (true, true) | (false, false) => a > b,
                (false, true) | (true, false) => a < b,
            }
        })
    }

    fn decode_variable(&mut self) -> Option<Vec<u8>> {
        let spf = self.cfg.spf;
        self.history[self.history_id].copy_from_slice(&self.frame);
        self.history_id = (self.history_id + 1) % MAX_SPECTRUM_HISTORY;

        if self.history_id == 0 || self.receiving {
            self.average.fill(0.0);
            for h in &self.history {
                for (a, &x) in self.average.iter_mut().zip(h) {
                    *a += x;
                }
            }
            let norm = 1.0f32 / MAX_SPECTRUM_HISTORY as f32;
            for a in &mut self.average {
                *a *= norm;
            }
            self.fft.power(&self.average, &mut self.spectrum);
        }

        if self.frames_left_to_record > 0 {
            if self.recorded.is_empty() {
                self.recorded = vec![0.0; MAX_RECORDED_FRAMES * spf];
            }
            let at = (self.frames_to_record - self.frames_left_to_record) as usize * spf;
            self.recorded[at..at + spf].copy_from_slice(&self.frame);
            self.frames_left_to_record -= 1;
            if self.frames_left_to_record <= 0 {
                self.analyzing = true;
            }
        }

        let mut result = None;
        if self.analyzing {
            result = self.analyze();
            self.frames_to_record = if result.is_some() { 0 } else { -1 };
            self.receiving = false;
            self.analyzing = false;
            self.spectrum.fill(0.0);
        }

        if !self.receiving {
            let found = self
                .protocols
                .iter()
                .find(|(_, p)| self.marker_detected(*p, true))
                .map(|(_, p)| p.freq_start);
            if let Some(freq_start) = found {
                self.marker_freq_start = freq_start;
                self.receiving = true;
                let max_len = MAX_LENGTH_VARIABLE + ecc_bytes_for_length(MAX_LENGTH_VARIABLE);
                // minBytesPerTx() in C is always 1
                self.recv_duration_frames = 2 * self.cfg.n_marker_frames
                    + max_frames_per_tx(&self.protocols, true) * (max_len as i32 + 1);
                self.frames_to_record = self.recv_duration_frames;
                self.frames_left_to_record = self.recv_duration_frames;
            }
        } else {
            let ended = self
                .protocols
                .iter()
                .any(|(_, p)| self.marker_detected(*p, false));
            if ended && self.frames_to_record > 1 {
                self.recv_duration_frames -= self.frames_left_to_record - 1;
                self.frames_left_to_record = 1;
            }
        }
        result
    }

    fn analyze(&mut self) -> Option<Vec<u8>> {
        let spf = self.cfg.spf;
        let step = spf / STEPS_PER_FRAME as usize;
        let nm = self.cfg.n_marker_frames;
        let offset = self.cfg.encoded_data_offset;

        for i in 0..self.protocols.len() {
            let p = self.protocols[i].1;
            if p.extra == 2 || p.freq_start != self.marker_freq_start {
                continue;
            }
            let bpt = p.bytes_per_tx as usize;
            let fpt = p.frames_per_tx;
            let base_bin = ((self.cfg.hz_per_sample * p.freq_start as f32) as f64
                * self.cfg.ihz_per_sample as f64)
                .round() as usize;

            for ii in (0..nm * STEPS_PER_FRAME).rev() {
                let mut known_length = false;
                let mut decoded_length = 0usize;
                for itx in 0..1024usize {
                    let offset_tx = ii + itx as i32 * fpt * STEPS_PER_FRAME;
                    if offset_tx >= self.recv_duration_frames * STEPS_PER_FRAME
                        || (itx + 1) * bpt >= self.encoded.len()
                    {
                        break;
                    }
                    let base = offset_tx as usize * step;
                    self.fft_in
                        .copy_from_slice(&self.recorded[base..base + spf]);
                    for k in 1..fpt {
                        let at = (offset_tx + k * STEPS_PER_FRAME) as usize * step;
                        for (x, &r) in self.fft_in.iter_mut().zip(&self.recorded[at..at + spf]) {
                            *x += r;
                        }
                    }
                    self.fft.power(&self.fft_in, &mut self.spectrum);

                    let mut cur_byte = 0u8;
                    for i in 0..2 * bpt {
                        let bin = base_bin + 16 * i;
                        let mut kmax = 0u8;
                        let mut amax = 0.0f64;
                        for k in 0..16 {
                            let v = self.spectrum[bin + k] as f64;
                            if v > amax {
                                kmax = k as u8;
                                amax = v;
                            }
                        }
                        if i % 2 == 1 {
                            cur_byte = cur_byte.wrapping_add(kmax << 4);
                            self.encoded[itx * bpt + i / 2] = cur_byte;
                            cur_byte = 0;
                        } else {
                            cur_byte = kmax;
                        }
                    }

                    if itx * bpt > offset && !known_length {
                        match reed_solomon::decode(&self.encoded[..offset], offset - 1) {
                            Some(d) if d[0] > 0 && d[0] as usize <= MAX_LENGTH_VARIABLE => {
                                known_length = true;
                                decoded_length = d[0] as usize;
                                let bytes =
                                    offset + decoded_length + ecc_bytes_for_length(decoded_length);
                                let frames = 2 * nm + bytes.div_ceil(bpt) as i32 * fpt;
                                if self.recv_duration_frames > frames
                                    || self.recv_duration_frames < frames - 2 * nm
                                {
                                    known_length = false;
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }

                    let bytes = offset + decoded_length + ecc_bytes_for_length(decoded_length);
                    if known_length && itx * bpt > bytes + 1 {
                        break;
                    }
                }

                if known_length {
                    let ecc = ecc_bytes_for_length(decoded_length);
                    let coded = &self.encoded[offset..offset + decoded_length + ecc];
                    if let Some(mut data) = reed_solomon::decode(coded, ecc) {
                        if self.cfg.dss {
                            for (i, b) in data.iter_mut().enumerate() {
                                *b ^= dss_magic(i);
                            }
                        }
                        return Some(data);
                    }
                }
            }
        }
        None
    }

    fn decode_fixed(&mut self) -> Option<Vec<u8>> {
        let spf = self.cfg.spf;
        let payload_length = self.cfg.fixed_len.unwrap_or(0);
        self.fft.power(&self.frame, &mut self.spectrum);

        let mut amax = 0.0f32;
        for &s in &self.spectrum[self.min_freq_start.max(1)..spf / 2] {
            amax = if amax >= s { amax } else { s };
        }
        let amax = 255.0f32 / if amax == 0.0 { 1.0 } else { amax };
        let row = &mut self.history_fixed[self.history_id_fixed];
        for (h, &s) in row.iter_mut().zip(&self.spectrum) {
            *h = (s * amax).round().clamp(0.0, 255.0) as u8;
        }
        let history_len = self.history_fixed.len();
        self.history_id_fixed = (self.history_id_fixed + 1) % history_len;

        let total_length = payload_length + ecc_bytes_for_length(payload_length);
        for &(_, p) in &self.protocols {
            let bin_start = p.freq_start as usize;
            let bin_offset = if p.extra == 1 { 16 } else { 0 };
            let bpt = p.bytes_per_tx as usize;
            let fpt = p.frames_per_tx as usize;
            let extra = p.extra as usize;
            let total_txs = extra * total_length.div_ceil(bpt);

            let history_start =
                (self.history_id_fixed + history_len - total_txs * fpt) % history_len;
            let mut detected_bins = vec![0u8; 2 * total_length];
            let mut detected_tones = vec![0u8; 2 * 16 * bpt];
            let mut tx_needed_total = 0;
            let mut tx_detected_total = 0;

            for k in 0..total_txs {
                if k % extra == 0 {
                    detected_tones.fill(0);
                }
                for i in 0..fpt {
                    let row = &self.history_fixed[(history_start + k * fpt + i) % history_len];
                    // argmax over 16 bins, ties going to the higher bin
                    let argmax = |from: usize| {
                        (1..16)
                            .fold((row[from], 0), |(m, mb), b| {
                                if m <= row[from + b] {
                                    (row[from + b], b)
                                } else {
                                    (m, mb)
                                }
                            })
                            .1
                    };
                    for j in 0..bpt {
                        let f0bin = argmax(bin_start + 2 * j * 16);
                        let f1bin = if p.extra == 1 {
                            argmax(bin_start + 2 * j * 16 + bin_offset)
                        } else {
                            f0bin
                        };
                        if k % extra == 0 {
                            detected_tones[2 * j * 16 + f0bin] += 1;
                        }
                        if (k + 1) % extra == 0 {
                            detected_tones[(2 * j + 1) * 16 + f1bin] += 1;
                        }
                    }
                }
                if extra > 1 && k % extra == 0 {
                    continue;
                }
                for j in 0..bpt {
                    let byte = (k / extra) * bpt + j;
                    if byte >= total_length {
                        break;
                    }
                    tx_needed_total += 2;
                    for b in 0..16 {
                        if detected_tones[2 * j * 16 + b] as usize > fpt / 2 {
                            detected_bins[2 * byte] = b as u8;
                            tx_detected_total += 1;
                        }
                        if detected_tones[(2 * j + 1) * 16 + b] as usize > fpt / 2 {
                            detected_bins[2 * byte + 1] = b as u8;
                            tx_detected_total += 1;
                        }
                    }
                }
            }

            if (tx_detected_total as f64) < 0.75 * tx_needed_total as f64 {
                continue;
            }
            for j in 0..total_length {
                self.encoded[j] = (detected_bins[2 * j + 1] << 4) + detected_bins[2 * j];
            }
            let ecc = ecc_bytes_for_length(payload_length);
            if let Some(mut data) = reed_solomon::decode(&self.encoded[..total_length], ecc) {
                if self.cfg.dss {
                    for (i, b) in data.iter_mut().enumerate() {
                        *b ^= dss_magic(i);
                    }
                }
                return Some(data);
            }
        }
        None
    }
}

fn max_frames_per_tx(protocols: &[(Protocol, ProtocolParams)], exclude_mt: bool) -> i32 {
    protocols
        .iter()
        .filter(|(_, p)| !(exclude_mt && p.extra > 1))
        .map(|(_, p)| p.frames_per_tx * p.extra)
        .max()
        .unwrap_or(0)
}
