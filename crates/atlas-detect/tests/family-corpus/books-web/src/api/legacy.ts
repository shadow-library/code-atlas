export default {
  delete: (path: string) => fetch(path, { method: 'DELETE' }),
};
