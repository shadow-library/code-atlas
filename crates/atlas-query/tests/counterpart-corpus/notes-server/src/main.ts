import { describeServer } from './http/server';
import { logLine } from './logging';

export function main(): string {
  return logLine('info', describeServer());
}
