import { checkout } from './checkout';
import { PaymentProvider } from './payment-provider';

describe('checkout', () => {
  it('charges the provider and returns the charge id', async () => {
    const provider = { charge: jest.fn().mockResolvedValue({ id: 'ch_1', amountCents: 100 }) } as unknown as PaymentProvider;

    await expect(checkout(provider, 100)).resolves.toBe('ch_1');
    expect(provider.charge).toHaveBeenCalledWith(100);
  });
});
