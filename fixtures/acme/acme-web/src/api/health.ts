import { API_URL } from '../config';

export async function checkHealth(): Promise<boolean> {
  const response = await fetch(`${API_URL}/health`);
  return response.ok;
}
