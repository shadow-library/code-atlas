import { pinNote } from '../ops/op-switch';
import { labelOf } from '../ops/op-labels';

export function toolbarButtons(slug: string): { label: string; run: () => Promise<void> }[] {
  return [{ label: labelOf('page.pin_note'), run: () => pinNote(slug) }];
}
