export const SYNC_FIELDS: Record<string, string[]> = {
  'sync.pull_outline': ['outline_cursor'],
  'sync.push_outline': ['outline_cursor'],
  'sync.warm_outline': ['outline_cursor'],
  'sync.chill_outline': ['outline_cursor'],
  'sync.hydrate_outline': ['outline_cursor', 'hydration_window'],
  'sync.pull_shelf': ['shelf_token'],
  'sync.push_shelf': ['shelf_token'],
  'sync.probe_shelf': ['shelf_token'],
  'sync.compact_shelf': ['shelf_token'],
  'sync.evict_shelf': ['shelf_token', 'eviction_reason'],
  'sync.fold_replica': ['replica_epoch'],
  'sync.split_replica': ['replica_epoch'],
  'sync.ping_replica': ['replica_epoch'],
  'sync.mirror_replica': ['replica_epoch'],
  'sync.seal_epoch': ['replica_epoch'],
  'sync.thaw_epoch': ['replica_epoch'],
  'sync.prune_epoch': ['replica_epoch'],
  'sync.stamp_epoch': ['replica_epoch'],
  'sync.mark_conflict': ['conflict_hint'],
  'sync.clear_conflict': ['conflict_hint'],
  'sync.route_conflict': ['conflict_hint'],
  'sync.defer_conflict': ['conflict_hint'],
  'sync.rebase_cursor': ['outline_cursor'],
  'sync.drop_cursor': ['outline_cursor'],
  'sync.park_cursor': ['outline_cursor'],
  'sync.wake_cursor': ['outline_cursor'],
};

export function fieldsOf(kind: string): string[] {
  return SYNC_FIELDS[kind] ?? [];
}
