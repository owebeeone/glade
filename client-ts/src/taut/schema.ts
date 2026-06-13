// The neutral IR, as consumed by TypeScript. Parsed from taut/corpus/
// griplab.ir.json — the same artifact Python authored. No schema is re-declared
// here; this is the cross-language contract.

export type TypeRef =
  | { k: "scalar"; scalar: "int" | "str" | "bytes" | "bool" }
  | { k: "enum"; name: string }
  | { k: "msg"; name: string }
  | { k: "list"; elem: TypeRef }
  | { k: "map"; key: TypeRef; value: TypeRef };

export interface FieldDef {
  name: string;
  tag: number;
  type: TypeRef;
  optional: boolean;
  transient: boolean;
}

export interface MessageDef {
  name: string;
  fields: FieldDef[];
}

export interface EnumDef {
  name: string;
  members: Record<string, number>;
}

export interface ParamDef {
  name: string;
  type: TypeRef;
}

export interface OutSlot {
  slot: string;
  type: TypeRef;
}

// The minimal contract (D22): (name, in, out, shape). `shape` is the sole
// discriminator; `out` binds a type to each of the shape's delivery slots.
// `kind`/`output`/`events` are derived (see methodOutput / methodEvents).
export interface MethodDef {
  name: string;
  role: string;
  shape: string;
  params: ParamDef[];
  out: OutSlot[];
}

// Derived view: the single return type of a once-delivered (unary) method.
export function methodOutput(m: MethodDef): TypeRef | null {
  return m.out.length ? m.out[0].type : null;
}

// Derived view: the slot->type bindings a streamed method delivers.
export function methodEvents(m: MethodDef): [string, TypeRef][] {
  return m.out.map((o) => [o.slot, o.type]);
}

export interface ServiceDef {
  name: string;
  methods: MethodDef[];
}

export interface Schema {
  version: number;
  enums: EnumDef[];
  messages: MessageDef[];
  services: ServiceDef[];
}

export class SchemaIndex {
  private msgs = new Map<string, MessageDef>();
  private enms = new Map<string, EnumDef>();
  private mths = new Map<string, MethodDef>();

  constructor(schema: Schema) {
    for (const m of schema.messages) this.msgs.set(m.name, m);
    for (const e of schema.enums) this.enms.set(e.name, e);
    for (const s of schema.services ?? []) for (const m of s.methods) this.mths.set(m.name, m);
  }

  method(name: string): MethodDef {
    const m = this.mths.get(name);
    if (!m) throw new Error(`unknown method ${name}`);
    return m;
  }

  message(name: string): MessageDef {
    const m = this.msgs.get(name);
    if (!m) throw new Error(`unknown message ${name}`);
    return m;
  }

  enumDef(name: string): EnumDef {
    const e = this.enms.get(name);
    if (!e) throw new Error(`unknown enum ${name}`);
    return e;
  }

  wireFields(m: MessageDef): FieldDef[] {
    return m.fields.filter((f) => !f.transient);
  }
}

export function loadSchema(json: unknown): SchemaIndex {
  return new SchemaIndex(json as Schema);
}
