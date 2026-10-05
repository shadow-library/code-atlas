import { Test } from '@nestjs/testing';
import { createMock } from '@golevelup/ts-jest';
import { OrdersController } from './orders.controller';
import { OrdersService } from './orders.service';
import { Mailer } from './mailer';
import { Clock } from './clock';

describe('OrdersController', () => {
  const ordersService = { findOne: jest.fn(), create: vi.fn() };
  const mailer = createMock<Mailer>();
  let controller: OrdersController;
  let clock: jest.Mocked<Clock>;

  beforeEach(async () => {
    clock = { now: jest.fn() } as unknown as jest.Mocked<Clock>;
    const moduleRef = await Test.createTestingModule({
      providers: [OrdersController, { provide: OrdersService, useValue: ordersService }, { provide: Mailer, useValue: { send: jest.fn() } }],
    }).compile();
    controller = moduleRef.get(OrdersController);
  });

  it('should return the order', async () => {
    ordersService.findOne.mockResolvedValue({ id: 'o1' });
    clock.now.mockReturnValue(0);
    await controller.findOne('o1');
    expect(ordersService.findOne).toHaveBeenCalledWith('o1');
    expect(mailer.send).not.toHaveBeenCalled();
  });

  it('should spy on real collaborators', () => {
    const real = new OrdersService();
    jest.spyOn(real, 'create').mockResolvedValue({ id: 'o2' });
    vi.spyOn(Clock.prototype, 'now');
    jest.spyOn(console, 'log');
  });
});
