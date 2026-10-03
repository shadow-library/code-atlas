import { NotFoundException } from '@nestjs/common';

import { normalizeEmail } from './email';
import { UsersService } from './users.service';

describe('UsersService', () => {
  const repository = { findOneBy: jest.fn(), save: jest.fn(), create: jest.fn(), delete: jest.fn() };
  const service = new UsersService(repository as never);

  afterEach(() => jest.resetAllMocks());

  describe('findOne', () => {
    it('should return the user when it exists', async () => {
      repository.findOneBy.mockResolvedValue({ id: 'u1', email: 'a@acme.test' });
      await expect(service.findOne('u1')).resolves.toMatchObject({ id: 'u1' });
    });

    it('should throw NotFoundException when the user is missing', async () => {
      repository.findOneBy.mockResolvedValue(null);
      await expect(service.findOne('missing')).rejects.toBeInstanceOf(NotFoundException);
    });
  });

  describe('normalizeEmail', () => {
    it('should lowercase and trim', () => {
      expect(normalizeEmail('  Ada@Acme.TEST ')).toBe('ada@acme.test');
    });
  });
});
