import { AcmeClient } from '@acme/sdk';

import { API_URL } from '../config';
import { getToken } from './session';

export const acme = new AcmeClient({ baseUrl: API_URL, auth: { type: 'bearer', token: getToken() } });
