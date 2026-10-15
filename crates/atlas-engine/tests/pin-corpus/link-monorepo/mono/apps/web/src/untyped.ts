export async function loadAnyItem(id: string) {
  return fetch(`/api/items/${id}`);
}
