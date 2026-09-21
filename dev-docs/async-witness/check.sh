#!/bin/sh
set -eu
witness_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
list_only=false
if [ "${1:-}" = "--list" ]; then
    list_only=true
    shift
fi
if [ "$#" -gt 1 ]; then
    echo "usage: check.sh [--list] [all|ports|fast|real]" >&2
    exit 2
fi
case "${1:-all}" in
    all) packages="async-witness-ports async-witness-fast async-witness-real" ;;
    ports|fast|real) packages="async-witness-$1" ;;
    *) echo "Unknown witness selector: $1" >&2; exit 2 ;;
esac
if [ "$list_only" = true ]; then
    for package in $packages; do printf '%s\n' "$package"; done
    exit 0
fi
set --
for package in $packages; do set -- "$@" -p "$package"; done
manifest="$witness_root/Cargo.toml"
# Explicit GWZ-local adoption: the checker is tooling, never a crate dependency.
checker="$witness_root/../../../glade-discover/tools/architecture-check/Cargo.toml"
if [ ! -f "$checker" ]; then
    echo "Architecture checker unavailable: materialize the glade-discover GWZ member" >&2
    exit 1
fi
cargo run --quiet --locked --offline --manifest-path "$checker" -- "$witness_root"

# The checker shells `cargo metadata --no-deps`, so it classifies WORKSPACE
# MEMBERS only. `glade-wire` is a path dependency outside this workspace and can
# never appear there: Cargo refuses a member that is not hierarchically below
# the workspace root, and an outside path dependency is absent from --no-deps
# metadata, so listing it in the policy would fail ARCH-001 instead of gating
# anything. Assert the property that actually matters about it directly — the
# pure crate the port's types come from is still a leaf with no dependency of
# its own. `glade-decl` is not reachable from this workspace at all.
wire_lines=$(cargo tree --locked --offline --manifest-path "$manifest" -p glade-wire | wc -l | tr -d ' ')
if [ "$wire_lines" != "1" ]; then
    echo "glade-wire is no longer dependency-free: the DI-E04 wall now rests on $wire_lines tree lines" >&2
    cargo tree --locked --offline --manifest-path "$manifest" -p glade-wire >&2
    exit 1
fi

# `--no-deps` also hides TRANSITIVE reachability from the checker: a contract
# crate depending on a permitted pure crate that itself pulled in a framework
# would pass. The second, equally cheap assertion (AsyncWitnessPlan.md §4.5):
# inverting the tree from each framework must reach no contract-role package.
# `contracts` MUST list every role: "contract" package in architecture-policy.json.
contracts="async-witness-ports"
for framework in shaku sdax sdax-tokio sdax-testkit tokio iroh; do
    inverted=$(cargo tree --locked --offline --manifest-path "$manifest" --invert "$framework")
    for contract in $contracts; do
        if printf '%s\n' "$inverted" | grep -q -- "$contract"; then
            echo "$contract is reachable from $framework: the DI-E04 wall is breached" >&2
            printf '%s\n' "$inverted" >&2
            exit 1
        fi
    done
done

# `--all-features --all-targets` is deliberately NOT used. From Phase 1 the fast
# member carries examples that MUST fail to compile (di-eval/README.md records
# the same rule); sweeping them in would invert their meaning. Run those by hand
# and read the diagnostics.
cargo test --locked --offline --manifest-path "$manifest" "$@" --lib --tests
# `cargo fmt --all` formats a package's LOCAL PATH DEPENDENCIES too, which from
# here reaches glade-wire, glade-node and the contracts. Select packages by name.
cargo fmt --manifest-path "$manifest" "$@" -- --check
cargo clippy --locked --offline --manifest-path "$manifest" "$@" --lib --tests -- -D warnings
