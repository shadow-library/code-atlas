export function toCents(amount: string): number {
  const trimmed = amount.trim();
  const negative = trimmed.startsWith('-');
  const [whole, fraction = ''] = (negative ? trimmed.slice(1) : trimmed).split('.');
  const cents = Number(whole) * 100 + Number(fraction.padEnd(2, '0').slice(0, 2));
  return negative ? -cents : cents;
}
