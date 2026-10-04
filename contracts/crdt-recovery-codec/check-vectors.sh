#!/bin/sh
# Representation only: B1 genuine signatures/trusted policy remain mandatory.
set -eu
if [ "$#" -gt 1 ]; then
    echo "usage: check-vectors.sh [--pre-remote]" >&2
    exit 2
fi
case "${1:-}" in
    "") set -- ;;
    --pre-remote) ;;
    *) echo "usage: check-vectors.sh [--pre-remote]" >&2; exit 2 ;;
esac
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
python3 -B "$root/compat/gate.py" "$root" "$@"
python3 -B "$root/compat/test_gate.py"
cargo test --locked --offline --manifest-path "$root/../Cargo.toml" -p glade-crdt-recovery-codec --test vectors
python3 -B "$root/compat/python.py" "$root"
node --experimental-transform-types "$root/compat/typescript.ts" "$root"
