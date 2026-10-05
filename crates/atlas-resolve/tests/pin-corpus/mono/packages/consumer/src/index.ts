import { Base, Store } from '@pin/contract';

export function go(store: Store, base: Base) {
  return store.find() + base.step();
}
