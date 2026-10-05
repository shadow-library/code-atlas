import { API_URL } from './config';

export function loadMe(token: string) {
  return fetch(`${API_URL}/users/me`, { headers: { Authorization: `Bearer ${token}` } });
}
