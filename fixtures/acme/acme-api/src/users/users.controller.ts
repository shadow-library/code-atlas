import { Body, Controller, Delete, Get, Param, Patch, Post, UseGuards } from '@nestjs/common';

import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { PermissionsGuard } from '../auth/permissions.guard';
import { RequirePermission } from '../auth/require-permission.decorator';
import { type CreateUserInput, type UpdateUserInput, UsersService } from './users.service';

@Controller('users')
@UseGuards(JwtAuthGuard, PermissionsGuard)
export class UsersController {
  constructor(private readonly usersService: UsersService) {}

  @Get(':id')
  @RequirePermission('users:read')
  findOne(@Param('id') id: string) {
    return this.usersService.findOne(id);
  }

  @Post()
  @RequirePermission('users:write')
  create(@Body() input: CreateUserInput) {
    return this.usersService.create(input);
  }

  @Patch(':id')
  @RequirePermission('users:write')
  update(@Param('id') id: string, @Body() input: UpdateUserInput) {
    return this.usersService.update(id, input);
  }

  @Delete(':id')
  @RequirePermission('users:delete')
  remove(@Param('id') id: string) {
    return this.usersService.remove(id);
  }
}
