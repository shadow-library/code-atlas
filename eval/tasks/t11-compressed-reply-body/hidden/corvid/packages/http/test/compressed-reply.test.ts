import { describe, expect, test } from 'bun:test';
import { brotliDecompressSync, gunzipSync } from 'node:zlib';

import { Controller, Get, HttpServer, RawResponse } from '../src';

const XML = '<?xml version="1.0"?><urlset><url><loc>https://corvid.example/</loc></url></urlset>';

@Controller('probe')
class ProbeController {
  @Get('raw')
  raw() {
    return new RawResponse('application/xml', XML);
  }

  @Get('text')
  text() {
    return 'plain text body';
  }

  @Get('json')
  json() {
    return { ok: true };
  }

  @Get('empty')
  empty() {
    return undefined;
  }
}

const server = HttpServer.create({ port: 0, controllers: [ProbeController], compression: { threshold: 0, encodings: ['br', 'gzip'] } });

function decode(body: Buffer | undefined, encoding: string | undefined): string {
  const bytes = body ?? Buffer.alloc(0);
  if (encoding === 'gzip') return gunzipSync(bytes).toString('utf8');
  if (encoding === 'br') return brotliDecompressSync(bytes).toString('utf8');
  return bytes.toString('utf8');
}

describe('compressed replies', () => {
  for (const encoding of ['gzip', 'br']) {
    test(`should keep a raw response body under ${encoding}`, async () => {
      const reply = await server.inject({ method: 'GET', url: '/probe/raw', headers: { 'accept-encoding': encoding } });
      expect(reply.headers['content-encoding']).toBe(encoding);
      expect(reply.headers['content-type']).toBe('application/xml');
      expect(decode(reply.body, encoding)).toBe(XML);
    });

    test(`should keep a string body under ${encoding}`, async () => {
      const reply = await server.inject({ method: 'GET', url: '/probe/text', headers: { 'accept-encoding': encoding } });
      expect(reply.headers['content-encoding']).toBe(encoding);
      expect(decode(reply.body, encoding)).toBe('plain text body');
    });

    test(`should keep a json body under ${encoding}`, async () => {
      const reply = await server.inject({ method: 'GET', url: '/probe/json', headers: { 'accept-encoding': encoding } });
      expect(JSON.parse(decode(reply.body, encoding))).toEqual({ ok: true });
    });
  }

  test('should leave an uncompressed raw reply exactly as the handler produced it', async () => {
    const reply = await server.inject({ method: 'GET', url: '/probe/raw', headers: {} });
    expect(reply).toEqual({ status: 200, headers: { 'content-type': 'application/xml' }, text: XML });
  });

  test('should leave an uncompressed string reply exactly as the handler produced it', async () => {
    const reply = await server.inject({ method: 'GET', url: '/probe/text', headers: {} });
    expect(reply).toEqual({ status: 200, headers: { 'content-type': 'text/plain; charset=utf-8' }, text: 'plain text body' });
  });

  test('should send no body and no encoding for an empty reply', async () => {
    const reply = await server.inject({ method: 'GET', url: '/probe/empty', headers: { 'accept-encoding': 'gzip' } });
    expect(reply.status).toBe(204);
    expect(reply.headers['content-encoding']).toBeUndefined();
  });
});
