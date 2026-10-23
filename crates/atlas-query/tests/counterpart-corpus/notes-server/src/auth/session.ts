export function sessionUser(header: string | undefined): string | null {
  if (!header || !header.startsWith('Bearer ')) {
    return null;
  }
  return header.slice(7);
}
