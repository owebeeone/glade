"""Gryth workspace demo — app surface types (taut).

The demo's shared surfaces, addressed by glade id (a stable key, like a Grip
key) and carrying a *declared* taut payload type — the thing both client and
(eventually) provider agree on. For now this declares the payload messages;
the surface registry (glade id -> shape + type) is what the `.glade` compiler
will generate. The wire/frame protocol is separate (`taut/ir/glade.taut.py`).

Surfaces (glade id -> shape : payload):
  app:selection  value : str   (lww — a file path)
  app:notes      value : str   (lww — shared notes)
  app:activity   log   : ChatLine
"""

import sys
from pathlib import Path

# glade/demo/ir -> glial-dev/taut/src
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "taut" / "src"))

from taut.ir.dsl import INT, STR, F, Msg, schema

SCHEMA = schema(
    # one activity-log entry: when, who, what.
    Msg("ChatLine",
        F("ts", 1, INT),       # epoch millis
        F("user", 2, STR),     # origin / author
        F("text", 3, STR)),    # the message
)
