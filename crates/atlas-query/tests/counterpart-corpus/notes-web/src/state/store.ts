export interface Page {
  drafts: string[];
  heading: string;
  anchor: string;
  archived: boolean;
  pinned: boolean;
  revision: number;
}

export function initialPage(): Page {
  return { drafts: [], heading: 'Untitled', anchor: '', archived: false, pinned: false, revision: 0 };
}
