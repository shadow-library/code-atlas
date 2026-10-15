export const ordersApi = {
  get: (path: string) => window.fetch(path),
  post: (path: string, body: unknown) => window.fetch(path, { method: 'POST', body: JSON.stringify(body) }),
};
