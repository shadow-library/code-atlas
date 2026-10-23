import { specOf } from './op-specs';
import { savePage } from '../db/pages-table';

export async function applyOp(body: { type: string }): Promise<number> {
  const spec = specOf(body.type);
  if (!spec) {
    return 400;
  }
  await savePage(body);
  return 204;
}
