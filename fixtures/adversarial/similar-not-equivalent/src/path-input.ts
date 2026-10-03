export function normalize(value: string): string {
  return value.replace(/\\/g, '/').replace(/\/+$/, '');
}
