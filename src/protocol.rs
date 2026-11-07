/// A ggwave transmission protocol. Frequencies assume the default 48 kHz /
/// 1024 samples per frame (46.875 Hz per bin).
///
/// The discriminants match `ggwave_ProtocolId` in C ggwave, so `p as u8`
/// gives the C protocol id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Protocol {
    /// Audible, base 1875 Hz, 9 frames per 3 bytes.
    AudibleNormal = 0,
    /// Audible, base 1875 Hz, 6 frames per 3 bytes.
    AudibleFast = 1,
    /// Audible, base 1875 Hz, 3 frames per 3 bytes.
    AudibleFastest = 2,
    /// Ultrasound, base 15000 Hz, 9 frames per 3 bytes.
    UltrasoundNormal = 3,
    /// Ultrasound, base 15000 Hz, 6 frames per 3 bytes.
    UltrasoundFast = 4,
    /// Ultrasound, base 15000 Hz, 3 frames per 3 bytes.
    UltrasoundFastest = 5,
    /// Dual-tone, base 1125 Hz, 9 frames per byte.
    DtNormal = 6,
    /// Dual-tone, base 1125 Hz, 6 frames per byte.
    DtFast = 7,
    /// Dual-tone, base 1125 Hz, 3 frames per byte.
    DtFastest = 8,
    /// Mono-tone, base 1125 Hz, 9 frames per nibble (fixed length only).
    MtNormal = 9,
    /// Mono-tone, base 1125 Hz, 6 frames per nibble (fixed length only).
    MtFast = 10,
    /// Mono-tone, base 1125 Hz, 3 frames per nibble (fixed length only).
    MtFastest = 11,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProtocolParams {
    /// FFT bin index of the lowest frequency.
    pub freq_start: i32,
    /// Number of frames used to transmit one chunk of data.
    pub frames_per_tx: i32,
    /// Number of bytes in one chunk of data.
    pub bytes_per_tx: i32,
    /// 2 for mono-tone protocols, 1 otherwise.
    pub extra: i32,
}

impl Protocol {
    /// Every protocol, in C protocol-id order.
    pub const ALL: [Protocol; 12] = [
        Protocol::AudibleNormal,
        Protocol::AudibleFast,
        Protocol::AudibleFastest,
        Protocol::UltrasoundNormal,
        Protocol::UltrasoundFast,
        Protocol::UltrasoundFastest,
        Protocol::DtNormal,
        Protocol::DtFast,
        Protocol::DtFastest,
        Protocol::MtNormal,
        Protocol::MtFast,
        Protocol::MtFastest,
    ];

    /// The protocol with the given C `ggwave_ProtocolId`, if it is one of the built-in ones.
    pub fn from_id(id: u8) -> Option<Protocol> {
        Self::ALL.get(id as usize).copied()
    }

    /// The protocol name as C ggwave reports it.
    pub fn name(self) -> &'static str {
        [
            "Normal",
            "Fast",
            "Fastest",
            "[U] Normal",
            "[U] Fast",
            "[U] Fastest",
            "[DT] Normal",
            "[DT] Fast",
            "[DT] Fastest",
            "[MT] Normal",
            "[MT] Fast",
            "[MT] Fastest",
        ][self as usize]
    }

    /// Mono-tone protocols (MT) only work with fixed-length payloads.
    pub fn is_mono_tone(self) -> bool {
        self.params().extra == 2
    }

    pub(crate) fn params(self) -> ProtocolParams {
        // (freqStart, framesPerTx, bytesPerTx, extra) from ggwave.h Protocols::kDefault()
        let (freq_start, frames_per_tx, bytes_per_tx, extra) = match self {
            Protocol::AudibleNormal => (40, 9, 3, 1),
            Protocol::AudibleFast => (40, 6, 3, 1),
            Protocol::AudibleFastest => (40, 3, 3, 1),
            Protocol::UltrasoundNormal => (320, 9, 3, 1),
            Protocol::UltrasoundFast => (320, 6, 3, 1),
            Protocol::UltrasoundFastest => (320, 3, 3, 1),
            Protocol::DtNormal => (24, 9, 1, 1),
            Protocol::DtFast => (24, 6, 1, 1),
            Protocol::DtFastest => (24, 3, 1, 1),
            Protocol::MtNormal => (24, 9, 1, 2),
            Protocol::MtFast => (24, 6, 1, 2),
            Protocol::MtFastest => (24, 3, 1, 2),
        };
        ProtocolParams {
            freq_start,
            frames_per_tx,
            bytes_per_tx,
            extra,
        }
    }
}
