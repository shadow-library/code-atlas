import { Injectable } from '@nestjs/common';

import { roundHalfEven } from './rounding';

const TAX_RATE = 0.0825;

@Injectable()
export class BillingService {
  applyTax(subtotalCents: number): number {
    return subtotalCents + roundHalfEven(subtotalCents * TAX_RATE);
  }
}
