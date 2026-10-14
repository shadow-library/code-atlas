import { pgSchema, pgTable, text } from 'drizzle-orm/pg-core';

export const invoices = pgTable('invoices', { id: text('id').primaryKey() });

const ledger = pgSchema('ledger');
export const entries = ledger.table('entries', { id: text('id').primaryKey() });
