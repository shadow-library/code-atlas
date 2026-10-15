import { eq } from 'drizzle-orm';

import { db } from './client';
import * as datastore from './datastore';
import { schema } from '@store/datastore';
import { legacyUsers } from './legacy/users';

export async function listOrders(userId: string) {
  return db.select().from(schema.orders).innerJoin(schema.users, eq(schema.users.id, schema.orders.userId)).where(eq(schema.orders.userId, userId));
}

export async function placeOrder(id: string, userId: string) {
  await db.transaction(async (tx) => {
    await tx.insert(datastore.schema.orders).values({ id, userId });
  });
}

export async function mergeUsers() {
  const rows = await db.select().from(legacyUsers);
  await db.delete(schema.users);
  return rows;
}

export async function auditUsers() {
  const current = await db.select().from(schema.users);
  const legacy = await db.select().from(legacyUsers);
  return [...current, ...legacy];
}
