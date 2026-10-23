export function waitBeforeRetry(error: unknown, attempt: number): number {
  const base = 250;
  const ceiling = 30_000;
  if (typeof error === 'object' && error !== null && 'retryAfter' in error) {
    const hinted = Number((error as { retryAfter: unknown }).retryAfter);
    if (Number.isFinite(hinted) && hinted > 0) {
      return Math.min(hinted * 1000, ceiling);
    }
  }
  const exponential = base * 2 ** Math.max(0, attempt - 1);
  return Math.min(exponential, ceiling);
}

export function retryDelayFromHeader(header: string | undefined): number | undefined {
  if (header === undefined || header.trim() === '') {
    return undefined;
  }
  const seconds = Number.parseInt(header, 10);
  return Number.isNaN(seconds) ? undefined : seconds * 1000;
}
