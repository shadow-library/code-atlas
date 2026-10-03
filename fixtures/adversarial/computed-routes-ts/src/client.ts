import axios from 'axios';

export function getItemConcat(id: string) {
  return axios.get('/api/items/' + id);
}

export function getItemTemplate(id: string) {
  return axios.get(`/api/items/${id}`);
}

export function createItem(body: unknown) {
  return axios.post('/api/items', body);
}

export function getStatus() {
  return axios.get('/api/status');
}

export function getDynamic(url: string) {
  return axios.get(url);
}
