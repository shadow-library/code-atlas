import { describe, expect, it } from 'vitest';

import { toCents } from './money';

describe('toCents', () => {
  it('should convert whole and fractional amounts', () => {
    expect(toCents('12.34')).toBe(1234);
    expect(toCents('12.3')).toBe(1230);
    expect(toCents('12')).toBe(1200);
  });

  it('should keep the sign of negative amounts', () => {
    expect(toCents('-1.50')).toBe(-150);
  });
});
