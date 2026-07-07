"""Glade system-data records + the SystemSnapshot blob (GDL-036, Lane R step 1).

Design: glial-dev/dev-docs/glade/GladeSystemDataSeam.md (the seam) and
`GladeWorkspaceDirectory.md` §2 (the home-share record kinds). Authored INSIDE
the glade repo (mirrors `glade/demo/ir/workspace.taut.py`): it bootstraps the
taut builder from the sibling `../taut/src` gwz member and generates native
Rust into `glade/node/src/sysdata.rs`.

Two layers:
  1. the home-share record kinds (WD §2) — each rides an Op.payload as opaque
     taut bytes; the RegistryApi fold decodes them to answer queries.
  2. `SystemSnapshot{records, heads}` — the whole system state as ONE taut
     message (the interim StoreApi blob). `records`/`heads` are lists of CBOR
     bytes: each record is an encoded wire `Op` (glade-wire, the single Op
     type), each head an encoded wire `StreamHeads`. A snapshot IS a cached
     fold + heads (SubstrateV1 §2) — keeping records as encoded wire Ops means
     no second Op type is minted; op-granular sync later reuses the SAME bytes.

REGENERATE (never hand-edit the generated .rs):
    cd glade-wz/taut && PYTHONPATH=src python3 -m taut.cli gen \
        ../glade/node/ir/sysdata.taut.py -o /tmp/sysdata-gen -l rust --api-only
    # then copy /tmp/sysdata-gen/rust/api.rs -> glade/node/src/sysdata.rs
"""

import sys
from pathlib import Path

# glade/node/ir -> glade-wz/taut/src (the taut builder, a sibling gwz member).
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "taut" / "src"))

from taut.ir.dsl import BYTES, INT, STR, F, List, Msg, schema

SCHEMA = schema(
    # ---- home-share record kinds (WD §2) ----------------------------------
    # A node's machine identity + its operator. nodes_of(operator) folds these.
    Msg("NodeRecord",
        F("node_id", 1, STR),
        F("operator", 2, STR)),

    # A workspace directory entry. LWW-per-field at the fold; eligible_hosts are
    # the nodes with a checkout. replicas_of(share) reads these.
    Msg("WorkspaceEntry",
        F("workspace", 1, STR),
        F("name", 2, STR),
        F("eligible_hosts", 3, List(STR))),

    # A leased "node X serves share W" claim. Lease expiry is evaluated at
    # projection/read time (never inside the fold); highest live epoch wins.
    Msg("ServeClaim",
        F("node", 1, STR),
        F("share", 2, STR),
        F("lease_expiry_ms", 3, INT),
        F("epoch", 4, INT)),

    # who may do what. Set-union at the fold; a matching CapabilityRevocation
    # wins. grants_for(principal, share) folds grants minus revocations.
    Msg("CapabilityGrant",
        F("principal", 1, STR),
        F("share", 2, STR),
        F("verbs", 3, List(STR))),
    Msg("CapabilityRevocation",
        F("principal", 1, STR),
        F("share", 2, STR)),

    # ---- the snapshot wrapper (substrate vocabulary, not a hack) -----------
    # The whole system state as ONE taut message: a cached fold + heads.
    # records[i] = CBOR(wire Op)   heads[j] = CBOR(wire StreamHeads).
    Msg("SystemSnapshot",
        F("records", 1, List(BYTES)),
        F("heads", 2, List(BYTES))),
)
