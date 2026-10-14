import { sql, type Pool } from "./db";

interface Row {
  id: string;
  total: number;
}

export function totals(pool: Pool, id: string) {
  const rows = sql<Row[]>`select id, total from orders where id = ${id}`;
  const one = pool.query()<Row>`select 1`;
  const nested = sql<Map<string, Array<{ id: string; at: () => Date }>>>`select now()`;
  const spread = sql<{
    id: string;
  }>`select id
     from orders`;
  return [rows, one, nested, spread];
}

export const db = () => sql;

export function plain(id: string) {
  return db()`select ${id}`;
}

export function compare(a: number, b: number, f: <T>(x: T) => T) {
  const ordered = a < b > `c`;
  return f<number>(a) + Number(ordered);
}

export class Repo {
  constructor(private readonly pool: Pool) {}

  find(id: string) {
    return this.pool.tx()<Row>`select * from orders where id = ${id}`;
  }
}
