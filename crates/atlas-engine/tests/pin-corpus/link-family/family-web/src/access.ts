export async function openAccess(token: string) {
  return fetch(`${process.env.FAMILY_API_URL}/api/access/open`, { headers: { Authorization: `Bearer ${token}` } });
}

export async function listScoped(token: string) {
  return fetch(`${process.env.FAMILY_API_URL}/api/scoped`, { headers: { Authorization: `Bearer ${token}` } });
}

export async function loadMaybe(token: string) {
  return fetch(`${process.env.FAMILY_API_URL}/api/access/maybe`, { headers: { Authorization: `Bearer ${token}` } });
}
