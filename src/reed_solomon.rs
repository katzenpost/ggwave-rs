//! Reed-Solomon over GF(2^8) (primitive polynomial 0x11d), ported from the
//! `reed-solomon` code bundled with C ggwave (Mike Lubinets, MIT).
//!
//! Polynomials are stored highest degree first, as in the C code. Decoding
//! follows the C algorithm step by step, including that it trusts the
//! Berlekamp-Massey result without re-checking syndromes, so it accepts and
//! rejects exactly the same inputs as C ggwave.

const fn tables() -> ([u8; 512], [u8; 256]) {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11d;
        }
        i += 1;
    }
    while i < 512 {
        exp[i] = exp[i - 255];
        i += 1;
    }
    (exp, log)
}

const TABLES: ([u8; 512], [u8; 256]) = tables();
const EXP: [u8; 512] = TABLES.0;
const LOG: [u8; 256] = TABLES.1;

fn mul(x: u8, y: u8) -> u8 {
    if x == 0 || y == 0 {
        return 0;
    }
    EXP[LOG[x as usize] as usize + LOG[y as usize] as usize]
}

// C asserts y != 0; with assertions off it uses log[0] = 0, which we mirror.
fn div(x: u8, y: u8) -> u8 {
    if x == 0 {
        return 0;
    }
    EXP[(LOG[x as usize] as usize + 255 - LOG[y as usize] as usize) % 255]
}

fn pow(x: u8, power: i64) -> u8 {
    EXP[(LOG[x as usize] as i64 * power).rem_euclid(255) as usize]
}

fn inverse(x: u8) -> u8 {
    EXP[255 - LOG[x as usize] as usize]
}

fn poly_scale(p: &[u8], x: u8) -> Vec<u8> {
    p.iter().map(|&c| mul(c, x)).collect()
}

fn poly_add(p: &[u8], q: &[u8]) -> Vec<u8> {
    let n = p.len().max(q.len());
    let mut r = vec![0u8; n];
    for (i, &c) in p.iter().enumerate() {
        r[i + n - p.len()] = c;
    }
    for (i, &c) in q.iter().enumerate() {
        r[i + n - q.len()] ^= c;
    }
    r
}

fn poly_mul(p: &[u8], q: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; p.len() + q.len() - 1];
    for (j, &qj) in q.iter().enumerate() {
        for (i, &pi) in p.iter().enumerate() {
            r[i + j] ^= mul(pi, qj);
        }
    }
    r
}

fn poly_eval(p: &[u8], x: u8) -> u8 {
    let mut y = p[0];
    for &c in &p[1..] {
        y = mul(y, x) ^ c;
    }
    y
}

/// Number of ECC bytes ggwave appends to a payload of `len` bytes.
pub(crate) fn ecc_bytes_for_length(len: usize) -> usize {
    if len < 4 {
        2
    } else {
        (2 * (len / 5)).max(4)
    }
}

fn generator(ecc_len: usize) -> Vec<u8> {
    let mut gen = vec![1u8];
    for i in 0..ecc_len {
        gen = poly_mul(&gen, &[1, pow(2, i as i64)]);
    }
    gen
}

/// Systematic encoding: returns `msg` followed by `ecc_len` parity bytes.
pub(crate) fn encode(msg: &[u8], ecc_len: usize) -> Vec<u8> {
    let gen = generator(ecc_len);
    let mut out = msg.to_vec();
    out.resize(msg.len() + ecc_len, 0);
    for i in 0..msg.len() {
        let coef = out[i];
        if coef != 0 {
            for j in 1..gen.len() {
                out[i + j] ^= mul(gen[j], coef);
            }
        }
    }
    // the division above overwrote the message part; only the remainder is parity
    out[..msg.len()].copy_from_slice(msg);
    out
}

/// Decodes `src` (message followed by `ecc_len` parity bytes) and returns the
/// corrected message, or `None` when C ggwave would report failure.
pub(crate) fn decode(src: &[u8], ecc_len: usize) -> Option<Vec<u8>> {
    let msg_len = src.len() - ecc_len;
    let msg_in = src;

    // synd[0] = 0, synd[i] = msg(alpha^(i-1))
    let synd: Vec<u8> = (0..=ecc_len)
        .map(|i| {
            if i == 0 {
                0
            } else {
                poly_eval(msg_in, pow(2, i as i64 - 1))
            }
        })
        .collect();
    if synd.iter().all(|&s| s == 0) {
        return Some(msg_in[..msg_len].to_vec());
    }

    // No erasures: the Forney syndromes are the syndromes without the leading 0.
    let eloc = find_error_locator(&synd[1..], ecc_len)?;
    let reloc: Vec<u8> = eloc.iter().rev().copied().collect();

    let n = msg_in.len();
    let errs = reloc.len() - 1;
    let err_pos: Vec<usize> = (0..n)
        .filter(|&i| poly_eval(&reloc, pow(2, i as i64)) == 0)
        .map(|i| n - 1 - i)
        .collect();
    if err_pos.len() != errs || err_pos.is_empty() {
        return None;
    }

    let corrected = correct_errata(&synd, &err_pos, msg_in);
    Some(corrected[..msg_len].to_vec())
}

