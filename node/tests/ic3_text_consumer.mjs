// Qualification consumer runs the actual pinned Glial decodeTextEdit; no copied decoder.
import assert from "node:assert/strict";
import fs from "node:fs";
import { pathToFileURL } from "node:url";
const { decodeTextEdit } = await import(pathToFileURL(process.argv[2]));
const rows = fs.readFileSync(new URL("fixtures/ic3_text_payload_rows.tsv", import.meta.url), "utf8").trim().split("\n");
for (const line of rows) {
  const [expected, hex, name] = line.split("\t");
  let valid = false;
  try {
    decodeTextEdit(Uint8Array.from(Buffer.from(hex, "hex")));
    valid = true;
  } catch {
    valid = false;
  }
  assert.equal(valid, expected === "1", name);
}
console.log(`Actual released Glial text decoder: ${rows.length} shared rows PASS`);
