// Byte helpers shared across the glade TS client.

export function hex(b: Uint8Array): string {
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

export function unhex(s: string): Uint8Array {
  const m = s.match(/../g) ?? [];
  return Uint8Array.from(m.map((b) => parseInt(b, 16)));
}

export function bytesEq(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export function utf8(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}
