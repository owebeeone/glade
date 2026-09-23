#!/bin/sh
# Plan Step 3.5's node-side ARCH-002 fixture, executable and failing CLOSED, on
# the witness's pattern (../dev-docs/async-witness/arch002-fixture.sh).
#
# check.sh runs the architecture checker over this workspace against
# architecture-policy.json, whose exact allowlist is what keeps a framework out
# of glade-node until a reviewed policy change lets one in. A gate that has
# never been seen to refuse anything is not evidence that it would, so this
# script injects `shaku` into glade-node's manifest twice -- once as an ordinary
# dependency, once under [target.'cfg(windows)'], a platform branch that is
# disabled on every host but Windows -- and requires the checker to refuse each
# with exactly `ARCH-002 glade-node: undeclared dependency normal:shaku`.
#
# It passes on that diagnostic and on nothing else. A PASS, another diagnostic,
# or a checker that could not run `cargo metadata` fails here, because each
# would let a gate that had stopped working look like one that was working.
#
# Everything happens on a COPY in a temporary directory. The live Cargo.toml,
# Cargo.lock and architecture-policy.json are only read, so an interrupted run
# cannot leave the tree changed. The copy keeps the tree's depth below a
# `glade` root, and `wire-rs`, which the manifest names by path, is a symlink
# this script only reads through. The checker needs no lockfile (it shells
# `cargo metadata --no-deps`), so the untracked Cargo.lock is not copied.
#
# When a reviewed step allows `normal:shaku` for glade-node (the plan's Step 3.2
# puts the Shaku assembly in this crate), this fixture fails closed -- the
# injection is then accepted, or collides with the real entry -- until
# `framework` below names one glade-node still may not declare. That edit
# belongs to the same reviewed policy change.
set -eu
node_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
glade_root=$(CDPATH= cd -- "$node_root/.." && pwd -P)
# Explicit GWZ-local adoption, as in check.sh: the checker is tooling, never a
# crate dependency.
checker="$node_root/../../glade-discover/tools/architecture-check/Cargo.toml"
if [ ! -f "$checker" ]; then
    echo "Architecture checker unavailable: materialize the glade-discover GWZ member" >&2
    exit 1
fi

framework=shaku
version="=0.6.3"
expected="ARCH-002 glade-node: undeclared dependency normal:$framework"

work=$(mktemp -d "${TMPDIR:-/tmp}/glade-node-arch002.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM

copy="$work/glade/node"
mkdir -p "$copy"
(cd "$node_root" && tar -cf - Cargo.toml architecture-policy.json src tests) |
    (cd "$copy" && tar -xf -)
ln -s "$glade_root/wire-rs" "$work/glade/wire-rs"

gate() {
    cargo run --quiet --locked --offline --manifest-path "$checker" -- "$copy" 2>&1
}

# 1. The positive control. Without it, "the gate failed" could mean "the copy
#    was broken", and nothing below would be about the gate.
if ! control=$(gate); then
    echo "ARCH-002 fixture: the untouched copy does not pass the gate, so nothing below decides anything" >&2
    printf '%s\n' "$control" >&2
    exit 1
fi
case "$control" in
    *"Architecture boundaries: PASS"*) : ;;
    *)
        echo "ARCH-002 fixture: the untouched copy did not report PASS" >&2
        printf '%s\n' "$control" >&2
        exit 1
        ;;
esac

# 2. Each injection starts from the live manifest, so the second case never
#    sees the first. A dotted table header appended at the end is valid TOML
#    whether or not the manifest already has the table it extends.
refuse() {
    table=$1
    label=$2
    cp "$node_root/Cargo.toml" "$copy/Cargo.toml"
    printf '\n[%s]\nversion = "%s"\n' "$table" "$version" >> "$copy/Cargo.toml"
    if diagnostic=$(gate); then
        echo "ARCH-002 fixture: the gate ACCEPTED $framework as $label of glade-node" >&2
        printf '%s\n' "$diagnostic" >&2
        exit 1
    fi
    case "$diagnostic" in
        *"cargo metadata failed"*)
            echo "ARCH-002 fixture: the checker could not run, so its non-zero exit decides nothing" >&2
            printf '%s\n' "$diagnostic" >&2
            exit 1
            ;;
    esac
    if [ "$diagnostic" != "$expected" ]; then
        echo "ARCH-002 fixture: the gate refused $label, but not with the diagnostic this fixture is about" >&2
        echo "expected: $expected" >&2
        echo "actual:   $diagnostic" >&2
        exit 1
    fi
    printf '  refused %s: %s\n' "$label" "$diagnostic"
}

refuse "dependencies.$framework" "an ordinary dependency"
refuse "target.'cfg(windows)'.dependencies.$framework" "a cfg(windows) dependency"
printf 'ARCH-002 fixture: the gate refused %s injected into glade-node, in both cases\n' "$framework"
