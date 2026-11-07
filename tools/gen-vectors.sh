#!/bin/sh
# Regenerates tests/vectors/: C ggwave 060aec7 waveforms (i16 little-endian,
# C's GGWAVE_SAMPLE_FORMAT_I16 output) plus manifest.txt, so `cargo test`
# checks against C output without a C toolchain.
#
# Audible and DT/MT vectors are rendered at 16 kHz through C's resampler to
# keep the repository small; ultrasound needs 48 kHz. The live tests in
# tests/c_interop.rs cover every protocol and length at 48 kHz.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TOOL=$("$ROOT/tools/build-c-ggwave.sh")
OUT="$ROOT/tests/vectors"
VOLUME=50

rm -f "$OUT"/*.i16
mkdir -p "$OUT"
echo "# name protocol_id fixed_len(0=variable) sample_rate_out volume payload_hex" > "$OUT/manifest.txt"

# payload of $1 bytes, same formula as tests/common payload(len, seed=$1)
payload() {
    i=0; hex=""
    while [ "$i" -lt "$1" ]; do
        hex="$hex$(printf '%02x' $(( ((i * 37 + $1) & 255) ^ 90 )))"
        i=$((i + 1))
    done
    echo "$hex"
}

vector() { # protocol fixed len rate
    name="p$1-$( [ "$2" -eq 0 ] && echo var || echo fixed)-$3-$4"
    hex=$(payload "$3")
    lflag=""; [ "$2" -ne 0 ] && lflag="-l $2"
    # shellcheck disable=SC2086
    "$TOOL" encode -s -p "$1" -v "$VOLUME" $lflag -r "$4" "$hex" > "$OUT/$name.i16"
    echo "$name $1 $2 $4 $VOLUME $hex" >> "$OUT/manifest.txt"
}

# the must-have: AUDIBLE_FAST at 48 kHz, variable and fixed, including 32-byte payloads
for len in 1 5 32; do vector 1 0 "$len" 48000; done
vector 1 32 32 48000

# every other variable-length protocol
for p in 0 2 6 7 8; do vector "$p" 0 16 16000; done
for p in 3 4 5; do vector "$p" 0 16 48000; done

# every protocol in fixed-length mode
for p in 0 1 2 6 7 8 9 10 11; do vector "$p" 8 8 16000; done
for p in 3 4 5; do vector "$p" 8 8 48000; done

du -sh "$OUT"
