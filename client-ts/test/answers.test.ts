// Op outcomes and the subscribe outcome (client-writes plan Steps 3.3 and 3.4;
// GladeSubstrateV1 §6, "Session answers" R1 and R5-R7, and "Cross-node writes"
// W5). The answers and replays tables are pure, and the client reads them over
// a fake socket: no node, no socket and no clock (LBT-008). The backoff runs on
// node:test's mock timers.

import test, { mock } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import * as codec from "../src/taut/codec.ts";
import { Answers, Replays, backoffMs, type OpOutcome, type SubscribeOutcome } from "../src/answers.ts";
import { GladeClient } from "../src/client.ts";
import { Session, UnresumedChain, type Op } from "../src/session.ts";
import { zoneKey } from "../src/store.ts";
import { hex, utf8 } from "../src/bytes.ts";
import { opHash } from "../src/hash.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

const hashOf = (op: Op) => hex(opHash(schema, op as never));
const zone = zoneKey("sh", "g", new Uint8Array());

/** `n` ops on one origin's chain in the zone ("sh", "g"). */
function chain(origin: string, n: number): Op[] {
  const session = new Session(schema, origin);
  const ops: Op[] = [];
  for (let i = 0; i < n; i++) {
    ops.push(session.append("sh", "g", "value", utf8(`${origin}-${i}`)));
  }
  return ops;
}

/** Keep `op` as sent, and catch the outcome it resolves with. */
function send(answers: Answers, op: Op): { outcome?: OpOutcome } {
  const caught: { outcome?: OpOutcome } = {};
  answers.sent(op, hashOf(op), (o) => {
    caught.outcome = o;
  });
  return caught;
}

test("a status resolves its op by its hash", () => {
  const [op] = chain("a", 1);
  const answers = new Answers();
  const caught = send(answers, op);
  assert.equal(answers.status(hashOf(op), "ok", "appended")?.refused, false);
  assert.deepEqual(caught.outcome, { op, ok: true, code: "ok", message: "appended" });
  // A settled op leaves the table, so a later status for it is ignored.
  assert.equal(answers.status(hashOf(op), "ok", "already held"), undefined);
});

test("retention settles an op, and is no refusal", () => {
  const [op] = chain("a", 1);
  const answers = new Answers();
  const caught = send(answers, op);
  assert.equal(answers.status(hashOf(op), "retention", "below the first seq held")?.refused, false);
  assert.equal(caught.outcome?.ok, true);
  assert.equal(caught.outcome?.code, "retention");
});

test("a refusal resolves its op as refused, and the session drops its tail", () => {
  const session = new Session(schema, "a");
  const ops = [0, 1, 2].map((i) => session.append("sh", "g", "value", utf8(`v${i}`)));
  const answers = new Answers();
  const caught = ops.map((op) => send(answers, op));
  answers.status(hashOf(ops[0]), "ok", "appended");
  assert.equal(answers.status(hashOf(ops[1]), "equivocation", "forked")?.refused, true);
  assert.deepEqual(caught[1].outcome, { op: ops[1], ok: false, code: "equivocation", message: "forked" });
  // The tail waits on its own statuses (R1).
  assert.equal(caught[2].outcome, undefined);

  // Answer 4: the refused op and every later op of its chain go, and the
  // chain takes no append until a subscribe of its zone resumes it.
  session.refuse(ops[1]);
  assert.deepEqual(session.dump(), [ops[0]]);
  assert.throws(() => session.append("sh", "g", "value", utf8("next")), UnresumedChain);
  session.resume("sh", "g", new Uint8Array());
  assert.equal(session.append("sh", "g", "value", utf8("next")).seq, 1);
  // Another origin's refused op is left alone.
  const bystander = new Session(schema, "a");
  bystander.refuse(chain("b", 1)[0]);
  assert.equal(bystander.append("sh", "g", "value", utf8("still")).seq, 0);
});

test("an unknown hash is ignored", () => {
  const [op] = chain("a", 1);
  const answers = new Answers();
  const caught = send(answers, op);
  assert.equal(answers.status("00".repeat(32), "equivocation", "forked"), undefined);
  const untouched = caught.outcome;
  assert.equal(untouched, undefined);
  answers.status(hashOf(op), "ok", "appended");
  assert.equal(caught.outcome?.ok, true);
});

