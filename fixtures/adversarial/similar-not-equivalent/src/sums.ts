export function sumLoop(values: number[]): number {
  let total = 0;
  for (const value of values) total += value;
  return total;
}

export function sumReduce(values: number[]): number {
  return values.reduce((acc, value) => acc + value, 0);
}

export function isEvenMod(n: number): boolean {
  return n % 2 === 0;
}

export function isEvenBit(n: number): boolean {
  return (n & 1) === 0;
}
