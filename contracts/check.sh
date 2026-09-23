#!/bin/sh
set -eu
contract_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
list_only=false
if [ "${1:-}" = "--list" ]; then
    list_only=true
    shift
fi
if [ "$#" -gt 1 ]; then
    echo "usage: check.sh [--list] [all|binding|invocation|subscription|sync|lifecycle|persistence|carrier|clock|grant|signer]" >&2
    exit 2
fi
case "${1:-all}" in
    all) packages="glade-binding-api glade-invocation-api glade-subscription-api glade-sync-api glade-lifecycle-api glade-persistence-api glade-carrier-api glade-clock-api glade-grant-api glade-signer-api" ;;
    binding|invocation|subscription|sync|lifecycle|persistence|carrier|clock|grant|signer) packages="glade-$1-api" ;;
    *) echo "Unknown contract selector: $1" >&2; exit 2 ;;
esac
if [ "$list_only" = true ]; then
    for package in $packages; do printf '%s\n' "$package"; done
    exit 0
fi
set --
for package in $packages; do set -- "$@" -p "$package"; done
# Explicit GWZ-local adoption: checker is tooling, never a crate dependency.
checker="$contract_root/../../glade-discover/tools/architecture-check/Cargo.toml"
if [ ! -f "$checker" ]; then
    echo "Architecture checker unavailable: materialize the glade-discover GWZ member" >&2
    exit 1
fi
cargo run --quiet --locked --offline --manifest-path "$checker" -- "$contract_root"
cargo test --locked --offline --manifest-path "$contract_root/Cargo.toml" "$@" --all-features
cargo fmt --manifest-path "$contract_root/Cargo.toml" "$@" -- --check
cargo clippy --locked --offline --manifest-path "$contract_root/Cargo.toml" "$@" --all-features --all-targets -- -D warnings
