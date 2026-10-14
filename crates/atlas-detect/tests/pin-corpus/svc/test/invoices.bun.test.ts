import { describe, expect, it, mock, spyOn } from 'bun:test';

import { Mailer } from '../src/mailer';
import { listInvoices } from '../src/billing/invoices.repo';

mock.module('../src/db', () => ({ db: { select: mock(() => []) } }));

describe('invoices', () => {
  it('lists through the replaced module', async () => {
    const mailer = new Mailer();
    spyOn(mailer, 'send').mockReturnValue(true);
    expect(await listInvoices()).toEqual([]);
  });
});