// Berlekamp-Massey, as in RS::ReedSolomon::FindErrorLocator without erasures.
fn find_error_locator(synd: &[u8], ecc_len: usize) -> Option<Vec<u8>> {
    let mut err_loc = vec![1u8];
    let mut old_loc = vec![1u8];
    for k in 0..ecc_len {
        let mut delta = synd[k];
        for j in 1..err_loc.len() {
            delta ^= mul(err_loc[err_loc.len() - j - 1], synd[k - j]);
        }
        old_loc.push(0);
        if delta != 0 {
            if old_loc.len() > err_loc.len() {
                let new_loc = poly_scale(&old_loc, delta);
                old_loc = poly_scale(&err_loc, inverse(delta));
                err_loc = new_loc;
            }
            err_loc = poly_add(&err_loc, &poly_scale(&old_loc, delta));
        }
    }
    let shift = err_loc.iter().take_while(|&&c| c == 0).count();
    if shift == err_loc.len() {
        return None;
    }
    let errs = err_loc.len() - shift - 1;
    if errs * 2 > ecc_len {
        return None;
    }
    Some(err_loc[shift..].to_vec())
}

fn correct_errata(synd: &[u8], err_pos: &[usize], msg_in: &[u8]) -> Vec<u8> {
    let coef_pos: Vec<usize> = err_pos.iter().map(|&p| msg_in.len() - 1 - p).collect();

    let mut errata_loc = vec![1u8];
    for &p in &coef_pos {
        errata_loc = poly_mul(&errata_loc, &[pow(2, p as i64), 1]);
    }

    // Error evaluator: (reversed syndromes * errata locator) mod x^(nerr + 1).
    let rsynd: Vec<u8> = synd.iter().rev().copied().collect();
    let product = poly_mul(&rsynd, &errata_loc);
    let re_eval = &product[product.len() - errata_loc.len()..];

    let x: Vec<u8> = coef_pos
        .iter()
        .map(|&p| pow(2, -(255 - p as i64)))
        .collect();

    let mut e = vec![0u8; msg_in.len()];
    for (i, &xi) in x.iter().enumerate() {
        let xi_inv = inverse(xi);
        let mut err_loc_prime = 1u8;
        for (j, &xj) in x.iter().enumerate() {
            if j != i {
                err_loc_prime = mul(err_loc_prime, 1 ^ mul(xi_inv, xj));
            }
        }
        let y = mul(pow(xi, 1), poly_eval(re_eval, xi_inv));
        e[err_pos[i]] = div(y, err_loc_prime);
    }
    poly_add(msg_in, &e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_c() {
        // spot values from reed-solomon/gf.hpp
        assert_eq!(
            &EXP[..10],
            &[1, 2, 4, 8, 0x10, 0x20, 0x40, 0x80, 0x1d, 0x3a]
        );
        assert_eq!(&EXP[508..], &[0x47, 0x8e, 0x1, 0x2]);
        assert_eq!(&LOG[..8], &[0, 0, 1, 0x19, 2, 0x32, 0x1a, 0xc6]);
        assert_eq!(LOG[255], 0xaf);
    }

    #[test]
    fn parity_matches_c() {
        // C RS::ReedSolomon(32, 12).Encode() of this message
        let msg: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(37) ^ 0x5a).collect();
        let enc = encode(&msg, 12);
        assert_eq!(
            &enc[32..],
            &[0x5e, 0xf0, 0xe5, 0x27, 0x07, 0xbb, 0xb8, 0x30, 0xbc, 0xc0, 0x5c, 0x38]
        );
    }

    #[test]
    fn corrects_up_to_half_ecc() {
        let msg: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(37) ^ 0x5a).collect();
        let ecc = ecc_bytes_for_length(msg.len());
        let enc = encode(&msg, ecc);
        assert_eq!(decode(&enc, ecc).as_deref(), Some(&msg[..]));
        let mut bad = enc.clone();
        let n = bad.len();
        for i in 0..ecc / 2 {
            bad[i * 7 % n] ^= 0xa5;
        }
        assert_eq!(decode(&bad, ecc).as_deref(), Some(&msg[..]));
        let mut worse = enc;
        for i in 0..ecc {
            worse[i * 3] ^= 0x11;
        }
        assert_ne!(decode(&worse, ecc).as_deref(), Some(&msg[..]));
    }
}
