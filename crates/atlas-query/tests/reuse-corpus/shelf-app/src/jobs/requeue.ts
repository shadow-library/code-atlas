export function requeueDelay(reason: unknown, tries: number): number {
  const first = 250;
  const most = 30_000;
  if (typeof reason === 'object' && reason !== null && 'retryAfter' in reason) {
    const asked = Number((reason as { retryAfter: unknown }).retryAfter);
    if (Number.isFinite(asked) && asked > 0) {
      return Math.min(asked * 1000, most);
    }
  }
  const next = first * 2 ** Math.max(0, tries - 1);
  return Math.min(next, most);
}
