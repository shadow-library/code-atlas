import { type CreateOrderInput, type Order } from '@acme/sdk';
import { useCallback, useState } from 'react';

import { acme } from '../api/client';

export function useCreateOrder() {
  const [pending, setPending] = useState(false);

  const submit = useCallback(async (input: CreateOrderInput): Promise<Order> => {
    setPending(true);
    try {
      return await acme.createOrder(input);
    } finally {
      setPending(false);
    }
  }, []);

  return { submit, pending };
}
