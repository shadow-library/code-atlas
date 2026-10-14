import { rank, sql, total } from "./db";

export const called = rank < total() > `c`;
export const summed = rank < total + 1 > `c`;
export const typed = sql<{ m(): void; new (): Date }>`select 1`;
