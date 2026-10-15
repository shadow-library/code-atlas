import { createKitClient } from '@acme/http-kit';

export const adminKit = createKitClient({ surfaces: { audit: '/api/audit/', users: '/api/users/' } });

export const AdminRequest = adminKit.users;
