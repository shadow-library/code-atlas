import { total } from '../index';

export function checkout(prices: number[]): string {
  return `Total: ${total(prices)}`;
}
