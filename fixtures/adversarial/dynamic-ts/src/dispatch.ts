import * as handlers from './handlers';

export function invoke(methodName: string): string {
  const table = handlers as unknown as Record<string, () => string>;
  return table[methodName]();
}

export function invokeOn(target: Record<string, (...args: unknown[]) => unknown>, methodName: string): unknown {
  return target[methodName]();
}

export function loadHandler(modulePath: string): unknown {
  return require(modulePath);
}

export async function loadPlugin(name: string): Promise<{ default: () => string }> {
  return import(`./plugins/${name}`);
}

export function runExpression(source: string): unknown {
  return eval(source);
}

export function staticCall(): string {
  return handlers.start();
}
