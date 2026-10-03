import { format } from './format';

export function renderImported(items: string[]): string[] {
  return items.map((item) => format(item));
}

export function renderLocal(items: string[]): string[] {
  const format = (value: string) => value.toUpperCase();
  return items.map((item) => format(item));
}

export function renderParam(format: (value: string) => string, item: string): string {
  return format(item);
}

export function renderBlockScoped(items: string[]): string[] {
  const out: string[] = [];
  for (const item of items) {
    out.push(format(item));
  }
  {
    const format = (value: string) => value.trim();
    out.push(format(' block '));
  }
  return out;
}
