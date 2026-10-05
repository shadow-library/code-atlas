export interface ChargeResult {
  id: string;
  amountCents: number;
}

export interface PaymentProvider {
  readonly name: string;
  charge(amountCents: number): Promise<ChargeResult>;
  refund(chargeId: string): Promise<void>;
}
