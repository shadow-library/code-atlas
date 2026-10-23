import type { Page } from './store';

export function visibleDrafts(page: Page): string[] {
  return page.archived ? [] : page.drafts;
}

export function isPinned(page: Page): boolean {
  return page.pinned;
}
