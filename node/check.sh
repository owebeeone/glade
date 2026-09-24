#!/bin/sh
# Plan Step 3.5's gate (dev-docs/GladeFirstSlicePlan.md at the glade-wz root,
# "Step 3.5 -- The gate"): one script no step of Phases 3-4 can pass without.
# The witness's check.sh (../dev-docs/async-witness/check.sh) is the pattern;
# AR-09 (dev-docs/arch1/RuntimeAndAssurance.md:127) is the criterion --
# dependency inversion and the isolated fast targets fail closed.
#
# Every component runs to completion, so each run reports all of them, then the
# named gaps and what the gate does NOT check. The exit status is 0 only when
# every component passed.
#
#   architecture       glade-discover's checker over this workspace, against
#                      architecture-policy.json
#   arch002-node       arch002-fixture.sh: that checker is seen to refuse dill,
#                      a framework glade-node may not declare, injected into it,
#                      on a copy
#   arch002-contracts  glade/contracts/arch002-fixture.sh, plan Step 3.1's
#                      fixture for the port side; absent means red
#   confinement        cargo tree --invert: each framework is seen only by the
#                      crates the allowlist below names, on every target
#                      platform for the node
#   node-tests         cargo test for this workspace, twice: GLADE_NODE_ASSEMBLED
#                      unset, so every glade-node the tests spawn starts from the
#                      hand-written composition root, then =1, so each starts
#                      from the assembled one (plan Step 3.2); both must pass
#   contracts-gate     glade/contracts/check.sh (its checker, tests, fmt, clippy)
#   fmt, clippy        by package: a held package must pass; a package whose
#                      debt predates this gate is counted and printed as a
#                      named gap, never fixed here and never hidden, and is
#                      ratcheted: a count above its recorded baseline fails
#
# Cargo reads CARGO_TARGET_DIR from the environment and nothing here sets or
# overrides it, so a caller can keep every build out of the repositories. Every
# cargo command passes --locked --offline where it accepts them (cargo fmt
# accepts neither; it only reads, with --check), and none writes a manifest or
# a lockfile. Sub-scripts are run with `sh`, as the other gates run theirs.
set -u

if [ "$#" -ne 0 ]; then
    echo "usage: check.sh   (no arguments; honours CARGO_TARGET_DIR from the environment)" >&2
    exit 2
fi

node_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
glade_root=$(CDPATH= cd -- "$node_root/.." && pwd -P)
contracts_root="$glade_root/contracts"
node_manifest="$node_root/Cargo.toml"
node_policy="$node_root/architecture-policy.json"
contracts_manifest="$contracts_root/Cargo.toml"
# Explicit GWZ-local adoption: the checker is tooling, never a crate dependency.
checker="$node_root/../../glade-discover/tools/architecture-check/Cargo.toml"

# `cargo tree --invert` confinement, as an explicit allowlist: workspace,
# framework, and the local crates allowed to see it, comma-separated ("-" is
# none). A local crate is a path package -- ours. The inverted tree lists every
# package that reaches the framework transitively, over normal, build and dev
# edges with all features on, so a registry or git crate in between does not
# launder an edge.
#
# iroh may be seen only by glade-node, whose peer carrier it is, shaku only by
# glade-node, whose assembly it is (plan Step 3.2, src/assembly.rs), and the
# sdax family only by glade-node, whose lifecycle it is (plan Step 3.3,
# src/lifecycle.rs; sdax-testkit as a dev-dependency). The contracts workspace
# must never reach iroh, tokio, shaku or sdax.
confinement_allowlist='
node       iroh          glade-node
node       shaku         glade-node
node       sdax          glade-node
node       sdax-tokio    glade-node
node       sdax-testkit  glade-node
contracts  iroh          -
contracts  tokio         -
contracts  shaku         -
contracts  sdax          -
contracts  sdax-tokio    -
contracts  sdax-testkit  -
'
# A real edge confinement must SEE: glade-node's peer carrier depends on iroh.
# If this is not seen, the check is blind, not passing.
confinement_witness='node iroh glade-node'
# The workspaces whose confinement must cover every target platform (owner,
# 2026-09-24, plan Step 3.5): `cargo tree --target all` must resolve them from
# the offline cache, or the component fails. One online `cargo fetch --locked`
# filled the cache with the node's platform crates; a dependency change that
# needs new ones needs that fetch again. A workspace not named here falls back
# to the host target, and the narrowing is printed as a named gap.
confinement_all_targets='node'

