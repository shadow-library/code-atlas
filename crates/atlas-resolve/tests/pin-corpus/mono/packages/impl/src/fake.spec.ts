import { Base, Store } from '@pin/contract';

export class SpecStore implements Store {
  find() {
    return 2;
  }
}

export class SpecImpl extends Base {
  step() {
    return 2;
  }
}
