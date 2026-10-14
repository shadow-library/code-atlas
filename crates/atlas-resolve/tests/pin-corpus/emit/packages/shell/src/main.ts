import { Ledger, total } from '@emit/core';
import { slug } from '@emit/core/text/slug';
import { legacyFormat } from '@emit/legacy';
import { typedLabel } from '@emit/typed';

export function run(values: number[]): string {
  const ledger = new Ledger();
  values.forEach((value) => ledger.record(value));
  return typedLabel(slug(legacyFormat(total(ledger.entries))));
}
