import { toCents } from '@acme/sdk';
import { type FormEvent, useState } from 'react';

import { useCreateOrder } from '../hooks/useCreateOrder';
import { useMe } from '../hooks/useMe';

export function OrderForm() {
  const me = useMe();
  const { submit, pending } = useCreateOrder();
  const [amount, setAmount] = useState('0.00');

  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    if (!me) return;
    submit({ userId: me.id, subtotalCents: toCents(amount) });
  };

  return (
    <form onSubmit={onSubmit}>
      <input value={amount} onChange={(event) => setAmount(event.target.value)} />
      <button disabled={pending || !me}>Place order</button>
    </form>
  );
}