test("the connection's end leaves every waiting op unknown, and keeps the unplaced", () => {
  const ops = chain("a", 3);
  const answers = new Answers();
  const caught = ops.map((op) => send(answers, op));
  answers.status(hashOf(ops[0]), "unknown_share", "no live claim");
  answers.ended();
  for (const i of [1, 2]) {
    assert.deepEqual(caught[i].outcome, { op: ops[i], ok: false, code: null, message: "the connection ended" });
  }
  // No status for them can come on a new connection.
  assert.equal(answers.status(hashOf(ops[1]), "ok", "appended"), undefined);
  // An op not placed is still the client's to send again (W5).
  assert.deepEqual(answers.unplacedIn(zone), [ops[0]]);
});

test("the fire-and-forget table is bounded", () => {
  // A node from before Phase 2 answers no op: the oldest waiting op goes
  // first, its fate unknown.
  const ops = chain("a", 3);
  const answers = new Answers(2);
  const caught = ops.map((op) => send(answers, op));
  assert.deepEqual(caught[0].outcome, { op: ops[0], ok: false, code: null, message: "no status came" });
  assert.equal(answers.status(hashOf(ops[0]), "ok", "appended"), undefined);
  answers.status(hashOf(ops[2]), "ok", "appended");
  assert.equal(caught[2].outcome?.ok, true);
});

test("an op not placed is not refused: it and its chain are kept, in order", () => {
  const ops = chain("a", 3);
  const answers = new Answers();
  const caught = ops.map((op) => send(answers, op));
  const answered = ops.map((op) => answers.status(hashOf(op), "unknown_share", "no live claim"));
  assert.deepEqual(answered.map((a) => [a?.refused, a?.unplaced]), [[false, true], [false, true], [false, true]]);
  assert.deepEqual(caught[0].outcome, { op: ops[0], ok: false, code: "unknown_share", message: "no live claim" });
  assert.deepEqual(answers.unplacedIn(zone), ops);
  // Sent again and still not placed, it is kept, and not told of again.
  send(answers, ops[0]);
  assert.equal(answers.status(hashOf(ops[0]), "unknown_share", "no live claim")?.unplaced, false);
  assert.deepEqual(answers.unplacedIn(zone), ops);
});

test("a repeat's Ok settles an op that was not placed", () => {
  const [op] = chain("a", 1);
  const answers = new Answers();
  send(answers, op);
  answers.status(hashOf(op), "unknown_share", "no live claim");
  const again = send(answers, op);
  answers.status(hashOf(op), "ok", "already held");
  assert.equal(again.outcome?.ok, true);
  assert.deepEqual(answers.unplacedIn(zone), []);
});

test("a later refusal still drops the tail", () => {
  const ops = chain("a", 3);
  const [other] = chain("b", 1);
  const answers = new Answers();
  for (const op of [...ops, other]) {
    send(answers, op);
    answers.status(hashOf(op), "unknown_share", "no live claim");
  }
  // Sent again: the first lands and the second is refused, so the third is
  // kept no longer. Another chain's op in the zone stays.
  for (const op of ops) {
    send(answers, op);
  }
  answers.status(hashOf(ops[0]), "ok", "appended");
  assert.equal(answers.status(hashOf(ops[1]), "equivocation", "forked")?.refused, true);
  assert.deepEqual(answers.unplacedIn(zone), [other]);
});

