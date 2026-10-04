// Actual frozen Glial/Taut engine consumes histories recovered from real nodes.
import assert from "node:assert/strict";
import fs from "node:fs";
import { pathToFileURL } from "node:url";
const { assembleTextCrdt } = await import(pathToFileURL(process.argv[2]));
const byte = (hex) => hex === "-" ? null : Uint8Array.from(Buffer.from(hex, "hex"));
const text = (hex) => new TextDecoder("utf-8", { fatal: true }).decode(byte(hex));
const fork = process.argv[3] === "--fork";
const snapshots = process.argv.slice(fork ? 4 : 3).map((path) => {
  const rows = fs.readFileSync(path, "utf8").trim().split("\n").filter(Boolean).map((row) => {
    const [origin, seq, lamport, prev, refs, payload, eligible] = row.split("\t");
    assert.ok(eligible === undefined || eligible === "0" || eligible === "1");
    return {
      eligible: eligible === undefined || eligible === "1",
      origin: text(origin), seq: Number(seq), lamport: Number(lamport), prev: byte(prev), payload: byte(payload),
      refs: refs === "-" ? [] : refs.split(",").map((ref) => {
        const [origin, seq, hash] = ref.split(":");
        return { origin: text(origin), seq: Number(seq), hash: byte(hash) };
      }),
    };
  });
  assert.equal(rows.length, 2);
  const ops = rows.filter((row) => row.eligible);
  assert.equal(ops.length, fork ? 0 : 2);
  if (fork) {
    assert.equal(rows[0].origin, rows[1].origin);
    assert.equal(rows[0].seq, rows[1].seq);
  }
  const forward = assembleTextCrdt(ops);
  const reverse = assembleTextCrdt([...ops].reverse());
  assert.equal(forward.text, reverse.text);
  assert.equal(forward.text, fork ? "" : "AB");
  assert.deepEqual(forward.diagnostics, []);
  return forward;
});
assert.equal(snapshots.length, 2);
assert.equal(snapshots[0].text, snapshots[1].text);
console.log(`Actual two-process validated eligible cut -> released Glial/Taut text ${fork ? "empty (fork quarantine)" : "AB"} in both arrival orders PASS`);
