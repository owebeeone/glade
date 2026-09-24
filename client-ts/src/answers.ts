// Op outcomes and the subscribe outcome (client-writes plan Steps 3.3 and 3.4;
// GladeSubstrateV1 §6, "Session answers" R1 and R5-R7, and "Cross-node writes"
// W5): the ops the client sent, kept by hash until the node's status names
// them, and the subscribes it sent, kept until each one's answer and replay
// are in. Pure, with no socket and no clock: the client applies what an answer
// means to its session, its listeners and its resends.

import { zoneKey, type Head, type Op } from "./store.ts";

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

/** The node's answer to a subscribe (R5, R6), given once its replay is in
 *  (R7): data, as an op's outcome is. */
export interface SubscribeOutcome {
  /** Acked, with the zone's replay in. */
  ok: boolean;
  /** The ack's heads: each origin's last seq in the zone, and that op's hash
   *  (R5). Empty for a refusal. */
  heads: Head[];
  /** `ok` when acked; a refusal's code, as the wire names it (R6); null when
   *  a refusal's reason did not come. */
  code: string | null;
  /** A refusal's reason. */
  message: string;
}

/** One zone's heads, as an ack names them (R5). */
export interface ZoneHeads {
  share: string;
  glade_id: string;
  key: Uint8Array;
  heads: Head[];
}

interface Subscribing {
  share: string;
  gladeId: string;
  zone: string;
  resolve: (outcome: SubscribeOutcome) => void;
  reject: (failure: Error) => void;
  /** The ack's heads, which the replay must reach (R7). */
  heads: Head[];
}

export class Replays {
  /** Sent and not yet acked, in the order sent, which is the order acks come in. */
  private unacked: Subscribing[] = [];
  /** Acked, with the replay not yet in. */
  private replaying: Subscribing[] = [];
  /** Refused by an ack that names no zone, until their reason comes (R6). */
  private refused: Subscribing[] = [];
  /** Each (zone, origin)'s highest seq this connection has received, or sent
   *  and had answered Ok (R7). No client announces heads in its Hello. */
  private reached = new Map<string, number>();

  /** Keep a subscribe the client sent until its answer and replay are in. */
  sent(share: string, gladeId: string, key: Uint8Array, resolve: (outcome: SubscribeOutcome) => void, reject: (failure: Error) => void): void {
    this.unacked.push({ share, gladeId, zone: zoneKey(share, gladeId, key), resolve, reject, heads: [] });
  }

  /** Take the next ack (R5, R6), which answers the oldest subscribe not yet
   *  acked. Returns whether that subscribe's replay was in at once. */
  ack(streams: ZoneHeads[]): boolean {
    const s = this.unacked.shift();
    if (!s) {
      return false;
    }
    if (streams.length === 0) {
      this.refused.push(s);
      return false;
    }
    const named = streams[0];
    if (zoneKey(named.share, named.glade_id, named.key) !== s.zone) {
      this.unacked.unshift(s);
      this.fail("an ack for another zone came in its subscribe's turn");
      return false;
    }
    s.heads = named.heads;
    this.replaying.push(s);
    return this.complete(s.zone);
  }

  /** A refused subscribe's reason: the next Error with no corr for its share
   *  and glade id (R6). */
  reason(share: string, gladeId: string, code: string, message: string): void {
    const at = this.refused.findIndex((s) => s.share === share && s.gladeId === gladeId);
    if (at >= 0) {
      this.refused.splice(at, 1)[0].resolve({ ok: false, heads: [], code, message });
    }
  }

  /** An op this connection received, or sent and had answered Ok (R7).
   *  Returns whether it brought in a subscribe's replay. */
  reach(op: Op): boolean {
    const zone = zoneKey(op.share, op.glade_id, op.key);
    const key = `${zone}\x00${op.origin}`;
    this.reached.set(key, Math.max(op.seq, this.reached.get(key) ?? op.seq));
    return this.complete(zone);
  }

  /** Fail the replays of `zones`, which a frame the client could not take
   *  would have carried; or, with none named, every waiting subscribe, since
   *  none can be matched to its answer any more. */
  fail(why: string, zones?: Set<string>): void {
    const failed = this.replaying.filter((s) => zones === undefined || zones.has(s.zone));
    this.replaying = this.replaying.filter((s) => !failed.includes(s));
    if (zones === undefined) {
      failed.push(...this.unacked.splice(0));
      // A refusal whose reason did not come is still a refusal.
      for (const s of this.refused.splice(0)) {
        s.resolve({ ok: false, heads: [], code: null, message: "refused; its reason did not come" });
      }
    }
    for (const s of failed) {
      s.reject(new Error(why));
    }
  }

  /** The connection ended: no ack, reason or op can come on it (R7). */
  ended(): void {
    this.fail("the connection ended");
    this.reached.clear();
  }

  /** Settle each subscribe of `zone` whose replay is in (R7). */
  private complete(zone: string): boolean {
    const done = this.replaying.filter((s) => s.zone === zone && s.heads.every((h) => (this.reached.get(`${zone}\x00${h.origin}`) ?? -Infinity) >= h.seq));
    this.replaying = this.replaying.filter((s) => !done.includes(s));
    for (const s of done) {
      s.resolve({ ok: true, heads: s.heads, code: "ok", message: "" });
    }
    return done.length > 0;
  }
}
