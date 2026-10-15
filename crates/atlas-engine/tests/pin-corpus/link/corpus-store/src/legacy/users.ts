import { pgTable, text } from 'drizzle-orm/pg-core';

export const legacyUsers = pgTable('users', { id: text('id').primaryKey() });
