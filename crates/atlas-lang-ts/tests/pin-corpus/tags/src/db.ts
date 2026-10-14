export interface Pool {
  query(): typeof sql;
  tx(): typeof sql;
}

export function sql<T>(strings: TemplateStringsArray, ...values: unknown[]): T {
  return [strings, values] as unknown as T;
}
