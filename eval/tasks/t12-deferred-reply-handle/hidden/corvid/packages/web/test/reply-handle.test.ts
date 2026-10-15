import { describe, expect, test } from 'bun:test';

import { Kernel } from '@corvid/core';

import { Fetch, Payload, Reply, type ReplyHandle, Resource, Store, WebRouter } from '../src';

const ATOM = '<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"/>';

@Resource()
class ProbeResource {
  @Fetch('/probe/later')
  async later(@Reply() reply: ReplyHandle) {
    await Promise.resolve();
    reply.send(ATOM, 'application/atom+xml');
  }

  @Fetch('/probe/timer')
  async timer(@Reply() reply: ReplyHandle) {
    await new Promise(resolve => setTimeout(resolve, 5));
    reply.status(202).send('accepted');
  }

  @Fetch('/probe/now')
  now(@Reply() reply: ReplyHandle) {
    reply.send('now');
  }

  @Fetch('/probe/value')
  async value() {
    await Promise.resolve();
    return { ok: true };
  }

  @Store('/probe/echo')
  echo(@Payload() payload: { word: string }, @Reply() reply: ReplyHandle) {
    reply.send(payload.word.toUpperCase());
  }

  @Fetch('/probe/silent')
  async silent(@Reply() _reply: ReplyHandle) {
    await Promise.resolve();
  }
}

const router = new WebRouter();
Kernel.boot({ transport: router, units: [ProbeResource] });

describe('reply handle', () => {
  test('should send what an async handler wrote after an await', async () => {
    const reply = await router.dispatch({ verb: 'GET', path: '/probe/later', headers: {} });
    expect(reply).toEqual({ status: 200, headers: { 'content-type': 'application/atom+xml' }, content: ATOM });
  });

  test('should keep the status an async handler set before sending', async () => {
    const reply = await router.dispatch({ verb: 'GET', path: '/probe/timer', headers: {} });
    expect(reply).toEqual({ status: 202, headers: { 'content-type': 'text/plain; charset=utf-8' }, content: 'accepted' });
  });

  test('should send what a sync handler wrote', async () => {
    const reply = await router.dispatch({ verb: 'GET', path: '/probe/now', headers: {} });
    expect(reply).toEqual({ status: 200, headers: { 'content-type': 'text/plain; charset=utf-8' }, content: 'now' });
  });

  test('should serialise the value of a handler without a reply handle', async () => {
    const reply = await router.dispatch({ verb: 'GET', path: '/probe/value', headers: {} });
    expect(reply).toEqual({ status: 200, headers: { 'content-type': 'application/json' }, content: '{"ok":true}' });
  });

  test('should pass the reply handle at its own parameter position', async () => {
    const reply = await router.dispatch({ verb: 'POST', path: '/probe/echo', headers: {}, payload: { word: 'loud' } });
    expect(reply.content).toBe('LOUD');
  });

  test('should answer 204 when a handler neither returns nor sends', async () => {
    const reply = await router.dispatch({ verb: 'GET', path: '/probe/silent', headers: {} });
    expect(reply).toEqual({ status: 204, headers: {}, content: '' });
  });
});
