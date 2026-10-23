import type { OpSpec } from './op-specs';

export function missingFields(spec: OpSpec, body: Record<string, unknown>): string[] {
  return Object.keys(spec.required).filter((field) => body[field] === undefined);
}
