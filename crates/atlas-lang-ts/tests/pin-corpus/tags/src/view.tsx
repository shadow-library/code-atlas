import { sql } from "./db";

export function OrderView({ id }: { id: string }) {
  const rows = sql<string[]>`select id from orders where id = ${id}`;
  return <ul>{rows.map((row) => <li key={row}>{row}</li>)}</ul>;
}
