export interface OpSpec {
  required: Record<string, string>;
  optional: Record<string, string>;
}

export const DECLARED_OP_SPECS: Record<string, OpSpec> = {
  'page.create_draft': { required: { title: 'string' }, optional: { draft_slug: 'string' } },
  'page.rename_title': { required: { draft_slug: 'string', title: 'string' }, optional: {} },
  'page.move_section': { required: { section_anchor: 'string' }, optional: {} },
  'page.split_section': { required: { section_anchor: 'string' }, optional: {} },
  'page.join_sections': { required: { section_anchor: 'string' }, optional: {} },
  'page.archive_draft': { required: { draft_slug: 'string' }, optional: {} },
  'page.restore_draft': { required: { draft_slug: 'string' }, optional: {} },
  'page.merge_drafts': { required: { source_slug: 'string', target_slug: 'string' }, optional: {} },
  'page.pin_note': { required: { draft_slug: 'string' }, optional: {} },
  'page.unpin_note': { required: { draft_slug: 'string' }, optional: {} },
  'page.tag_note': { required: { draft_slug: 'string', tag: 'string' }, optional: {} },
  'page.untag_note': { required: { draft_slug: 'string', tag: 'string' }, optional: {} },
  'page.stamp_revision': { required: { revision_mark: 'number' }, optional: {} },
};

export const WIRE_FIELDS = ['draft_slug', 'section_anchor', 'revision_mark', 'merge_receipt'];

export function specOf(type: string): OpSpec | undefined {
  return DECLARED_OP_SPECS[type];
}
