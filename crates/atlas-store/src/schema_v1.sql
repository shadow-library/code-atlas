CREATE TABLE owners (
  id INTEGER PRIMARY KEY,
  key TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK (kind IN ('file', 'project')),
  repo TEXT,
  path TEXT,
  pass TEXT
);
CREATE INDEX owners_file ON owners(repo, path);
CREATE INDEX owners_pass ON owners(pass);

CREATE TABLE entities (
  id INTEGER PRIMARY KEY,
  key TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  repo TEXT,
  path TEXT,
  certainty_class TEXT NOT NULL,
  certainty_tier INTEGER NOT NULL,
  confidence REAL
);
CREATE INDEX entities_name ON entities(name);
CREATE INDEX entities_file ON entities(repo, path);
CREATE INDEX entities_kind ON entities(kind);

CREATE TABLE entity_owners (
  entity_id INTEGER NOT NULL REFERENCES entities(id),
  owner_id INTEGER NOT NULL REFERENCES owners(id),
  run_id INTEGER,
  data TEXT NOT NULL,
  UNIQUE (entity_id, owner_id)
);
CREATE INDEX entity_owners_owner ON entity_owners(owner_id, entity_id);

CREATE TABLE edges (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  src INTEGER NOT NULL,
  dst INTEGER NOT NULL,
  certainty_class TEXT NOT NULL,
  certainty_tier INTEGER NOT NULL,
  confidence REAL
);
CREATE INDEX edges_src ON edges(src);
CREATE INDEX edges_dst ON edges(dst);
CREATE INDEX edges_kind ON edges(kind);

CREATE TABLE edge_owners (
  edge_id INTEGER NOT NULL REFERENCES edges(id),
  owner_id INTEGER NOT NULL REFERENCES owners(id),
  run_id INTEGER,
  data TEXT NOT NULL,
  UNIQUE (edge_id, owner_id)
);
CREATE INDEX edge_owners_owner ON edge_owners(owner_id, edge_id);

CREATE TABLE unresolved_refs (
  owner_id INTEGER NOT NULL REFERENCES owners(id),
  ordinal INTEGER NOT NULL,
  name TEXT NOT NULL,
  src INTEGER NOT NULL,
  run_id INTEGER,
  data TEXT NOT NULL,
  PRIMARY KEY (owner_id, ordinal)
) WITHOUT ROWID;
CREATE INDEX unresolved_refs_name ON unresolved_refs(name);

CREATE TABLE files (
  repo TEXT NOT NULL,
  path TEXT NOT NULL,
  language TEXT,
  content_hash TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL,
  inode INTEGER,
  parse_status TEXT NOT NULL,
  analyzer_name TEXT,
  analyzer_version INTEGER,
  indexed_run INTEGER,
  PRIMARY KEY (repo, path)
) WITHOUT ROWID;

CREATE TABLE index_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  trigger TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  status TEXT NOT NULL CHECK (status IN ('running', 'ok', 'failed', 'partial')),
  phase_timings TEXT NOT NULL,
  counts TEXT NOT NULL,
  change_set TEXT NOT NULL,
  error TEXT
);

CREATE TABLE run_fact_deltas (
  run_id INTEGER NOT NULL REFERENCES index_runs(id),
  owner TEXT NOT NULL,
  entities_added INTEGER NOT NULL,
  entities_removed INTEGER NOT NULL,
  entities_changed INTEGER NOT NULL,
  edges_added INTEGER NOT NULL,
  edges_removed INTEGER NOT NULL,
  edges_changed INTEGER NOT NULL,
  unresolved_added INTEGER NOT NULL,
  unresolved_removed INTEGER NOT NULL,
  PRIMARY KEY (run_id, owner)
) WITHOUT ROWID;

CREATE TABLE llm_cache (
  key TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  response TEXT NOT NULL,
  usage TEXT NOT NULL,
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE llm_calls (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id INTEGER,
  task_kind TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  usage TEXT NOT NULL,
  cost_estimate REAL,
  cache TEXT NOT NULL CHECK (cache IN ('hit', 'miss')),
  duration_ms INTEGER NOT NULL,
  outcome TEXT NOT NULL CHECK (outcome IN ('ok', 'failed')),
  error TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX llm_calls_run ON llm_calls(run_id);

CREATE TABLE inferred_facts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  relation TEXT NOT NULL,
  payload TEXT NOT NULL,
  certainty TEXT NOT NULL,
  confidence REAL NOT NULL,
  evidence TEXT NOT NULL,
  freshness TEXT NOT NULL,
  freshness_state TEXT NOT NULL CHECK (freshness_state IN ('current', 'stale', 'needs_revalidation')),
  created_run INTEGER,
  llm_call_id INTEGER REFERENCES llm_calls(id)
);
CREATE INDEX inferred_facts_relation ON inferred_facts(relation);

CREATE TABLE inferred_subjects (
  fact_id INTEGER NOT NULL REFERENCES inferred_facts(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  entity_id INTEGER NOT NULL,
  PRIMARY KEY (fact_id, position)
) WITHOUT ROWID;
CREATE INDEX inferred_subjects_entity ON inferred_subjects(entity_id);

CREATE TABLE inferred_evidence (
  fact_id INTEGER NOT NULL REFERENCES inferred_facts(id) ON DELETE CASCADE,
  repo TEXT NOT NULL,
  path TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  PRIMARY KEY (fact_id, repo, path, content_hash)
) WITHOUT ROWID;
CREATE INDEX inferred_evidence_file ON inferred_evidence(repo, path);

CREATE TABLE derived_blobs (
  name TEXT PRIMARY KEY,
  version INTEGER NOT NULL,
  bytes BLOB NOT NULL,
  updated_run INTEGER
) WITHOUT ROWID;
