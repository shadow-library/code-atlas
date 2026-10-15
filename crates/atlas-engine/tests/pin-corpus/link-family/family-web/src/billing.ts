export async function loadLedger(token: string) {
  return fetch(`${process.env.FAMILY_BLEND_URL}/billing/ledger`, { headers: { Authorization: `Bearer ${token}` } });
}
