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
        ../glade/node/ir/sysdata.taut.py -o /tmp/sysdata-gen -l rust --api-only \
        --legacy-codec
    # then copy /tmp/sysdata-gen/rust/api.rs -> glade/node/src/sysdata.rs
    # --legacy-codec matches glade-wire's frozen cbor runtime (fail-open);
    # removed at taut v0.10 — see GladeGrazelAttachNotes.md ambiguity #1.
"""

import sys
from pathlib import Path

# glade/node/ir -> glade-wz/taut/src (the taut builder, a sibling gwz member).
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "taut" / "src"))

from taut.ir.dsl import BOOL, BYTES, INT, STR, F, List, Msg, schema

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

    # ---- app declaration records (GDL-037/038, Lane R step 4) --------------
    # A <app>.glade file's declarations REGISTERED as ordinary records: the
    # loader appends these under the registrant's chain — byte-identical to
    # what dynamic configuration would write. Base glade folds them without
    # knowing any app; grazel is just the first contributor.
    #
    # A declared binding surface (glade-decl vocabulary, GladeDeclSurface.md):
    # app-static — no share/key here; the ServeClaim selects the node and the
    # mount fills domain/zone/key. shape/authority/zone/retention ride as
    # strings (data, not enums) so the record evolves additively.
    Msg("BindingDecl",
        F("app", 1, STR),
        F("glade_id", 2, STR),
        F("shape", 3, STR),
        F("authority", 4, STR),
        F("zone", 5, STR),
        F("retention", 6, STR)),

    # A declared service: the authority provider an app attaches, named with
    # the EXCHANGE glade id it answers (directed frames route to it — never a
    # replica; the fan-out asymmetry).
    Msg("ServiceDefinition",
        F("app", 1, STR),
        F("name", 2, STR),
        F("glade_id", 3, STR)),

    # ---- the create ceremony (GLP-0006 P0.S2 — audit F2, s-create D1–D3) ---
    # Rides ExchangeReq.payload on the reserved built-in `workspace.create`
    # surface, handled by the NODE (never a supplier): creation PRECEDES
    # claims, so the request names its TARGET node explicitly — the one routed
    # operation that cannot consult a ServeClaim (it makes the thing claims
    # will be about). Node-local system data; the wire IR is untouched.
    Msg("WorkspaceCreateReq",
        F("workspace", 1, STR),
        F("name", 2, STR),
        F("target", 3, STR)),
    # The answer: which node performed it. created=False = the target already
    # served that workspace (re-create is idempotent — records diff away).
    Msg("WorkspaceCreateRes",
        F("workspace", 1, STR),
        F("node", 2, STR),
        F("created", 3, BOOL)),

    # ---- the snapshot wrapper (substrate vocabulary, not a hack) -----------
    # The whole system state as ONE taut message: a cached fold + heads.
    # records[i] = CBOR(wire Op)   heads[j] = CBOR(wire StreamHeads).
    Msg("SystemSnapshot",
        F("records", 1, List(BYTES)),
        F("heads", 2, List(BYTES))),
)
