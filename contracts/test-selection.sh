#!/bin/sh
set -eu
selection_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
for name in binding invocation subscription sync lifecycle persistence carrier clock grant signer; do
    actual=$(sh "$selection_root/check.sh" --list "$name")
    test "$actual" = "glade-$name-api"
done
for name in crdt-admission crdt-storage-attempt; do
    test "$(sh "$selection_root/check.sh" --list "$name")" = "$(printf 'glade-crdt-storage-attempt-api\nglade-crdt-admission-core')"
done
test "$(sh "$selection_root/check.sh" --list all | wc -l | tr -d ' ')" = 16
for name in crdt-data crdt-evidence crdt-recovery crdt-codec; do
    case "$name" in
        crdt-data) expected="glade-crdt-admission-data" ;;
        crdt-evidence) expected="glade-crdt-admission-data glade-crdt-evidence-api" ;;
        crdt-recovery) expected="glade-crdt-admission-data glade-crdt-recovery-api" ;;
        crdt-codec) expected="glade-crdt-admission-data glade-crdt-recovery-codec" ;;
    esac
    test "$(sh "$selection_root/check.sh" --list "$name" | tr '\n' ' ' | sed 's/ $//')" = "$expected"
done
if sh "$selection_root/check.sh" --list invalid >/dev/null 2>&1; then
    echo "Invalid selector was accepted" >&2
    exit 1
fi
echo "Contract test selection: PASS"
