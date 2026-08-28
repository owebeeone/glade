/** Exact op/fold shapes implemented by the TypeScript Glade session. */

export type FoldShape = "value" | "log";
export type OpShape = FoldShape | "swmr";

export class UnsupportedShapeError extends Error {
  readonly code = "GLADE_UNSUPPORTED_SHAPE";
  readonly shape: string;
  readonly operation: string;
  readonly supported: readonly string[];

  constructor(shape: string, operation: string, supported: readonly string[] = ["value", "log"]) {
    super(`Glade client does not support shape ${JSON.stringify(shape)} for ${operation}; supported: ${supported.join(", ")}`);
    this.name = "UnsupportedShapeError";
    this.shape = shape;
    this.operation = operation;
    this.supported = supported;
  }
}

export function requireFoldShape(shape: string, operation: string): FoldShape {
  if (shape === "value" || shape === "log") return shape;
  throw new UnsupportedShapeError(shape, operation);
}

export function requireOpShape(shape: string, operation: string): OpShape {
  if (shape === "value" || shape === "log" || shape === "swmr") return shape;
  throw new UnsupportedShapeError(shape, operation, ["value", "log", "swmr"]);
}
