import { sendOp } from './send-op';
import type { Page } from '../state/store';

export interface PageOp {
  type: string;
  draft_slug?: string;
  section_anchor?: string;
  revision_mark?: number;
}

export function applyOp(page: Page, op: PageOp): Page {
  switch (op.type) {
    case 'page.create_draft':
      return { ...page, drafts: [...page.drafts, op.draft_slug ?? ''] };
    case 'page.rename_title':
      return { ...page, heading: op.draft_slug ?? page.heading };
    case 'page.move_section':
    case 'page.split_section':
    case 'page.join_sections':
      return { ...page, anchor: op.section_anchor ?? page.anchor };
    case 'page.archive_draft':
    case 'page.restore_draft':
      return { ...page, archived: !page.archived };
    case 'page.pin_note':
    case 'page.unpin_note':
      return { ...page, pinned: !page.pinned };
    case 'page.tag_note':
    case 'page.untag_note':
      return page;
    case 'page.stamp_revision':
      return { ...page, revision: op.revision_mark ?? page.revision };
    default:
      return page;
  }
}

export function pinNote(slug: string): Promise<void> {
  return sendOp({ type: 'page.pin_note', draft_slug: slug });
}
