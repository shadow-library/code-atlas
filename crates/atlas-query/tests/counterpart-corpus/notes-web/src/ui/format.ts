export function formatValue(value: unknown): string {
  if (typeof value === 'string') {
    return value.trim();
  }
  if (typeof value === 'number') {
    return value.toFixed(0);
  }
  return '';
}

export const EMPTY = { title: '', heading: '', drafts: [] };
