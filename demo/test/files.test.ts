import test from "node:test";
import assert from "node:assert/strict";

import { assembleSwmr, encodeSwmrAction, utf8 } from "@owebeeone/glial-runtime";
import { M } from "../src/manifest.ts";
import { FILE_CODEC, projectFileEvent } from "../src/files.ts";

test("the demo's canonical ws.files surface is SWMR", () => {
  assert.equal(M.files.glade_id.id, "ws.files");
  assert.equal(M.files.shape, "swmr");
});

test("the demo declares collaborative notes as a CRDT surface", () => {
  assert.equal(M.collaborativeNotes.glade_id.id, "app:collaborative-notes");
  assert.equal(M.collaborativeNotes.shape, "crdt");
});

test("the demo projects one bounded full-image file generation", () => {
  const swmr = assembleSwmr([
    { origin: "writer-a", seq: 1, lamport: 1, prev: null, payload: encodeSwmrAction("snapshot", utf8("abcdef")) },
    { origin: "writer-a", seq: 2, lamport: 2, prev: null, payload: encodeSwmrAction("delta", utf8("uvwxyz")) },
  ], "ws.files");
  const window = projectFileEvent({ swmr } as never);
  assert.equal(FILE_CODEC.decode(window.bytes), "uvwxyz");
  assert.equal(window.revision, "0:1");
});
