import type { Page } from '../state/store';
import { visibleDrafts } from '../state/selectors';
import { formatValue } from './format';

export function renderPage(page: Page): string {
  return [formatValue(page.heading), ...visibleDrafts(page)].join('\n');
}
