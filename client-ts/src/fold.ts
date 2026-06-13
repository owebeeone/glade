// The folds — pure functions of the op-set, so every replica converges. The
// browser folds (GQ-8); these reproduce taut/corpus/glade_folds.json
// byte-for-byte (the cross-language fold oracle).
//
// value (lww): winner = max by (lamport, origin).
// log: deterministic order by (lamport, origin, seq).
// Both dedup by (origin, seq); a different payload/prev at the same (origin,seq)
// is equivocation (a forked chain) — detected, never folded.

import { bytesEq } from "./bytes.ts";

export interface FoldOp {
  origin: string;
  seq: number;
  lamport: number;
  prev: Uint8Array | null;
  payload: Uint8Array;
}

export class Equivocation extends Error {}

function dedup(ops: FoldOp[]): FoldOp[] {
  const seen = new Map<string, FoldOp>();
  for (const op of ops) {
    const k = `${op.origin}\x00${op.seq}`;
    const prior = seen.get(k);
    if (!prior) {
      seen.set(k, op);
    } else if (!bytesEq(prior.payload, op.payload) || !prevEq(prior.prev, op.prev)) {
      throw new Equivocation(`forked chain at (${op.origin},${op.seq})`);
    }
  }
  return [...seen.values()];
}

function prevEq(a: Uint8Array | null, b: Uint8Array | null): boolean {
  if (a === null || b === null) return a === b;
  return bytesEq(a, b);
}

export function foldValue(ops: FoldOp[]): Uint8Array | null {
  const live = dedup(ops);
  if (live.length === 0) return null;
  let win = live[0];
  for (const o of live) {
    if (o.lamport > win.lamport || (o.lamport === win.lamport && o.origin > win.origin)) {
      win = o;
    }
  }
  return win.payload;
}

export function foldLog(ops: FoldOp[]): Uint8Array[] {
  const live = dedup(ops);
  live.sort(
    (a, b) =>
      a.lamport - b.lamport ||
      (a.origin < b.origin ? -1 : a.origin > b.origin ? 1 : 0) ||
      a.seq - b.seq,
  );
  return live.map((o) => o.payload);
}

export function isEquivocation(ops: FoldOp[]): boolean {
  try {
    dedup(ops);
    return false;
  } catch (e) {
    if (e instanceof Equivocation) return true;
    throw e;
  }
}
