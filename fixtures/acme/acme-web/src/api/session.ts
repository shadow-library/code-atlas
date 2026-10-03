const TOKEN_KEY = 'acme.token';

export function getToken(): string {
  return window.localStorage.getItem(TOKEN_KEY) ?? '';
}

export function setToken(token: string): void {
  window.localStorage.setItem(TOKEN_KEY, token);
}
