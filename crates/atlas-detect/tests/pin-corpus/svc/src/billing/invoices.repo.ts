import { eq } from 'drizzle-orm';

import { db } from '../db';
import { entries, invoices } from './schema';

export async function listInvoices() {
  return db.select().from(invoices).leftJoin(entries, eq(entries.id, invoices.id));
}

export async function addEntry(id: string) {
  await db.insert(entries).values({ id });
  await db.delete(invoices).where(eq(invoices.id, id));
}
