#!/bin/sh
# Builds the C ggwave test tool (tools/ggwave-c.cpp) against upstream ggwave
# at the pinned commit. Test tooling only; the crate never links C.
#
# Usage: tools/build-c-ggwave.sh [out-dir]   (default: target/c-ggwave)
# Prints the path of the built binary. Needs git and a C++ compiler.
set -eu

COMMIT=060aec73dd7123ccac200442f75bdc7369795ffe
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$ROOT/target/c-ggwave"}
SRC="$OUT/ggwave"
CXX=${CXX:-c++}

mkdir -p "$OUT"
if [ ! -d "$SRC/.git" ]; then
    git init -q "$SRC"
    git -C "$SRC" remote add origin https://github.com/ggerganov/ggwave
fi
if [ "$(git -C "$SRC" rev-parse -q --verify HEAD 2>/dev/null || true)" != "$COMMIT" ]; then
    git -C "$SRC" fetch -q --depth 1 origin "$COMMIT"
    git -C "$SRC" checkout -q --detach FETCH_HEAD
fi

# -ffp-contract=off keeps float rounding identical across targets (gcc fuses multiply-adds on arm64)
"$CXX" -std=c++11 -O2 -DNDEBUG -ffp-contract=off \
    -I"$SRC/include" -I"$SRC/src" \
    "$ROOT/tools/ggwave-c.cpp" "$SRC/src/ggwave.cpp" \
    -o "$OUT/ggwave-c"
echo "$OUT/ggwave-c"
