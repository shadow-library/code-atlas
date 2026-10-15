export const booksApi = {
  get: (path: string, init?: object) => fetch(path, init),
  post: (path: string, body: unknown) => fetch(path, { method: 'POST', body: JSON.stringify(body) }),
};

export function formatTitle(title: string): string {
  return title.trim();
}
