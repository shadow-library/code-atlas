import { DECLARED_OP_SPECS } from '../src/ops/op-specs';

it('should declare the merge op', () => {
  expect(DECLARED_OP_SPECS['page.merge_drafts'].required).toEqual({ source_slug: 'string', target_slug: 'string' });
});
