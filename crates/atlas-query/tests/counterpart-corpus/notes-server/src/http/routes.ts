import { applyOp } from '../ops/apply-op';

export const HANDLERS: Record<string, (body: { type: string }) => Promise<number>> = {
  'pages.search': async () => 200,
  'pages.listing': async () => 200,
  'pages.export_all': async () => 202,
  'ops.apply': applyOp,
};
