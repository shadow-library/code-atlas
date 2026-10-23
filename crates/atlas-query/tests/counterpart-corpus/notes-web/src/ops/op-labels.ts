export const OP_LABELS: Record<string, string> = {
  'page.create_draft': 'Create a draft',
  'page.rename_title': 'Rename the title',
  'page.move_section': 'Move a section',
  'page.split_section': 'Split a section',
  'page.join_sections': 'Join sections',
  'page.archive_draft': 'Archive the draft',
  'page.restore_draft': 'Restore the draft',
  'page.pin_note': 'Pin the note',
  'page.unpin_note': 'Unpin the note',
  'page.tag_note': 'Tag the note',
  'page.untag_note': 'Untag the note',
  'page.stamp_revision': 'Stamp a revision',
};

export function labelOf(type: string): string {
  return OP_LABELS[type] ?? type;
}
