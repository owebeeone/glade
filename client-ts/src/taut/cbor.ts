// Minimal deterministic CBOR — the TypeScript binding of the frozen wire
// substrate. Byte-for-byte identical to taut/src/taut/wire/cbor.py: same tiny
// subset (int, bytes, text, array, int-keyed map, bool, null), same core
// deterministic encoding (definite length, shortest-form ints, ascending map
// keys). Maps use integer keys only — they carry field tags.

export type CborValue =
  | number
  | string
  | boolean
  | null
  | Uint8Array
  | CborValue[]
  | Map<number, CborValue>;

function pushHead(out: number[], major: number, n: number): void {
  const mt = major << 5;
  if (n < 24) out.push(mt | n);
  else if (n < 0x100) out.push(mt | 24, n);
  else if (n < 0x10000) out.push(mt | 25, (n >> 8) & 0xff, n & 0xff);
  else if (n < 0x100000000)
    out.push(mt | 26, (n >>> 24) & 0xff, (n >>> 16) & 0xff, (n >>> 8) & 0xff, n & 0xff);
  else {
    out.push(mt | 27);
    const bn = BigInt(n);
    for (let i = 7; i >= 0; i--) out.push(Number((bn >> BigInt(i * 8)) & 0xffn));
  }
}

function enc(value: CborValue, out: number[]): void {
  if (value === null) { out.push(0xf6); return; }
  if (value === true) { out.push(0xf5); return; }
  if (value === false) { out.push(0xf4); return; }
  if (typeof value === "number") {
    if (!Number.isInteger(value)) throw new Error("frozen CBOR subset: no floats");
    if (value >= 0) pushHead(out, 0, value);
    else pushHead(out, 1, -1 - value);
    return;
  }
  if (value instanceof Uint8Array) {
    pushHead(out, 2, value.length);
    for (const b of value) out.push(b);
    return;
  }
  if (typeof value === "string") {
    const e = new TextEncoder().encode(value);
    pushHead(out, 3, e.length);
    for (const b of e) out.push(b);
    return;
  }
  if (Array.isArray(value)) {
    pushHead(out, 4, value.length);
    for (const v of value) enc(v, out);
    return;
  }
  if (value instanceof Map) {
    const keys = [...value.keys()].sort((a, b) => a - b); // deterministic
    pushHead(out, 5, keys.length);
    for (const k of keys) {
      pushHead(out, 0, k);
      enc(value.get(k) as CborValue, out);
    }
    return;
  }
  throw new Error("type not in the frozen CBOR subset");
}

export function encode(value: CborValue): Uint8Array {
  const out: number[] = [];
  enc(value, out);
  return Uint8Array.from(out);
}

function readArg(data: Uint8Array, off: number, info: number): [number, number] {
  if (info < 24) return [info, off];
  if (info === 24) return [data[off], off + 1];
  if (info === 25) return [(data[off] << 8) | data[off + 1], off + 2];
  if (info === 26)
    return [
      data[off] * 0x1000000 + data[off + 1] * 0x10000 + data[off + 2] * 0x100 + data[off + 3],
      off + 4,
    ];
  if (info === 27) {
    let bn = 0n;
    for (let i = 0; i < 8; i++) bn = (bn << 8n) | BigInt(data[off + i]);
    return [Number(bn), off + 8];
  }
  throw new Error(`unsupported additional-info ${info}`);
}

function dec(data: Uint8Array, off: number): [CborValue, number] {
  const initial = data[off];
  const major = initial >> 5;
  const info = initial & 0x1f;
  off++;
  if (major === 0) return readArg(data, off, info);
  if (major === 1) {
    const [n, o] = readArg(data, off, info);
    return [-1 - n, o];
  }
  if (major === 2) {
    const [n, o] = readArg(data, off, info);
    return [data.slice(o, o + n), o + n];
  }
  if (major === 3) {
    const [n, o] = readArg(data, off, info);
    return [new TextDecoder().decode(data.slice(o, o + n)), o + n];
  }
  if (major === 4) {
    let [n, o] = readArg(data, off, info);
    const arr: CborValue[] = [];
    for (let i = 0; i < n; i++) {
      const [v, o2] = dec(data, o);
      arr.push(v);
      o = o2;
    }
    return [arr, o];
  }
  if (major === 5) {
    let [n, o] = readArg(data, off, info);
    const m = new Map<number, CborValue>();
    for (let i = 0; i < n; i++) {
      const [k, o2] = dec(data, o);
      const [v, o3] = dec(data, o2);
      m.set(k as number, v);
      o = o3;
    }
    return [m, o];
  }
  if (major === 7) {
    if (info === 20) return [false, off];
    if (info === 21) return [true, off];
    if (info === 22) return [null, off];
    throw new Error(`unsupported simple value ${info}`);
  }
  throw new Error(`unsupported major type ${major}`);
}

export function decode(data: Uint8Array): CborValue {
  const [value, off] = dec(data, 0);
  if (off !== data.length) throw new Error("trailing bytes after top-level CBOR item");
  return value;
}
