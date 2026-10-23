export const DRAFT_FIELDS = [
  { name: 'title', required: true },
  { name: 'summary', optional: true },
];

export function fieldNames(): string[] {
  return DRAFT_FIELDS.map((field) => field.name);
}
