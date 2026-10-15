import { eq } from 'drizzle-orm';

import { db } from '../db';
import * as billing from '.';
import { ledger } from '@svc/billing';
import { invoices } from './schema';

export async function settle(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(ledger.invoices).set({ id }).where(eq(ledger.invoices.id, id));
    await tx.insert(billing.ledger.entries).values({ id });
  });
  return db.select().from(invoices).where(eq(invoices.id, id));
}

export async function auditColumns() {
  return db.select().from(invoices.id);
}
