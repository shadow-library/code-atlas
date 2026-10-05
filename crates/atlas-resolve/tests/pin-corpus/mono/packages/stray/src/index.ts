import { Base, Store } from '@pin/contract';

export class StrayStore implements Store {
  find() {
    return 3;
  }
}

export class Stray extends Base {
  step() {
    return 3;
  }
}
