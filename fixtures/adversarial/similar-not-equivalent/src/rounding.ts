export function roundHalfUp(value: number): number {
  const floor = Math.floor(value);
  return value - floor < 0.5 ? floor : floor + 1;
}

export function roundHalfEven(value: number): number {
  const floor = Math.floor(value);
  const diff = value - floor;
  if (diff < 0.5) return floor;
  if (diff > 0.5) return floor + 1;
  return floor % 2 === 0 ? floor : floor + 1;
}