test("the backoff's schedule: 1 s doubling to 30 s, per zone, afresh once placed", () => {
  assert.deepEqual([0, 1, 2, 3, 4, 5, 6, 7].map(backoffMs), [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  const [op] = chain("a", 1);
  const elsewhere = zoneKey("sh", "h", new Uint8Array());
  const answers = new Answers();
  send(answers, op);
  answers.status(hashOf(op), "unknown_share", "no live claim");
  assert.deepEqual([0, 1, 2].map(() => answers.nextResend(zone)), [1000, 2000, 4000]);
  assert.equal(answers.nextResend(elsewhere), 1000);
  send(answers, op);
  answers.status(hashOf(op), "ok", "appended");
  assert.equal(answers.nextResend(zone), 1000);
});

// ---- catching up (R5-R7) -----------------------------------------------------

const empty = new Uint8Array();

/** An ack of a subscribe to ("sh", `gladeId`) naming each [origin, seq] (R5). */
function acked(heads: Array<[string, number]>, gladeId = "g") {
  return [{ share: "sh", glade_id: gladeId, key: empty, heads: heads.map(([origin, seq]) => ({ origin, seq, hash: null })) }];
}

/** Keep a subscribe to ("sh", `gladeId`) as sent, and catch how it settles. */
function subscribing(replays: Replays, gladeId = "g"): { outcome?: SubscribeOutcome; failure?: Error } {
  const caught: { outcome?: SubscribeOutcome; failure?: Error } = {};
  replays.sent("sh", gladeId, empty, (o) => {
    caught.outcome = o;
  }, (e) => {
    caught.failure = e;
  });
  return caught;
}

test("an ack with no origins completes at once, as does one the connection has reached", () => {
  const replays = new Replays();
  const first = subscribing(replays);
  assert.equal(replays.ack(acked([])), true);
  assert.deepEqual(first.outcome, { ok: true, heads: [], code: "ok", message: "" });
  // An op received on this connection before the ack counts (R7).
  replays.reach(chain("a", 1)[0]);
  const second = subscribing(replays);
  assert.equal(replays.ack(acked([["a", 0]])), true);
  assert.equal(second.outcome?.ok, true);
});

test("a replay is in once each origin reaches its acked seq: below waits, at or above is done", () => {
  const ops = chain("a", 3);
  const replays = new Replays();
  const at = subscribing(replays);
  assert.equal(replays.ack(acked([["a", 1], ["b", 0]])), false);
  assert.equal(replays.reach(ops[0]), false);
  assert.equal(replays.reach(chain("b", 1)[0]), false);
  assert.equal(replays.reach(ops[1]), true);
  assert.deepEqual(at.outcome?.heads.map((h) => [h.origin, h.seq]), [["a", 1], ["b", 0]]);
  const fresh = new Replays();
  const above = subscribing(fresh);
  fresh.ack(acked([["a", 1]]));
  assert.equal(fresh.reach(ops[2]), true);
  assert.equal(above.outcome?.ok, true);
});

test("an ack that names no zone is a refusal, and its reason is the next Error for its zone with no corr", () => {
  const replays = new Replays();
  const caught = subscribing(replays);
  assert.equal(replays.ack([]), false);
  replays.reason("sh", "other", "unknown_share", "not this subscribe's");
  const waiting = caught.outcome;
  assert.equal(waiting, undefined);
  replays.reason("sh", "g", "unknown_share", "no such share");
  assert.deepEqual(caught.outcome, { ok: false, heads: [], code: "unknown_share", message: "no such share" });
});

test("the connection's end fails every waiting subscribe, and what it reached is forgotten", () => {
  const replays = new Replays();
  const refused = subscribing(replays, "r");
  replays.ack([]);
  const replaying = subscribing(replays);
  replays.ack(acked([["a", 0]]));
  const unacked = subscribing(replays);
  replays.reach(chain("b", 1)[0]);
  replays.ended();
  // A refusal whose reason did not come is still a refusal.
  assert.deepEqual(refused.outcome, { ok: false, heads: [], code: null, message: "refused; its reason did not come" });
  assert.match(String(replaying.failure), /the connection ended/);
  assert.match(String(unacked.failure), /the connection ended/);
  const next = subscribing(replays);
  assert.equal(replays.ack(acked([["b", 0]])), false);
  assert.equal(next.outcome, undefined);
});

test("a frame the client cannot take fails its zones' replays; an ack for another zone fails them all", () => {
  const replays = new Replays();
  const mine = subscribing(replays);
  replays.ack(acked([["a", 0]]));
  const other = subscribing(replays, "h");
  replays.ack(acked([["a", 0]], "h"));
  replays.fail("an op it cannot take", new Set([zone]));
  assert.match(String(mine.failure), /an op it cannot take/);
  const untouched = other.failure;
  assert.equal(untouched, undefined);
  // Acks come in the order the subscribes went, so an ack for another zone
  // means none can be matched to its subscribe any more.
  const next = subscribing(replays);
  assert.equal(replays.ack(acked([], "elsewhere")), false);
  assert.match(String(next.failure), /another zone/);
  assert.match(String(other.failure), /another zone/);
});

// ---- the client, over a fake socket ------------------------------------------

/** Stands in for the global WebSocket: it opens at once, keeps what the
 *  client sends, and delivers what a node would. */
class FakeSocket {
  static last: FakeSocket;
  readyState = 1;
  binaryType = "blob";
  sent: Uint8Array[] = [];
  onopen?: () => void;
  onmessage?: (ev: { data: ArrayBuffer }) => void;
  onclose?: () => void;
  onerror?: () => void;

  constructor(readonly url: string) {
    FakeSocket.last = this;
    queueMicrotask(() => this.onopen?.());
  }
  send(bytes: Uint8Array): void {
    this.sent.push(bytes);
  }
  close(): void {
    this.readyState = 3;
    this.onclose?.();
  }
  /** Every op the client has sent, in order. */
  ops(): Op[] {
    return this.sent.filter((b) => b[0] === 4).flatMap((b) => codec.decode(schema, "Ops", b.slice(1)).ops as Op[]);
  }
  /** The node's status for `op` (R1). */
  status(op: Op, code: string): void {
    this.deliver(12, "Error", { code, message: code, share: op.share, glade_id: op.glade_id, corr: hashOf(op) });
  }
  /** The node's ack of a subscribe to ("sh", `gladeId`), naming each [origin, seq] (R5). */
  ack(heads: Array<[string, number]> = [], gladeId = "g"): void {
    this.deliver(5, "Heads", { streams: acked(heads, gladeId) });
  }
  /** The node's refusal of a subscribe to ("sh", "g") (R6). */
  refuse(code: string, message: string): void {
    this.deliver(5, "Heads", { streams: [] });
    this.deliver(12, "Error", { code, message, share: "sh", glade_id: "g", corr: null });
  }
  /** Ops from the node: a replay, or live ops. */
  receive(ops: Op[]): void {
    this.deliver(4, "Ops", { ops, pri: null });
  }
  /** Bytes as they came, which need not be a frame. */
  raw(bytes: Uint8Array): void {
    this.onmessage?.({ data: bytes.slice().buffer });
  }
  private deliver(tag: number, message: string, value: unknown): void {
    const body = codec.encode(schema, message, value as never);
    const bytes = new Uint8Array(1 + body.length);
    bytes[0] = tag;
    bytes.set(body, 1);
    this.raw(bytes);
  }
}

/** Lets pending promise callbacks run. */
const turn = () => new Promise((resolve) => setImmediate(resolve));

async function fakeClient(origin: string): Promise<{ client: GladeClient; socket: FakeSocket }> {
  const real = globalThis.WebSocket;
  globalThis.WebSocket = FakeSocket as never;
  try {
    const client = new GladeClient(schema, origin);
    await client.connect("ws://fake");
    return { client, socket: FakeSocket.last };
  } finally {
    globalThis.WebSocket = real;
  }
}

test("an outcome call fails at once when no socket is open, and leaves the chain alone", async () => {
  const never = new GladeClient(schema, "a");
  await assert.rejects(never.appendOutcome("sh", "g", "value", utf8("x")), /not connected/);
  await assert.rejects(never.sendOpsOutcome(chain("a", 1)), /not connected/);
  await assert.rejects(never.subscribeOutcome("sh", "g"), /not connected/);
  assert.deepEqual(never.session.dump(), []);
  const { client, socket } = await fakeClient("a");
  socket.close();
  await assert.rejects(client.appendOutcome("sh", "g", "value", utf8("x")), /not connected/);
  assert.deepEqual(client.session.dump(), []);
});

test("a session the client owns drops a refused op until a subscribe of its zone has its replay", async () => {
  const { client, socket } = await fakeClient("a");
  const told: OpOutcome[] = [];
  client.onRefused((o) => told.push(o));
  const op = client.append("sh", "g", "value", utf8("x"));
  socket.status(op, "equivocation");
  assert.deepEqual(told.map((o) => o.code), ["equivocation"]);
  assert.deepEqual(client.session.dump(), []);
  assert.throws(() => client.append("sh", "g", "value", utf8("y")), UnresumedChain);
  // The ack alone does not resume the chain; the replay that brings the op
  // the node holds at seq 0 does.
  const subscribed = client.subscribe("sh", "g");
  socket.ack([["a", 0]]);
  assert.throws(() => client.append("sh", "g", "value", utf8("y")), UnresumedChain);
  socket.receive(chain("a", 1));
  await subscribed;
  assert.equal(client.append("sh", "g", "value", utf8("y")).seq, 1);
});

test("a session a binder owns is only told, and a refusal no listener takes goes to the console", async () => {
  const { client, socket } = await fakeClient("a");
  client.onOps = () => {};
  const op = client.session.append("sh", "g", "value", utf8("x"));
  client.sendOps([op]);
  const warn = mock.method(console, "warn", () => {});
  try {
    socket.status(op, "equivocation");
    assert.equal(warn.mock.callCount(), 1);
    assert.match(String(warn.mock.calls[0].arguments[0]), /refused: equivocation/);
  } finally {
    warn.mock.restore();
  }
  assert.deepEqual(client.session.dump(), [op]);
  assert.equal(client.session.append("sh", "g", "value", utf8("y")).seq, 1);
});

test("an op not placed is told once, and sent again on the backoff and once its zone's replay is in", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const { client, socket } = await fakeClient("a");
    const told: OpOutcome[] = [];
    client.onUnplaced((o) => told.push(o));
    const first = client.append("sh", "g", "value", utf8("x"));
    socket.status(first, "unknown_share");
    // W5: kept, so its chain goes on.
    const second = client.append("sh", "g", "value", utf8("y"));
    socket.status(second, "unknown_share");
    assert.deepEqual(told.map((o) => o.op.seq), [0, 1]);
    mock.timers.tick(999);
    assert.equal(socket.ops().length, 2);
    mock.timers.tick(1);
    assert.deepEqual(socket.ops().map((o) => o.seq), [0, 1, 0, 1]);
    socket.status(first, "unknown_share");
    socket.status(second, "unknown_share");
    assert.equal(told.length, 2);
    mock.timers.tick(2000);
    assert.equal(socket.ops().length, 6);
    // Not at the ack: once the replay is in.
    const subscribed = client.subscribe("sh", "g");
    socket.ack([["b", 0]]);
    assert.equal(socket.ops().length, 6);
    socket.receive(chain("b", 1));
    await subscribed;
    assert.equal(socket.ops().length, 8);
    socket.status(first, "ok");
    socket.status(second, "ok");
    mock.timers.tick(60_000);
    assert.equal(socket.ops().length, 8);
    client.close();
  } finally {
    mock.timers.reset();
  }
});

