import { BaseProvider } from './base-provider';
import { ChargeResult, PaymentProvider } from './payment-provider';

export class StripeProvider extends BaseProvider implements PaymentProvider {
  readonly name = 'stripe';

  async charge(amountCents: number): Promise<ChargeResult> {
    this.log(`charge ${amountCents}`);
    return { id: `ch_${amountCents}`, amountCents };
  }

  async refund(chargeId: string): Promise<void> {
    await super.refund(chargeId);
  }
}
