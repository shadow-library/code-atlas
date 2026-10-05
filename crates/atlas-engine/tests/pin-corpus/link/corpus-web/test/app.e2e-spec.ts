import { API_URL } from '../src/config';

export function loadMeInE2e(token: string) {
  return fetch(`${API_URL}/users/me`, { headers: { Authorization: `Bearer ${token}` } });
}
