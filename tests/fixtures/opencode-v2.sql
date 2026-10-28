-- Synthetic fixture schema, based on anomalyco/opencode commit
-- 5ed03ad1377b549df83aa52dc11bdb07593a5327, packages/core/src/session/sql.ts.
-- Unused project/permission/account columns are intentionally omitted.
CREATE TABLE session_v2 (
    id TEXT PRIMARY KEY, title TEXT, directory TEXT NOT NULL, version TEXT NOT NULL,
    parent_id TEXT, fork_session_id TEXT, fork_boundary TEXT,
    time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER
);
CREATE TABLE session_message (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL,
    time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
);
CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
CREATE INDEX session_message_session_type_seq_idx ON session_message(session_id, type, seq);
