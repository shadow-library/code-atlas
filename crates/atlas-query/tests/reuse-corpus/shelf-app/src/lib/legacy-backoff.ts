export function legacyWaitBeforeRetry(failure: unknown, round: number): number {
  const start = 250;
  const limit = 30_000;
  if (typeof failure === 'object' && failure !== null && 'retryAfter' in failure) {
    const suggested = Number((failure as { retryAfter: unknown }).retryAfter);
    if (Number.isFinite(suggested) && suggested > 0) {
      return Math.min(suggested * 1000, limit);
    }
  }
  const grown = start * 2 ** Math.max(0, round - 1);
  return Math.min(grown, limit);
}
