import { OrdersController } from './orders.controller';
import { type OrdersService } from './orders.service';

describe('OrdersController', () => {
  const ordersService = { findOne: jest.fn(), create: jest.fn() };
  const controller = new OrdersController(ordersService as unknown as OrdersService);

  afterEach(() => jest.resetAllMocks());

  it('should delegate findOne to the service', async () => {
    ordersService.findOne.mockResolvedValue({ id: 'o1', display: '$12.00' });
    await expect(controller.findOne('o1')).resolves.toEqual({ id: 'o1', display: '$12.00' });
    expect(ordersService.findOne).toHaveBeenCalledWith('o1');
  });

  it('should delegate create to the service', async () => {
    ordersService.create.mockResolvedValue({ id: 'o2' });
    await controller.create({ userId: 'u1', subtotalCents: 1000 });
    expect(ordersService.create).toHaveBeenCalledWith({ userId: 'u1', subtotalCents: 1000 });
  });
});
