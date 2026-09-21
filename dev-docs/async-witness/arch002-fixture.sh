#!/bin/sh
# Step 3.5's negative fixture, executable and failing CLOSED.
#
# The DI-E04 claim is that `async-witness-ports` stays framework-free while a
# real iroh-backed provider fills its `CarrierPort`. A gate that has never been
# seen to reject anything is not evidence for that, so this script injects
# `shaku` into the contract crate's manifest and requires the architecture gate
# to refuse it with the exact `ARCH-002` naming `async-witness-ports`.
#
# It passes on that diagnostic and on nothing else. A non-zero exit for any
# other reason — a broken copy, a checker that could not run `cargo metadata`, a
# different error code — is a FAILURE here, because each of those would let a
# gate that had stopped working look like a gate that was working.
#
# Everything happens on a COPY. The live `ports/Cargo.toml`, `Cargo.lock` and
# `architecture-policy.json` are never edited, not even for the length of one
# command, so an interrupted run cannot leave the tree changed. The copy keeps
# the same depth below a `glade` root that the real tree has, so the members'
# `../../../` path dependencies resolve; the three siblings they name are
# symlinks, which this script only ever reads through.
set -eu
witness_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
glade_root=$(CDPATH= cd -- "$witness_root/../.." && pwd)
# Explicit GWZ-local adoption, as in check.sh: the checker is tooling, never a
# crate dependency.
checker="$witness_root/../../../glade-discover/tools/architecture-check/Cargo.toml"
if [ ! -f "$checker" ]; then
    echo "Architecture checker unavailable: materialize the glade-discover GWZ member" >&2
    exit 1
fi

expected="ARCH-002 async-witness-ports: undeclared dependency normal:shaku"

work=$(mktemp -d "${TMPDIR:-/tmp}/async-witness-arch002.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

fixture="$work/glade/dev-docs/async-witness"
mkdir -p "$fixture"
(cd "$witness_root" && tar -cf - Cargo.toml Cargo.lock architecture-policy.json ports fast real) |
    (cd "$fixture" && tar -xf -)
ln -s "$glade_root/wire-rs" "$work/glade/wire-rs"
ln -s "$glade_root/node" "$work/glade/node"
ln -s "$glade_root/contracts" "$work/glade/contracts"

# 1. The positive control. Without it, "the gate failed" could mean "the copy
#    was broken", and the fixture would prove nothing about the gate.
if ! control=$(cargo run --quiet --locked --offline --manifest-path "$checker" -- "$fixture" 2>&1); then
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

# 2. Inject the framework dependency into the CONTRACT crate's manifest, in the
#    copy. `[lints]` follows `[dependencies]` there, so the entry is inserted
#    after the section header rather than appended to the file; awk reports an
#    absent header rather than silently writing a file with no injection.
if ! awk '
    /^\[dependencies\]$/ { print; print "shaku = \"=0.6.3\""; injected = 1; next }
    { print }
    END { if (injected != 1) { exit 3 } }
' "$fixture/ports/Cargo.toml" > "$fixture/ports/Cargo.toml.injected"; then
    echo "ARCH-002 fixture: ports/Cargo.toml has no [dependencies] header to inject into" >&2
    exit 1
fi
mv "$fixture/ports/Cargo.toml.injected" "$fixture/ports/Cargo.toml"

# 3. The gate must now refuse, and refuse for exactly this reason.
if diagnostic=$(cargo run --quiet --locked --offline --manifest-path "$checker" -- "$fixture" 2>&1); then
    echo "ARCH-002 fixture: the gate ACCEPTED a framework dependency in the contract crate" >&2
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
    echo "ARCH-002 fixture: the gate refused, but not with the diagnostic this fixture is about" >&2
    echo "expected: $expected" >&2
    echo "actual:   $diagnostic" >&2
    exit 1
fi

printf 'ARCH-002 fixture: the gate refused the injected framework dependency\n  %s\n' "$diagnostic"
