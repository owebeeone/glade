#!/bin/sh
set -eu
package=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
trace=$(mktemp)
trap 'rm -f "$trace"' EXIT HUP INT TERM
cargo run --quiet --locked --offline --manifest-path "$package/Cargo.toml" --example text_admission_trace > "$trace"
node "$package/../../../glial/test/independent_admission_contract.mjs" "$trace"
