export function before(): number {
  return 1;
}

export class Early {
  run(): number {
    return before();
  }
}

export function middle(): number {
  const value = (1 + ;
  return value;
}

export function after(): number {
  return before() + 2;
}

export class Late {
  go(): number {
    return after();
  }
}
