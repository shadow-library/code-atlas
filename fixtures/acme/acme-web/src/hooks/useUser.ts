import { type User } from '@acme/sdk';
import { useEffect, useState } from 'react';

import { acme } from '../api/client';

export function useUser(id: string): { user: User | null; error: Error | null } {
  const [user, setUser] = useState<User | null>(null);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    acme.getUser(id).then(setUser, setError);
  }, [id]);

  return { user, error };
}
