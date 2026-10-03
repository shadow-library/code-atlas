import { type User } from '@acme/sdk';
import { useEffect, useState } from 'react';

import { getToken } from '../api/session';
import { API_URL } from '../config';

export function useMe(): User | null {
  const [me, setMe] = useState<User | null>(null);

  useEffect(() => {
    const token = getToken();
    fetch(`${API_URL}/users/me`, { headers: { Authorization: `Bearer ${token}` } })
      .then((response) => response.json())
      .then(setMe);
  }, []);

  return me;
}
