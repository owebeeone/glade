/** Exact op/fold shapes implemented by the TypeScript Glade session. */

export type FoldShape = "value" | "log";

export class UnsupportedShapeError extends Error {
  readonly code = "GLADE_UNSUPPORTED_SHAPE";
  readonly shape: string;
  readonly operation: string;

  constructor(shape: string, operation: string) {
    super(`Glade client does not support shape ${JSON.stringify(shape)} for ${operation}; supported: value, log`);
    this.name = "UnsupportedShapeError";
    this.shape = shape;
    this.operation = operation;
  }
}

export function requireFoldShape(shape: string, operation: string): FoldShape {
  if (shape === "value" || shape === "log") return shape;
  throw new UnsupportedShapeError(shape, operation);
}
