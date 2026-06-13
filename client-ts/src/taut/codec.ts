// IR-driven codec — native value <-> CBOR bytes, driven by the schema. The
// TypeScript mirror of taut/src/taut/wire/codec.py. A "native value" is a
// plain object keyed by field name (enums as member-name strings, bytes as
// Uint8Array). The wire is a projection of the tagged subset: messages -> CBOR
// maps keyed by field tag, transient fields skipped, optionals always emitted
// (null when absent).

import { type CborValue, decode as cborDecode, encode as cborEncode } from "./cbor.ts";
import { type SchemaIndex, type TypeRef } from "./schema.ts";

// deno-lint friendly: native values are dynamic by nature.
type Native = any; // eslint-disable-line @typescript-eslint/no-explicit-any

function toWire(schema: SchemaIndex, t: TypeRef, value: Native): CborValue {
  switch (t.k) {
    case "scalar":
      return value as CborValue;
    case "enum":
      return schema.enumDef(t.name).members[value as string];
    case "list":
      return (value as Native[]).map((v) => toWire(schema, t.elem, v));
    case "map": {
      const entries = [...(value as Map<Native, Native>).entries()]
        .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); // ascending keys
      return entries.map(([k, v]) =>
        new Map<number, CborValue>([[1, toWire(schema, t.key, k)], [2, toWire(schema, t.value, v)]]));
    }
    case "msg": {
      const m = schema.message(t.name);
      const out = new Map<number, CborValue>();
      for (const f of schema.wireFields(m)) {
        const fv = value[f.name];
        out.set(f.tag, fv === null || fv === undefined ? null : toWire(schema, f.type, fv));
      }
      // forward-compat: re-emit unknown tags this schema doesn't name (cbor sorts keys)
      const residual = value.__unknown__ as Map<number, CborValue> | undefined;
      if (residual) for (const [tag, raw] of residual) out.set(tag, raw);
      return out;
    }
  }
}

function fromWire(schema: SchemaIndex, t: TypeRef, cv: CborValue): Native {
  switch (t.k) {
    case "scalar":
      return cv;
    case "enum": {
      const members = schema.enumDef(t.name).members;
      for (const [name, val] of Object.entries(members)) if (val === cv) return name;
      throw new Error(`unknown enum value ${cv} for ${t.name}`);
    }
    case "list":
      return (cv as CborValue[]).map((v) => fromWire(schema, t.elem, v));
    case "map": {
      const out = new Map<Native, Native>();
      for (const e of cv as CborValue[]) {
        const em = e as Map<number, CborValue>;
        out.set(fromWire(schema, t.key, em.get(1)!), fromWire(schema, t.value, em.get(2)!));
      }
      return out;
    }
    case "msg": {
      const m = schema.message(t.name);
      const map = cv as Map<number, CborValue>;
      const out: Native = {};
      const known = new Set<number>();
      for (const f of schema.wireFields(m)) {
        known.add(f.tag);
        const raw = map.get(f.tag);
        out[f.name] = raw === null || raw === undefined ? null : fromWire(schema, f.type, raw);
      }
      // forward-compat: capture tags this schema doesn't know (preserved raw)
      const residual = new Map<number, CborValue>();
      for (const [tag, raw] of map) if (!known.has(tag)) residual.set(tag, raw);
      if (residual.size) out.__unknown__ = residual;
      return out;
    }
  }
}

export function encode(schema: SchemaIndex, message: string, value: Native): Uint8Array {
  return cborEncode(toWire(schema, { k: "msg", name: message }, value));
}

export function decode(schema: SchemaIndex, message: string, data: Uint8Array): Native {
  return fromWire(schema, { k: "msg", name: message }, cborDecode(data));
}

// TypeRef-driven (for IR-declared method params / outputs / events).
export function encodeRef(schema: SchemaIndex, tref: TypeRef, value: Native): Uint8Array {
  return cborEncode(toWire(schema, tref, value));
}

export function decodeRef(schema: SchemaIndex, tref: TypeRef, data: Uint8Array): Native {
  return fromWire(schema, tref, cborDecode(data));
}
