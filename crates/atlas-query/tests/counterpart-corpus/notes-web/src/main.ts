import { renderPage } from './ui/page-view';
import { initialPage } from './state/store';

export function start(): string {
  return renderPage(initialPage());
}
