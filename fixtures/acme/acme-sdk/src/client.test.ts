import { afterEach, describe, expect, it, vi } from 'vitest';

import { AcmeClient } from './client';

describe('AcmeClient', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('should send the bearer token in the Authorization header', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ id: 'u1' }) });
    vi.stubGlobal('fetch', fetchMock);
    const client = new AcmeClient({ baseUrl: 'https://api.acme.test/', auth: { type: 'bearer', token: 't0k' } });
    await client.getUser('u1');
    expect(fetchMock).toHaveBeenCalledWith('https://api.acme.test/users/u1', { headers: expect.objectContaining({ Authorization: 'Bearer t0k' }) });
  });

  it('should send the api key in the x-api-key header', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ id: 'o1' }) });
    vi.stubGlobal('fetch', fetchMock);
    const client = new AcmeClient({ baseUrl: 'https://api.acme.test', auth: { type: 'apiKey', key: 'k3y' } });
    await client.getOrder('o1');
    expect(fetchMock.mock.calls[0][1].headers['x-api-key']).toBe('k3y');
  });
});
