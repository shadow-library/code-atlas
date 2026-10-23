import { applyOp } from './op-switch';

const empty = { drafts: [], heading: '', anchor: '', archived: false, pinned: false, revision: 0 };

it('should archive a draft', () => {
  expect(applyOp(empty, { type: 'page.archive_draft' }).archived).toBe(true);
  expect(applyOp(empty, { type: 'page.merge_drafts' })).toBe(empty);
});
