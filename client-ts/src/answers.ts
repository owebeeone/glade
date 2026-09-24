// Op outcomes (client-writes plan Step 3.3; GladeSubstrateV1 §6, "Session
// answers" R1 and R7, and "Cross-node writes" W5): the ops the client sent,
// kept by hash until the node's status names them. Pure, with no socket and no
// clock: the client applies what an answer means to its session, its
// listeners and its resends.

import { zoneKey, type Op } from "./store.ts";

/** The node's answer to one op the client sent (R1, R7): data, as an
 *  exchange's answer is. */
export interface OpOutcome {
  op: Op;
  /** Settled: the node holds the op (`ok`), or took it as below the first seq
   *  its chain holds (`retention`), which drops nothing. */
  ok: boolean;
  /** The node's code, as the wire names it. `unknown_share` means not placed
   *  (W5), and any other code but `ok` and `retention` a refusal. Null when no
   *  status came: the connection ended first, or the node never answers. */
  code: string | null;
  message: string;
}

/** What one status means to the client. */
export interface Answered {
  outcome: OpOutcome;
  /** The op's zone, keyed as the store keys it. */
  zone: string;
  /** A refusal: a session the client owns drops the op and its chain's tail. */
  refused: boolean;
  /** Not placed, for the first time: the client tells, and sends it again. */
  unplaced: boolean;
}

/** W5's backoff: the wait before a zone's resend `n` (from 0), 1 s doubling
 *  to 30 s. */
export function backoffMs(n: number): number {
  return Math.min(30_000, 1_000 * 2 ** n);
}

interface Waiting {
  op: Op;
  /** Statuses still owed: each send gets one (R1). */
  sends: number;
  waiters: Array<(outcome: OpOutcome) => void>;
}

function settle(waiters: Array<(outcome: OpOutcome) => void>, outcome: OpOutcome): void {
  for (const w of waiters) {
    w(outcome);
  }
}

export class Answers {
  /** Sent ops awaiting a status, by hash, the oldest first. */
  private waiting = new Map<string, Waiting>();
  /** Ops answered `unknown_share`, by zone, then by hash (W5). */
  private unplaced = new Map<string, Map<string, Op>>();
  /** Each zone's resends since it last had no unplaced op. */
  private resends = new Map<string, number>();
  /** At most this many ops wait on a status: a node from before the
   *  client-writes plan's Phase 2 answers none. */
  private bound: number;

  constructor(bound = 4096) {
    this.bound = bound;
  }

  /** Keep a sent op until its status names it by hash (R1). */
  sent(op: Op, hash: string, waiter?: (outcome: OpOutcome) => void): void {
    const w = this.waiting.get(hash) ?? { op, sends: 0, waiters: [] };
    w.sends += 1;
    if (waiter) {
      w.waiters.push(waiter);
    }
    this.waiting.set(hash, w);
    if (this.waiting.size > this.bound) {
      const [first, oldest] = this.waiting.entries().next().value as [string, Waiting];
      this.waiting.delete(first);
      settle(oldest.waiters, { op: oldest.op, ok: false, code: null, message: "no status came" });
    }
  }

  /** Take one status (R1). Returns what it means, or undefined for a hash no
   *  sent op waits on, which is ignored. */
  status(corr: string, code: string, message: string): Answered | undefined {
    const w = this.waiting.get(corr);
    if (!w) {
      return undefined;
    }
    w.sends -= 1;
    if (w.sends === 0) {
      this.waiting.delete(corr);
    }
    const outcome: OpOutcome = { op: w.op, ok: code === "ok" || code === "retention", code, message };
    settle(w.waiters.splice(0), outcome);
    const zone = zoneKey(w.op.share, w.op.glade_id, w.op.key);
    const kept = this.unplaced.get(zone) ?? new Map<string, Op>();
    const notPlaced = code === "unknown_share";
    const unplaced = notPlaced && !kept.has(corr);
    if (notPlaced) {
      kept.set(corr, w.op);
      this.unplaced.set(zone, kept);
    } else {
      kept.delete(corr);
    }
    if (!outcome.ok && !notPlaced) {
      // A refusal drops its chain's tail (answer 4), unplaced ops included.
      for (const [hash, op] of kept) {
        if (op.origin === w.op.origin && op.seq > w.op.seq) {
          kept.delete(hash);
        }
      }
    }
    if (kept.size === 0) {
      this.unplaced.delete(zone);
      this.resends.delete(zone);
    }
    return { outcome, zone, refused: !outcome.ok && !notPlaced, unplaced };
  }

  /** A zone's unplaced ops, in their chains' order, to send again (W5). */
  unplacedIn(zone: string): Op[] {
    const ops = [...(this.unplaced.get(zone)?.values() ?? [])];
    return ops.sort((a, b) => (a.origin < b.origin ? -1 : a.origin > b.origin ? 1 : a.seq - b.seq));
  }

  /** The wait before a zone's next resend (W5), which it counts. */
  nextResend(zone: string): number {
    const n = this.resends.get(zone) ?? 0;
    this.resends.set(zone, n + 1);
    return backoffMs(n);
  }

  /** The connection ended (R7): no waiting op will be answered, so each one's
   *  fate is unknown. Unplaced ops stay, to be sent again (W5). */
  ended(): void {
    for (const w of this.waiting.values()) {
      settle(w.waiters, { op: w.op, ok: false, code: null, message: "the connection ended" });
    }
    this.waiting.clear();
  }
}
