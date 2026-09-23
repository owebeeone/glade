#!/bin/sh
# ARCH-002's negative fixture for this workspace, failing CLOSED, on the
# witness's pattern (../dev-docs/async-witness/arch002-fixture.sh).
#
# The claim is that no contract crate here can take a framework dependency
# without the architecture gate refusing it. A gate never seen to refuse proves
# nothing, so for EVERY workspace member in turn this script injects `shaku`
# into that crate's manifest and requires the gate to refuse with exactly
# `ARCH-002 <crate>: undeclared dependency normal:shaku`. Anything else fails
# here: a PASS, another diagnostic, a checker that could not run `cargo
# metadata`, or a member count that differs from the policy's contracts.
#
# Everything happens on a COPY in a temporary directory; the live manifests,
# lockfile and policy are only read. The copy keeps the tree's depth below the
# GWZ root, so `binding-api`'s `../../../glade-decl-rs` resolves through a
# symlink to the real sibling, which is only ever read through.
set -eu
contract_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
gwz_root=$(CDPATH= cd -- "$contract_root/../.." && pwd)
# Explicit GWZ-local adoption, as in check.sh: the checker is tooling, never a
# crate dependency.
checker="$gwz_root/glade-discover/tools/architecture-check/Cargo.toml"
if [ ! -f "$checker" ]; then
    echo "Architecture checker unavailable: materialize the glade-discover GWZ member" >&2
    exit 1
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/glade-contracts-arch002.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
copy="$work/glade/contracts"
mkdir -p "$copy"
ln -s "$gwz_root/glade-decl-rs" "$work/glade-decl-rs"

# The members, from the one line that lists them. A line this cannot read
# yields a broken copy, which the positive control refuses to trust.
members=$(sed -n 's/^members = \[\(.*\)\]$/\1/p' "$contract_root/Cargo.toml" | tr -d '" ' | tr ',' ' ')
count=$(printf '%s\n' $members | grep -c .)
contracts=$(grep -c '"role": "contract"' "$contract_root/architecture-policy.json")
if [ "$count" != "$contracts" ]; then
    echo "ARCH-002 fixture: $count members but $contracts contracts in the policy; the fixture must cover every contract" >&2
    exit 1
fi
(cd "$contract_root" && tar -cf - Cargo.toml Cargo.lock architecture-policy.json $members) |
    (cd "$copy" && tar -xf -)

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

# 2. One crate at a time: inject, require the exact refusal, restore the copy's
#    manifest from the live one. A dotted table header is valid TOML whether or
#    not the manifest already has a [dependencies] table.
refused=0
for member in $members; do
    manifest="$copy/$member/Cargo.toml"
    name=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$manifest" | head -n 1)
    printf '\n[dependencies.shaku]\nversion = "=0.6.3"\n' >> "$manifest"
    expected="ARCH-002 $name: undeclared dependency normal:shaku"
    if diagnostic=$(gate); then
        echo "ARCH-002 fixture: the gate ACCEPTED a framework dependency in $name" >&2
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
        echo "ARCH-002 fixture: the gate refused $name, but not with the diagnostic this fixture is about" >&2
        echo "expected: $expected" >&2
        echo "actual:   $diagnostic" >&2
        exit 1
    fi
    printf '  refused: %s\n' "$diagnostic"
    cp "$contract_root/$member/Cargo.toml" "$manifest"
    refused=$((refused + 1))
done
printf 'ARCH-002 fixture: the gate refused the injected framework dependency in all %s contract crates\n' "$refused"
