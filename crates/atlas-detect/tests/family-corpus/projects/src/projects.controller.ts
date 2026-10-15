import { Api, Auth, Authenticated, BotPermission, Guarded, Permit, Read, Remove, Write } from '@acme/server';
import { PROJECTS_WRITE, Scope } from './permissions';
import { SessionGuard } from './session.guard';

const PROJECTS_READ = 'projects:read';
const ADMIN_ROLE = 'admin';
const BOT = 'bot';

function policy(): string {
  return 'dynamic';
}

@Api('projects')
@Authenticated()
export class ProjectsController {
  @Read()
  @Permit(PROJECTS_READ)
  list() {
    return [];
  }

  @Write()
  @Auth({ roles: ['owner', ADMIN_ROLE], optional: true, scope: Scope.Admin, check: policy() })
  @Permit([PROJECTS_WRITE, 'projects:audit'])
  create() {
    return null;
  }

  @Remove(':id')
  @Guarded(PROJECTS_READ)
  @Guarded(SessionGuard, { optional: true })
  @BotPermission(Scope.Admin)
  remove() {
    return null;
  }

  @Read('bot')
  @Auth(ADMIN_ROLE, Scope.Admin, 3)
  @BotPermission(`${BOT}:read`)
  @Permit(policy())
  bot() {
    return null;
  }
}
