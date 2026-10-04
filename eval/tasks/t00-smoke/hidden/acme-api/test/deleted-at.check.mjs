import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const root = new URL('../src/users/', import.meta.url);
const entity = readFileSync(new URL('user.entity.ts', root), 'utf8');
const service = readFileSync(new URL('users.service.ts', root), 'utf8');

const deletedAtColumn = /@(DeleteDateColumn|Column)\(([^)]*)\)\s*(?:(?:public|readonly)\s+)*deletedAt[!?]?\s*:\s*Date(\s*\|\s*null)?/;

function findOneBody(source) {
  const start = source.search(/\bfindOne\s*\(/);
  assert.notEqual(start, -1, 'UsersService.findOne must exist');
  const open = source.indexOf('{', start);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1;
    if (source[index] === '}') depth -= 1;
    if (depth === 0) return source.slice(open, index + 1);
  }
  assert.fail('findOne body is not closed');
}

test('User declares deletedAt as a date column', () => {
  const match = entity.match(deletedAtColumn);
  assert.ok(match, 'expected a @DeleteDateColumn or @Column decorated `deletedAt: Date` property');
  if (match[1] === 'Column') assert.match(match[2], /nullable:\s*true/, 'a plain @Column must be nullable');
});

test('deletedAt is stored in the deleted_at column', () => {
  const match = entity.match(deletedAtColumn);
  assert.ok(match, 'deletedAt column missing');
  assert.match(match[2], /name:\s*['"]deleted_at['"]/);
});

test('UsersService.findOne still returns the user entity, so deletedAt is exposed', () => {
  const body = findOneBody(service);
  assert.ok(/return\s+user\s*;/.test(body) || /deletedAt/.test(body), 'findOne must return the loaded user or map deletedAt explicitly');
});
