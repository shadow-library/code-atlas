export interface Revision {
  page: string;
  number: number;
}

export function nextRevision(previous: Revision): Revision {
  return { page: previous.page, number: previous.number + 1 };
}
