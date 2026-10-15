import { Api, Authenticated, Permit, Read } from '@acme/http';

import { PROJECTS_READ } from './permissions';

@Api('projects')
@Authenticated()
export class ProjectsController {
  @Read(':id')
  @Permit(PROJECTS_READ)
  find() {
    return null;
  }
}
