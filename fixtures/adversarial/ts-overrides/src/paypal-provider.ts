import { ChargeResult, PaymentProvider } from './payment-provider';

export class PaypalProvider implements PaymentProvider {
  readonly name = 'paypal';
  private readonly audits: string[] = [];

  async charge(amountCents: number): Promise<ChargeResult> {
    return { id: `pp_${amountCents}`, amountCents };
  }

  async refund(chargeId: string): Promise<void> {
    this.audit(chargeId);
  }

  private audit(chargeId: string): void {
    this.audits.push(chargeId);
  }
}
