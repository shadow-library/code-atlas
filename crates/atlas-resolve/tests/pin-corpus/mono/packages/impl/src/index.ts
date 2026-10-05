import { Base, Store } from '@pin/contract';

export class MemStore implements Store {
  find() {
    return 1;
  }
}

export class Impl extends Base {
  step() {
    return 1;
  }
}
