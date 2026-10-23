const rows: unknown[] = [];

export async function savePage(row: unknown): Promise<void> {
  rows.push(row);
}

export function pageCount(): number {
  return rows.length;
}
