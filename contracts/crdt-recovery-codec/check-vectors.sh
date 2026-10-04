#!/bin/sh
# Representation only: B1 genuine signatures/trusted policy remain mandatory.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
python3 -B "$root/compat/gate.py" "$root" "${1:-}"
python3 -B "$root/compat/test_gate.py"
cargo test --locked --offline --manifest-path "$root/../Cargo.toml" -p glade-crdt-recovery-codec --test vectors
python3 -B "$root/compat/python.py" "$root"
node --experimental-transform-types "$root/compat/typescript.ts" "$root"
