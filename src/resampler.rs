//! Windowed-sinc resampler, ported from `GGWave::Resampler` so that output at
//! a non-native rate matches C ggwave sample for sample.

const WIDTH: i32 = 64;
const DELAY_SIZE: usize = 140;
const SAMPLES_PER_ZERO_CROSSING: i32 = 32;

#[derive(Clone, Copy, Default)]
struct State {
    n_samples_total: i32,
    time_int: i32,
    time_last: i32,
    time_now: f64,
}

pub(crate) struct Resampler {
    sinc_table: Vec<f32>,
    delay_buffer: Vec<f32>,
    edge_samples: Vec<f32>,
    samples_inp: Vec<f32>,
    state: State,
}

impl Resampler {
    pub fn new() -> Self {
        let n = (WIDTH * SAMPLES_PER_ZERO_CROSSING) as usize;
        let mut sinc_table = vec![0.0f32; n];
        let win_freq = std::f64::consts::PI / WIDTH as f64 / SAMPLES_PER_ZERO_CROSSING as f64;
        sinc_table[0] = 1.0;
        for (i, s) in sinc_table.iter_mut().enumerate().skip(1) {
            let temp = i as f64 * std::f64::consts::PI / SAMPLES_PER_ZERO_CROSSING as f64;
            *s = (temp.sin() / temp) as f32;
            let win = 0.5 + 0.5 * (win_freq * i as f64).cos();
            *s = (*s as f64 * win) as f32;
        }
        Resampler {
            sinc_table,
            delay_buffer: vec![0.0; 3 * WIDTH as usize],
            edge_samples: vec![0.0; WIDTH as usize],
            samples_inp: Vec::new(),
            state: State::default(),
        }
    }

    pub fn reset(&mut self) {
        self.state = State::default();
        self.edge_samples.fill(0.0);
        self.delay_buffer.fill(0.0);
    }

    pub fn n_samples_total(&self) -> i32 {
        self.state.n_samples_total
    }

    /// Resamples `input` by `factor` (input rate / output rate), appending to
    /// `out`. Output lags input by `WIDTH` samples, as in C. `input` must be
    /// longer than `WIDTH`.
    pub fn resample(&mut self, factor: f32, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len();
        assert!(n > WIDTH as usize);
        let w = WIDTH as usize;
        self.samples_inp.clear();
        self.samples_inp.extend_from_slice(&self.edge_samples);
        self.samples_inp.extend_from_slice(input);
        self.edge_samples.copy_from_slice(&input[n - w..]);

        self.state.n_samples_total += n as i32;
        let mut idx_inp: usize = 0; // C starts at -1 and pre-increments
        let factor_d = factor as f64;

        loop {
            if !self.feed(n, &mut idx_inp) {
                break;
            }

            let st = self.state;
            let mut temp1 = 0.0f64;
            let left_limit = ((st.time_now - WIDTH as f64 + 1.0) as i32).max(0);
            let right_limit = ((st.time_now + WIDTH as f64) as i32).min(st.n_samples_total + WIDTH);
            if factor < 1.0 {
                for j in left_limit..right_limit {
                    temp1 +=
                        self.get_data(j - st.time_int) as f64 * self.sinc(st.time_now - j as f64);
                }
            } else {
                let one_over_factor = 1.0 / factor_d;
                for j in left_limit..right_limit {
                    temp1 += self.get_data(j - st.time_int) as f64
                        * one_over_factor
                        * self.sinc(one_over_factor * (st.time_now - j as f64));
                }
            }
            out.push(temp1 as f32);

            self.state.time_now += factor_d;
            self.state.time_last = self.state.time_int;
            self.state.time_int = self.state.time_now as i32;
            if !self.feed(n, &mut idx_inp) {
                break;
            }
        }
    }

    // Pulls input samples until time_last catches up with time_int; false when input runs out.
    fn feed(&mut self, n: usize, idx_inp: &mut usize) -> bool {
        while self.state.time_last < self.state.time_int {
            if *idx_inp >= n {
                return false;
            }
            let d = self.samples_inp[*idx_inp];
            *idx_inp += 1;
            self.delay_buffer.copy_within(1..DELAY_SIZE - 4, 0);
            self.delay_buffer[DELAY_SIZE - 5] = d;
            self.state.time_last += 1;
        }
        true
    }

    fn get_data(&self, j: i32) -> f32 {
        self.delay_buffer[(j + WIDTH) as usize]
    }

    fn sinc(&self, x: f64) -> f64 {
        if x.abs() >= (WIDTH - 1) as f64 {
            return 0.0;
        }
        let temp = x.abs() * SAMPLES_PER_ZERO_CROSSING as f64;
        let low = temp as usize;
        let delta = temp - low as f64;
        let a = self.sinc_table[low] as f64;
        let b = self.sinc_table[low + 1] as f64;
        a + (b - a) * delta
    }
}
