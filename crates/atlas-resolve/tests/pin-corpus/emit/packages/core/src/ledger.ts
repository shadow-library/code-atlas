export class Ledger {
  entries: number[] = [];
  record(value: number): void {
    this.entries.push(value);
  }
}
