export class Other { ping() {} }
export class Outer {
  helper() {}
  run() {
    this.helper();
    const arrow = () => this.helper();
    class Inner { helper() {} go() { this.helper(); this.ping(); } }
    const callback = function () { this.helper(); };
    return [arrow, Inner, callback];
  }
}
export function free() { this.helper(); }
export const store = { a() { this.ping(); }, b() {} };
