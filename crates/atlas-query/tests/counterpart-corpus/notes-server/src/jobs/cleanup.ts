import { pageCount } from '../db/pages-table';

export function cleanupDue(limit: number): boolean {
  return pageCount() > limit;
}
