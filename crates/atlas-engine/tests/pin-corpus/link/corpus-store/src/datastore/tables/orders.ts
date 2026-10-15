import { pgSchema, text } from 'drizzle-orm/pg-core';

const sales = pgSchema('sales');
export const orders = sales.table('orders', { id: text('id').primaryKey(), userId: text('user_id') });
