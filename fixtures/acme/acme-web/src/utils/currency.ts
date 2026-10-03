export function parseMoney(input: string): number {
  const value = input.trim();
  const sign = value[0] === '-' ? -1 : 1;
  const digits = value.replace(/^-/, '');
  const dot = digits.indexOf('.');
  const whole = dot === -1 ? digits : digits.slice(0, dot);
  const fraction = dot === -1 ? '' : digits.slice(dot + 1);
  return sign * (Number(whole) * 100 + Number(`${fraction}00`.slice(0, 2)));
}
