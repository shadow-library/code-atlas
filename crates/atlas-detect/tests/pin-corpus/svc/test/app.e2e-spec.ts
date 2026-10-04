describe('app', () => {
  it('boots', () => {
    expect(process.env.E2E_BASE_URL).toBeDefined();
  });
});
