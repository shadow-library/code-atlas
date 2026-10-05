import { PaymentProvider } from './payment-provider';

export async function checkout(provider: PaymentProvider, amountCents: number): Promise<string> {
  const result = await provider.charge(amountCents);
  return result.id;
}

export async function cancel(provider: PaymentProvider, chargeId: string): Promise<void> {
  await provider.refund(chargeId);
}
