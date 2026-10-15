export async function loadProject(id: string, token: string) {
  return fetch(`${process.env.FAMILY_API_URL}/api/projects/${id}`, { headers: { Authorization: `Bearer ${token}` } });
}
