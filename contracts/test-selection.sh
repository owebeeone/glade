#!/bin/sh
set -eu
selection_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
for name in binding invocation subscription sync lifecycle persistence carrier clock grant signer; do
    actual=$(sh "$selection_root/check.sh" --list "$name")
    test "$actual" = "glade-$name-api"
done
test "$(sh "$selection_root/check.sh" --list all | wc -l | tr -d ' ')" = 10
if sh "$selection_root/check.sh" --list invalid >/dev/null 2>&1; then
    echo "Invalid selector was accepted" >&2
    exit 1
fi
echo "Contract test selection: PASS"
