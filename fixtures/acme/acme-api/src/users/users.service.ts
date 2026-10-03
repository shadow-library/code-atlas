import { Injectable, NotFoundException } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';

import { User } from './user.entity';

export interface CreateUserInput {
  email: string;
  displayName: string;
}

export type UpdateUserInput = Partial<CreateUserInput>;

@Injectable()
export class UsersService {
  constructor(@InjectRepository(User) private readonly users: Repository<User>) {}

  async findOne(id: string): Promise<User> {
    const user = await this.users.findOneBy({ id });
    if (!user) throw new NotFoundException(`User ${id} not found`);
    return user;
  }

  create(input: CreateUserInput): Promise<User> {
    return this.users.save(this.users.create({ ...input, permissions: ['users:read'] }));
  }

  async update(id: string, input: UpdateUserInput): Promise<User> {
    const user = await this.findOne(id);
    return this.users.save({ ...user, ...input });
  }

  async remove(id: string): Promise<void> {
    const result = await this.users.delete({ id });
    if (!result.affected) throw new NotFoundException(`User ${id} not found`);
  }
}
