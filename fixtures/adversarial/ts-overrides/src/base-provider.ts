import { ChargeResult, PaymentProvider } from './payment-provider';

export abstract class BaseProvider implements PaymentProvider {
  abstract readonly name: string;
  protected readonly entries: string[] = [];

  abstract charge(amountCents: number): Promise<ChargeResult>;

  async refund(chargeId: string): Promise<void> {
    this.log(`refund ${chargeId}`);
  }

  protected log(message: string): void {
    this.entries.push(`[${this.name}] ${message}`);
  }
}