# fmt and clippy dispositions, by package: name, fmt, clippy. `held`: the
# check must pass. `gap:N`: debt that predates this gate -- counted and printed
# on every run as a named gap, never fixed here -- and ratcheted (owner,
# 2026-09-24, plan Step 3.5) at the baseline N: rustfmt hunks for fmt, clippy
# warnings for clippy. A count above N fails the component. A count below N
# passes, and the gate says N can be lowered to it, an edit to this table. A
# package in scope that this table does not name is held, so a new crate
# starts clean. A name here that is no longer in scope fails the component, as
# a stale entry.
style_dispositions='
glade-node  gap:337  gap:11
glade-wire  gap:43   gap:7
'

results=$(mktemp -d "${TMPDIR:-/tmp}/glade-node-gate.XXXXXX") || exit 1
trap 'rm -rf "$results"' EXIT
trap 'exit 1' HUP INT TERM
: > "$results/summary"
: > "$results/gaps"
: > "$results/outside"

# why TEXT: the one-line reason the current component reports in the summary.
why() {
    printf '%s\n' "$*" > "$results/reason"
}

# gap TEXT: a named gap, printed at the end of the run.
gap() {
    printf '%s\n' "$*" >> "$results/gaps"
}

# rel PATH: PATH relative to the glade repository, for messages.
rel() {
    case "$1" in
        "$glade_root"/*) printf '%s\n' "glade/${1#"$glade_root"/}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

# run_component NAME FUNCTION: runs FUNCTION in a subshell and records PASS or
# FAIL with its reason. A component never stops the ones after it.
run_component() {
    printf '\n== %s\n' "$1"
    rm -f "$results/reason"
    ( "$2" )
    status=$?
    reason=""
    if [ -f "$results/reason" ]; then
        reason=$(cat "$results/reason")
    fi
    if [ "$status" -eq 0 ]; then
        verdict=PASS
    else
        verdict=FAIL
    fi
    printf '%s\t%s\t%s\n' "$verdict" "$1" "$reason" >> "$results/summary"
    printf '%s\n' "-- $1: $verdict${reason:+ -- $reason}"
}

# tree_of MANIFEST [ARGS...]: one `{p}` line per package the workspace reaches.
tree_of() {
    tree_manifest=$1
    shift
    cargo tree --locked --offline --manifest-path "$tree_manifest" --workspace \
        --all-features -e normal,build,dev --prefix none --format '{p}' "$@"
}

# note_lockfile DIR: --locked is only as good as the lockfile it holds to.
note_lockfile() {
    if [ ! -f "$1/Cargo.lock" ]; then
        gap "lockfile     $(rel "$1")/Cargo.lock is absent: every --locked command against it fails until one is generated deliberately, outside this gate"
    elif ! git -C "$1" ls-files --error-unmatch -- Cargo.lock > /dev/null 2>&1; then
        gap "lockfile     $(rel "$1")/Cargo.lock is not tracked by git: --locked holds this gate to this checkout's lockfile, not to a reviewed one"
    fi
}

c_architecture() {
    if [ ! -f "$checker" ]; then
        echo "Architecture checker unavailable: materialize the glade-discover GWZ member" >&2
        why "the checker is unavailable (glade-discover/tools/architecture-check)"
        return 1
    fi
    if ! out=$(cargo run --quiet --locked --offline --manifest-path "$checker" -- "$node_root" 2>&1); then
        printf '%s\n' "$out" >&2
        why "the checker refused the node workspace against glade/node/architecture-policy.json"
        return 1
    fi
    printf '%s\n' "$out"
    case "$out" in
        *"Architecture boundaries: PASS"*) : ;;
        *)
            why "the checker exited 0 without reporting PASS"
            return 1
            ;;
    esac
    why "every library in the node workspace is classified; declared dependencies of every kind and target match glade/node/architecture-policy.json exactly"
    return 0
}

c_arch002_node() {
    if sh "$node_root/arch002-fixture.sh"; then
        why "the checker refused dill injected into glade-node, as a normal and as a cfg(windows) dependency, on a copy"
        return 0
    fi
    why "glade/node/arch002-fixture.sh failed closed; its message above names the branch"
    return 1
}

c_arch002_contracts() {
    fixture="$contracts_root/arch002-fixture.sh"
    if [ ! -f "$fixture" ]; then
        echo "glade/contracts/arch002-fixture.sh is absent: plan Step 3.1 has not landed the port side's ARCH-002 fixture" >&2
        why "absent: plan Step 3.1 has not landed glade/contracts/arch002-fixture.sh, so the port side has never been seen to refuse a framework"
        return 1
    fi
    if sh "$fixture"; then
        why "glade/contracts/arch002-fixture.sh saw the contracts' checker refuse an injected framework"
        return 0
    fi
    why "glade/contracts/arch002-fixture.sh failed"
    return 1
}

# select_targets LABEL MANIFEST: inspect every target platform, resolved
# offline. A workspace named in confinement_all_targets fails when Cargo cannot;
# any other falls back to the host target, with the narrowing named as a gap.
select_targets() {
    if out=$(tree_of "$2" --target all 2>&1); then
        printf '%s\n' "$out" > "$results/tree.$1"
        echo all > "$results/targets.$1"
        return 0
    fi
    case "$out" in
        *"--offline was specified"* | *"failed to download"*)
            first=$(printf '%s\n' "$out" | sed -n 's/^error: failed to download `\(.*\)`$/\1/p' | head -n 1)
            case " $confinement_all_targets " in
                *" $1 "*)
                    printf '%s\n' "$out" >&2
                    echo "  $1: every target platform is required, and --target all needs crates absent from the offline cache (first: ${first:-unknown}); \`cargo fetch --locked --manifest-path $(rel "$2")\`, once and online, fetches them" >&2
                    return 1
                    ;;
            esac
            if ! out=$(tree_of "$2" 2>&1); then
                printf '%s\n' "$out" >&2
                return 1
            fi
            printf '%s\n' "$out" > "$results/tree.$1"
            echo host > "$results/targets.$1"
            gap "confinement  the $1 workspace was inspected on the host target only: --target all needs crates absent from the offline cache (first: ${first:-unknown}); \`cargo fetch --locked --manifest-path $(rel "$2")\`, once and online, closes this gap"
            return 0
            ;;
    esac
    printf '%s\n' "$out" >&2
    return 1
}

c_confinement() {
    echo "allowlist (workspace, framework, local crates allowed to see it):"
    printf '%s\n' "$confinement_allowlist" | sed -n 's/^\(..*\)$/  \1/p'
    echo "every target platform required for: $confinement_all_targets"
    for workspace in node contracts; do
        case "$workspace" in
            node) manifest=$node_manifest ;;
            *) manifest=$contracts_manifest ;;
        esac
        if ! select_targets "$workspace" "$manifest"; then
            why "cargo tree could not resolve the $workspace workspace, locked and offline, for the targets it must cover"
            return 1
        fi
    done
    bad=0
    while read -r workspace framework allowed; do
        if [ -z "$workspace" ]; then
            continue
        fi
        case "$workspace" in
            node) manifest=$node_manifest ;;
            contracts) manifest=$contracts_manifest ;;
            *)
                echo "  unknown workspace in the allowlist: $workspace"
                bad=1
                continue
                ;;
        esac
        target_flag=""
        if [ "$(cat "$results/targets.$workspace")" = all ]; then
            target_flag="--target all"
        fi
        versions=$(awk -v f="$framework" '$1 == f { sub(/^v/, "", $2); print $2 }' "$results/tree.$workspace" | sort -u)
        inverted=""
        : > "$results/seen.$workspace.$framework"
        if [ -n "$versions" ]; then
            set --
            for version in $versions; do
                set -- "$@" --invert "$framework@$version"
            done
            # $target_flag is deliberately unquoted: empty, or two words. Inside
            # a loop that reads its table from stdin, no child may read stdin.
            if ! inverted=$(tree_of "$manifest" $target_flag "$@" 2>&1 < /dev/null); then
                printf '%s\n' "$inverted" >&2
                echo "  $workspace: cargo tree --invert $framework failed"
                bad=1
                continue
            fi
            printf '%s\n' "$inverted" | awk '/ \(\// { print $1 }' | sort -u > "$results/seen.$workspace.$framework"
        fi
        seen=$(tr '\n' ' ' < "$results/seen.$workspace.$framework")
        breach=""
        for crate in $seen; do
            case ",$allowed," in
                *",$crate,"*) : ;;
                *) breach="$breach $crate" ;;
            esac
        done
        if [ -n "$versions" ]; then
            reach="seen by: ${seen:-no local crate}"
        else
            reach="absent"
        fi
        if [ -n "$breach" ]; then
            printf '  %-10s %-13s %s -- BREACH:%s may not see %s\n' "$workspace" "$framework" "$reach" "$breach" "$framework"
            printf '%s\n' "$inverted" >&2
            bad=1
        else
            printf '  %-10s %-13s %s -- ok\n' "$workspace" "$framework" "$reach"
        fi
    done <<EOF
$confinement_allowlist
EOF
    set -- $confinement_witness
    if ! grep -qx -- "$3" "$results/seen.$1.$2" 2>/dev/null; then
        echo "  $1: confinement did not see $3 reach $2, an edge that exists; the check is blind"
        bad=1
    fi
    # Crate-level only: say where iroh is named inside glade-node, textually.
    named=$(cd "$node_root" && grep -rl 'iroh::' src tests 2> /dev/null | grep -v '^src/iroh_carrier\.rs$' | sort | tr '\n' ' ' | sed 's/ $//')
    if [ -n "$named" ]; then
        count=$(printf '%s\n' $named | grep -c .)
        gap "confinement  iroh is confined by crate (glade-node), not by module: $count file(s) outside src/iroh_carrier.rs name \`iroh::\` paths (a textual scan, not syntax-aware): $named"
    fi
    targets=""
    for workspace in node contracts; do
        if [ "$(cat "$results/targets.$workspace")" = all ]; then
            label="all targets"
        else
            label="host target only"
        fi
        targets="$targets${targets:+; }$workspace: $label"
    done
    if [ "$bad" -ne 0 ]; then
        why "a framework is seen by a crate the allowlist does not name, or the check could not see ($targets)"
        return 1
    fi
    why "every framework seen only by allowlisted crates; the witness edge glade-node -> iroh seen ($targets)"
    return 0
}

# tee_status LOG COMMAND...: stream COMMAND's output and keep a copy; POSIX sh
# has no pipefail, so the status travels through a file.
tee_status() {
    tee_log=$1
    shift
    { "$@"; echo $? > "$tee_log.status"; } 2>&1 | tee "$tee_log"
    return "$(cat "$tee_log.status")"
}

# tests_hand_written, tests_assembled: the workspace's tests, with the
# composition root every spawned glade-node starts from chosen explicitly, so a
# GLADE_NODE_ASSEMBLED in the caller's environment decides nothing.
tests_hand_written() {
    (
        unset GLADE_NODE_ASSEMBLED
        cargo test --locked --offline --manifest-path "$node_manifest" --workspace
    )
}

tests_assembled() {
    GLADE_NODE_ASSEMBLED=1 cargo test --locked --offline --manifest-path "$node_manifest" --workspace
}

c_node_tests() {
    bad=0
    counts=""
    for root in hand-written assembled; do
        case "$root" in
            hand-written) runner=tests_hand_written ;;
            *) runner=tests_assembled ;;
        esac
        echo "-- cargo test --workspace, every spawned glade-node from the $root composition root"
        log="$results/node-tests.$root.log"
        if tee_status "$log" "$runner"; then
            passed=$(awk '/^test result: ok\./ { n += $4 } END { print n + 0 }' "$log")
            binaries=$(grep -c '^test result: ok\.' "$log")
            counts="$counts${counts:+; }$root: $passed tests passed across $binaries test binaries"
        else
            counts="$counts${counts:+; }$root: FAILED"
            bad=1
        fi
    done
    if [ "$bad" -ne 0 ]; then
        why "cargo test --locked --offline --workspace failed for the node workspace ($counts)"
        return 1
    fi
    why "cargo test --locked --offline --workspace, GLADE_NODE_ASSEMBLED unset then =1 -- $counts"
    return 0
}

c_contracts_gate() {
    if [ ! -f "$contracts_root/check.sh" ]; then
        why "glade/contracts/check.sh is absent"
        return 1
    fi
    if sh "$contracts_root/check.sh"; then
        why "glade/contracts/check.sh passed (its checker, tests, fmt --check and clippy -D warnings, by package)"
        return 0
    fi
    why "glade/contracts/check.sh failed"
    return 1
}

# style_scope: "name dir" for every local package the node workspace reaches on
# the host target that this repository owns and no other gate here holds. The
# contract crates are held by glade/contracts/check.sh; a path package outside
# this repository is its own repository's business, listed under "not checked".
style_scope() {
    if ! lines=$(tree_of "$node_manifest" 2>&1); then
        printf '%s\n' "$lines" >&2
        return 1
    fi
    printf '%s\n' "$lines" | sed -n 's/^\([^ ]*\) v[^ ]* (\(\/[^)]*\)).*$/\1 \2/p' | sort -u |
        while read -r name dir; do
            case "$dir/" in
                "$contracts_root"/*) : ;;
                "$glade_root"/*) printf '%s %s\n' "$name" "$dir" ;;
                *) printf '%s (%s) is outside this repository: its own gate holds its format and lints\n' "$name" "$dir" >> "$results/outside" ;;
            esac
        done
}

# disposition NAME COLUMN: held or gap, from style_dispositions (2 fmt, 3 clippy).
disposition() {
    printf '%s\n' "$style_dispositions" |
        awk -v n="$1" -v c="$2" '$1 == n { print $c; found = 1 } END { if (!found) { print "held" } }'
}

# stale_dispositions SCOPE: a name in style_dispositions must still be in scope.
stale_dispositions() {
    stale=0
    for listed in $(printf '%s\n' "$style_dispositions" | awk 'NF { print $1 }'); do
        if ! printf '%s\n' "$1" | awk '{ print $1 }' | grep -qx -- "$listed"; then
            echo "  $listed: named in style_dispositions but no longer in scope (a stale entry)"
            stale=1
        fi
    done
    return "$stale"
}

# baseline_of MODE: N, from a `gap:N` disposition; fails when N is not a count.
baseline_of() {
    n=${1#gap:}
    case "$n" in
        '' | *[!0-9]*)
            return 1
            ;;
    esac
    printf '%s\n' "$n"
}

# against COUNT BASELINE: where a counted gap stands against its ratchet.
against() {
    if [ "$1" -gt "$2" ]; then
        printf 'ABOVE its baseline of %s: the gap grew' "$2"
    elif [ "$1" -lt "$2" ]; then
        printf 'below its baseline of %s: the baseline can be lowered to %s' "$2" "$1"
    else
        printf 'at its baseline of %s' "$2"
    fi
}

c_fmt() {
    if ! scope=$(style_scope); then
        why "could not list the packages in scope"
        return 1
    fi
    bad=0
    stale_dispositions "$scope" || bad=1
    held=""
    while read -r name dir; do
        if [ -z "$name" ]; then
            continue
        fi
        mode=$(disposition "$name" 2)
        out=$(cargo fmt --manifest-path "$dir/Cargo.toml" -p "$name" -- --check 2> "$results/fmt.err" < /dev/null)
        status=$?
        hunks=$(printf '%s\n' "$out" | grep -c '^Diff in ')
        if grep -q '^error' "$results/fmt.err" || { [ "$status" -ne 0 ] && [ "$hunks" -eq 0 ]; }; then
            cat "$results/fmt.err" >&2
            echo "  $name: rustfmt could not check the package"
            bad=1
            continue
        fi
        case "$mode" in
            held)
                if [ "$status" -ne 0 ]; then
                    printf '%s\n' "$out"
                    echo "  $name: held, and NOT rustfmt-clean ($hunks hunks)"
                    bad=1
                else
                    echo "  $name: rustfmt-clean (held)"
                    held="$held $name"
                fi
                ;;
            gap:*)
                if ! baseline=$(baseline_of "$mode"); then
                    echo "  $name: fmt disposition '$mode' does not end in a count"
                    bad=1
                    continue
                fi
                files=$(printf '%s\n' "$out" | sed -n 's/^Diff in \(.*\):[0-9]*:$/\1/p' | sort -u | grep -c .)
                fix="cargo fmt --manifest-path $(rel "$dir")/Cargo.toml -p $name -- --check"
                standing=$(against "$hunks" "$baseline")
                echo "  $name: $hunks rustfmt hunks in $files files, $standing (a named gap, not held)"
                if [ "$hunks" -gt "$baseline" ]; then
                    echo "  $name: rustfmt hunks by file ($fix prints them):"
                    printf '%s\n' "$out" | sed -n 's/^Diff in \(.*\):[0-9]*:$/\1/p' | sort | uniq -c |
                        while read -r count file; do
                            printf '    %4s  %s\n' "$count" "$(rel "$file")"
                        done
                    bad=1
                fi
                if [ "$hunks" -eq 0 ]; then
                    gap "fmt          $name: 0 hunks -- the gap is closed; set its fmt disposition to held"
                else
                    gap "fmt          $name: $hunks rustfmt hunks in $files files, $standing; not fixed by this gate ($fix)"
                fi
                ;;
            *)
                echo "  $name: unknown disposition '$mode'"
                bad=1
                ;;
        esac
    done <<EOF
$scope
EOF
    if [ "$bad" -ne 0 ]; then
        why "a held package is not rustfmt-clean, a counted gap rose above its baseline, rustfmt could not run, or a disposition is stale"
        return 1
    fi
    why "held:${held:- none}; every other package in scope is a counted gap at or below its baseline"
    return 0
}

# clippy_warnings NAME LOG: distinct warnings, from Cargo's per-target summaries
# ("generated N warnings (M duplicates)"), which a replay from cache also prints.
clippy_warnings() {
    awk -v tick='`' -v n="$1" '
        $1 == "warning:" && $2 == tick n tick {
            generated = 0
            duplicates = 0
            for (i = 3; i <= NF; i++) {
                if ($i == "generated") { generated = $(i + 1) + 0 }
                if ($i ~ /^duplicates?\)/) { d = $(i - 1); gsub(/\(/, "", d); duplicates = d + 0 }
            }
            total += generated - duplicates
        }
        END { print total + 0 }
    ' "$2"
}

c_clippy() {
    if ! scope=$(style_scope); then
        why "could not list the packages in scope"
        return 1
    fi
    bad=0
    stale_dispositions "$scope" || bad=1
    held=""
    while read -r name dir; do
        if [ -z "$name" ]; then
            continue
        fi
        root=$(cargo locate-project --workspace --locked --offline --message-format plain --manifest-path "$dir/Cargo.toml" 2> /dev/null < /dev/null)
        if [ -n "$root" ]; then
            note_lockfile "$(dirname -- "$root")"
        fi
        mode=$(disposition "$name" 3)
        case "$mode" in
            held)
                echo "  $name: linting (held, -D warnings)"
                if cargo clippy --locked --offline --manifest-path "$dir/Cargo.toml" -p "$name" --all-targets --all-features -- -D warnings < /dev/null; then
                    echo "  $name: clippy-clean under -D warnings (held)"
                    held="$held $name"
                else
                    echo "  $name: held, and NOT clippy-clean under -D warnings"
                    bad=1
                fi
                ;;
            gap:*)
                if ! baseline=$(baseline_of "$mode"); then
                    echo "  $name: clippy disposition '$mode' does not end in a count"
                    bad=1
                    continue
                fi
                # Without -D warnings, so every target is linted and counted;
                # an error is a failure, never a counted gap.
                echo "  $name: linting (a gap: counted and ratcheted, not held)"
                if ! cargo clippy --locked --offline --manifest-path "$dir/Cargo.toml" -p "$name" --all-targets --all-features > "$results/clippy.log" 2>&1 < /dev/null; then
                    cat "$results/clippy.log" >&2
                    echo "  $name: clippy could not check the package"
                    bad=1
                    continue
                fi
                count=$(clippy_warnings "$name" "$results/clippy.log")
                fix="cargo clippy --manifest-path $(rel "$dir")/Cargo.toml -p $name --all-targets --all-features"
                standing=$(against "$count" "$baseline")
                echo "  $name: $count clippy warnings, $standing (a named gap, not held)"
                if [ "$count" -gt "$baseline" ]; then
                    echo "  $name: the warnings, each once ($fix prints them in full):"
                    awk '/^warning: / && !/ generated [0-9]+ warning/ { w = $0; next }
                         w != "" && /^ *--> / { print "    " $2 "  " w; w = "" }' "$results/clippy.log" | sort -u
                    bad=1
                fi
                if [ "$count" -eq 0 ]; then
                    gap "clippy       $name: 0 warnings -- the gap is closed; set its clippy disposition to held"
                else
                    gap "clippy       $name: $count warnings, $standing; not fixed by this gate ($fix)"
                fi
                ;;
            *)
                echo "  $name: unknown disposition '$mode'"
                bad=1
                ;;
        esac
    done <<EOF
$scope
EOF
    if [ "$bad" -ne 0 ]; then
        why "a held package is not clippy-clean, a counted gap rose above its baseline, clippy could not run, or a disposition is stale"
        return 1
    fi
    why "held:${held:- none}; every other package in scope is a counted gap at or below its baseline"
    return 0
}

echo "glade/node/check.sh -- plan Step 3.5's gate"
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    echo "  CARGO_TARGET_DIR=$CARGO_TARGET_DIR (from the environment)"
else
    echo "  CARGO_TARGET_DIR unset: each workspace builds into its own target/ directory"
fi
echo "  $(cargo --version 2>&1)"
note_lockfile "$node_root"
if [ ! -f "$node_root/Cargo.lock" ]; then
    echo "  glade/node/Cargo.lock is absent; this gate never creates one, so every --locked command against it fails" >&2
fi
if grep -q PROVISIONAL "$node_policy" 2> /dev/null; then
    gap "policy       glade/node/architecture-policy.json classifies glade-node PROVISIONALLY: owner review pending"
fi
gap "checker      glade-discover's checker skips items under #[cfg] or #[cfg_attr] without evaluating the condition: a required item there is reported missing (fails closed), but nothing else there -- a module, a public trait, type or impl -- is ever examined, and #[cfg_attr(all(), path = \"...\")] bypasses its #[path] refusal (measured in glade/dev-docs/async-witness/README.md). Dependency checks read cargo metadata and are unaffected."

run_component architecture c_architecture
run_component arch002-node c_arch002_node
run_component arch002-contracts c_arch002_contracts
run_component confinement c_confinement
run_component node-tests c_node_tests
run_component contracts-gate c_contracts_gate
run_component fmt c_fmt
run_component clippy c_clippy

printf '\nSummary\n'
awk -F '\t' '{ printf "  %s  %-18s %s\n", $1, $2, $3 }' "$results/summary"

printf '\nNamed gaps (recorded, counted where countable, never fixed by this gate)\n'
categories="policy checker lockfile confinement fmt clippy"
for category in $categories; do
    awk -v c="$category" '$1 == c && !seen[$0]++ { print "  " $0 }' "$results/gaps"
done
# Nothing recorded is dropped: a gap under any other category prints last.
awk -v known=" $categories " 'index(known, " " $1 " ") == 0 && !seen[$0]++ { print "  " $0 }' "$results/gaps"

cat <<'EOF'

Not checked. The five complementary checks of
dev-docs/LibraryBoundaryAndTestingPolicy.md:88-94 are the checklist:
  1 inventory/graph      PERFORMED. Unclassified libraries and undeclared direct
                         dependencies of every kind, target and rename fail, in
                         both workspaces; the named frameworks are confined by
                         cargo tree --invert. NOT a resolved audit of any other
                         third-party transitive dependency.
  2 source structure     PERFORMED where a policy names traits (the contracts).
                         glade-node is classified integration and names none, so
                         for it the checker only parses modules and finds its test
                         targets. The checker's #[cfg] blind spot is a gap above.
  3 compiler witness     PERFORMED for the node's assembly: its module compiles
                         only with every binding bound once and no constructor
                         cycle, and node-tests runs its compile_fail doctests (a
                         missing binding, a cycle, an ambiguous role, a carrier
                         asked for by port type). Stable rustdoc checks that each
                         fails, not its error code (the codes are recorded in
                         glade/dev-docs/GladeNodeAssembly.md).
  4 behavioural          PERFORMED for the contracts' own suites (their check.sh),
    conformance          and by node-tests for the node's deterministic providers
                         (tests/assembly: CL, CA, SI, GR) and the fail-closed half
                         of the assembled path's grant fold and signer (GR-003,
                         SI-003). NOT PERFORMED for any real adapter (LBT-009):
                         none implements CarrierPort, GrantPort or SignerPort yet.
  5 CI invocation        NOT PERFORMED. This is a local script: no CI job runs it
                         and no required merge check exists (a hosting setting).
Also not checked: public boundary types and transitive type leakage (LBT-004,
review only); test determinism (LBT-008) -- the node suite starts real iroh
endpoints on loopback and is the whole suite, not a measured fast loop
(LBT-010); disabled platform branches -- tests and clippy build the host target
only, so code under another platform's #[cfg] is neither compiled nor linted
(the assembled root's stop signal off Unix is one such branch);
the standing rule that #[cfg] sits inside cfg_if! or a platform module; a new
rustfmt deviation a few lines from an existing one -- the fmt ratchet counts
hunks, and rustfmt merges nearby deviations into one hunk, so such a line need
not raise the count; the witness workspace (glade/dev-docs/async-witness, its
own check.sh); the checker's own tests (glade-discover); branch protection.
EOF
if [ -s "$results/outside" ]; then
    awk '!seen[$0]++ { print "Also not checked: " $0 }' "$results/outside"
fi

failed=$(awk -F '\t' '$1 == "FAIL" { printf "%s%s", sep, $2; sep = ", " }' "$results/summary")
total=$(grep -c . "$results/summary")
if [ -n "$failed" ]; then
    count=$(awk -F '\t' '$1 == "FAIL"' "$results/summary" | grep -c .)
    printf '\nGATE: FAIL -- %s of %s components failed: %s\n' "$count" "$total" "$failed"
    exit 1
fi
printf '\nGATE: PASS -- all %s components passed; the gaps above stand\n' "$total"
exit 0
