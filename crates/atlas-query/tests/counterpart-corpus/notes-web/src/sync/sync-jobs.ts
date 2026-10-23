export interface SyncJob {
  kind: string;
  outline_cursor?: string;
  shelf_token?: string;
  replica_epoch?: number;
  conflict_hint?: string;
}

export const SYNC_JOB_KINDS = [
  'sync.pull_outline',
  'sync.push_outline',
  'sync.warm_outline',
  'sync.chill_outline',
  'sync.pull_shelf',
  'sync.push_shelf',
  'sync.probe_shelf',
  'sync.compact_shelf',
  'sync.fold_replica',
  'sync.split_replica',
  'sync.ping_replica',
  'sync.mirror_replica',
  'sync.seal_epoch',
  'sync.thaw_epoch',
  'sync.prune_epoch',
  'sync.stamp_epoch',
  'sync.mark_conflict',
  'sync.clear_conflict',
  'sync.route_conflict',
  'sync.defer_conflict',
  'sync.rebase_cursor',
  'sync.drop_cursor',
  'sync.park_cursor',
  'sync.wake_cursor',
];

export function queueSync(kind: string, cursor: string): SyncJob {
  return { kind, outline_cursor: cursor };
}
