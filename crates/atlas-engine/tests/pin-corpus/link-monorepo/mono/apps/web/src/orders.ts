import { ordersApi } from './api/orders';

export const loadOrderItem = (itemId: string) => ordersApi.get(`/api/items/${itemId}`);

export const createOrderItem = (sku: string) => ordersApi.post('/api/items', { sku });

export const checkOrders = () => ordersApi.get('/api/health');