test("a refused subscribe resolves as an empty zone, and subscribeOutcome gives its reason", async () => {
  const { client, socket } = await fakeClient("a");
  const plain = client.subscribe("sh", "g");
  socket.refuse("unknown_share", "no live claim");
  await plain;
  const outcome = client.subscribeOutcome("sh", "g");
  socket.refuse("unknown_share", "no live claim");
  assert.deepEqual(await outcome, { ok: false, heads: [], code: "unknown_share", message: "no live claim" });
  assert.equal(client.fold("sh", "g", "value"), null);
});

test("the session's own ops count toward its replay once answered Ok", async () => {
  // The node sends an op's status before a later subscribe's ack, and its gap
  // leaves out what the session sent and the node holds (R4). Here the ack
  // comes first, to show that the op counts only once it is answered.
  const { client, socket } = await fakeClient("a");
  const op = client.append("sh", "g", "value", utf8("x"));
  let returned = false;
  const subscribed = client.subscribe("sh", "g").then(() => {
    returned = true;
  });
  socket.ack([["a", 0]]);
  await turn();
  assert.equal(returned, false);
  socket.status(op, "ok");
  await subscribed;
  assert.equal(returned, true);
});

test("a frame the client cannot read fails every waiting subscribe, and goes to the console", async () => {
  const { client, socket } = await fakeClient("a");
  const replaying = client.subscribe("sh", "g");
  socket.ack([["b", 0]]);
  const unacked = client.subscribe("sh", "h");
  const warn = mock.method(console, "warn", () => {});
  try {
    socket.raw(new Uint8Array([99, 0xff]));
    assert.equal(warn.mock.callCount(), 1);
  } finally {
    warn.mock.restore();
  }
  await assert.rejects(replaying, /could not take/);
  await assert.rejects(unacked, /could not take/);
});

test("an Ops frame the client cannot take fails its zone's replay, and no other", async () => {
  const { client, socket } = await fakeClient("a");
  const [theirs] = chain("b", 1);
  const here = client.subscribe("sh", "g");
  socket.ack([["b", 0]]);
  const elsewhere = client.subscribe("sh", "h");
  socket.ack([["b", 0]], "h");
  const warn = mock.method(console, "warn", () => {});
  try {
    socket.receive([{ ...theirs, shape: "stream" }]);
    assert.equal(warn.mock.callCount(), 1);
  } finally {
    warn.mock.restore();
  }
  await assert.rejects(here, /could not take/);
  socket.receive([{ ...theirs, glade_id: "h" }]);
  await elsewhere;
});

test("a replay a consumer throws on still counts as received", async () => {
  const { client, socket } = await fakeClient("a");
  client.onOps = () => {
    throw new Error("the binder threw");
  };
  const subscribed = client.subscribe("sh", "g");
  socket.ack([["b", 0]]);
  assert.throws(() => socket.receive(chain("b", 1)), /the binder threw/);
  await subscribed;
});
