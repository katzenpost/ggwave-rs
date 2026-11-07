use ggwave::{Decoder, Encoder, Error, Parameters, Protocol, MAX_LENGTH_VARIABLE};

#[test]
fn rejects_bad_input() {
    let enc = Encoder::new(Parameters::default()).unwrap();
    assert_eq!(
        enc.encode(b"", Protocol::AudibleFast, 10),
        Err(Error::EmptyPayload)
    );
    assert_eq!(
        enc.encode(b"x", Protocol::AudibleFast, 101),
        Err(Error::InvalidVolume(101))
    );
    assert_eq!(
        enc.encode(b"x", Protocol::MtFast, 10),
        Err(Error::MonoToneNeedsFixedLength)
    );
    let long = vec![0u8; MAX_LENGTH_VARIABLE + 1];
    assert!(matches!(
        enc.encode(&long, Protocol::AudibleFast, 10),
        Err(Error::PayloadTooLong { .. })
    ));

    let bad = |p: Parameters| Encoder::new(p.clone()).is_err() && Decoder::new(p).is_err();
    assert!(bad(Parameters {
        samples_per_frame: 1000,
        ..Parameters::default()
    }));
    assert!(bad(Parameters {
        sample_rate_inp: 500.0,
        ..Parameters::default()
    }));
    assert!(bad(Parameters {
        payload_length: Some(65),
        ..Parameters::default()
    }));
}

#[test]
fn half_rate_half_frame_is_the_same_signal() {
    // 24 kHz / 512 has the same 46.875 Hz bins and frame length as 48 kHz / 1024
    let p = Parameters {
        sample_rate: 24000.0,
        sample_rate_inp: 24000.0,
        sample_rate_out: 24000.0,
        samples_per_frame: 512,
        ..Parameters::default()
    };
    let s = Encoder::new(p.clone())
        .unwrap()
        .encode(b"voucher", Protocol::AudibleFast, 50)
        .unwrap();
    let mut d = Decoder::with_protocols(p, &[Protocol::AudibleFast]).unwrap();
    let mut x = vec![0.0; 3000];
    x.extend(s);
    x.resize(x.len() + 24000, 0.0);
    assert_eq!(d.push(&x).as_deref(), Some(&b"voucher"[..]));
}
