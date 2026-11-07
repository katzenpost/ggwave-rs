//! Power spectrum of a real frame, laid out the way C ggwave's decoder sees it.

pub(crate) struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Fft {
    /// `n` must be a power of two.
    pub fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| {
                if bits == 0 {
                    0
                } else {
                    i.reverse_bits() >> (usize::BITS - bits)
                }
            })
            .collect();
        let (cos, sin) = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .unzip();
        Fft {
            n,
            cos,
            sin,
            rev,
            re: vec![0.0; n],
            im: vec![0.0; n],
        }
    }

    /// Writes |X_k|^2 into `spectrum[k]` for `0 < k < n/2`, the DC and Nyquist
    /// powers summed into `spectrum[0]` (Ooura packing), and zero above n/2,
    /// matching what ggwave.cpp computes from its `rdft` output.
    pub fn power(&mut self, input: &[f32], spectrum: &mut [f32]) {
        let n = self.n;
        for (&r, &x) in self.rev.iter().zip(input) {
            self.re[r] = x;
        }
        self.im.fill(0.0);
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = self.re[b] * wr - self.im[b] * wi;
                    let ti = self.re[b] * wi + self.im[b] * wr;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            len *= 2;
        }
        spectrum[0] = self.re[0] * self.re[0] + self.re[n / 2] * self.re[n / 2];
        for (k, s) in spectrum.iter_mut().enumerate().take(n / 2).skip(1) {
            *s = self.re[k] * self.re[k] + self.im[k] * self.im[k];
        }
        for s in &mut spectrum[n / 2..n] {
            *s = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_tone_lands_in_its_bin() {
        let n = 1024;
        let x: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 40.0 * i as f32 / n as f32).sin())
            .collect();
        let mut s = vec![0.0; n];
        Fft::new(n).power(&x, &mut s);
        let peak = (1..n / 2).max_by(|&a, &b| s[a].total_cmp(&s[b])).unwrap();
        assert_eq!(peak, 40);
        // |X_k| = n/2 for a unit sine
        assert!((s[40].sqrt() - n as f32 / 2.0).abs() < 0.5);
    }
}
