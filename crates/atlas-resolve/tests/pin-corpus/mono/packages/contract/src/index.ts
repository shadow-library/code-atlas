export interface Store {
  find(): number;
}

export abstract class Base {
  abstract step(): number;

  run() {
    return this.step();
  }
}
