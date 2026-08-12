ALTER TABLE sessions ADD COLUMN owner_uid INTEGER;
ALTER TABLE sessions ADD COLUMN owner_username TEXT;
ALTER TABLE sessions ADD COLUMN account_label TEXT;

CREATE INDEX IF NOT EXISTS idx_sessions_owner ON sessions(owner_uid, owner_username);
CREATE INDEX IF NOT EXISTS idx_sessions_account ON sessions(owner_uid, account_label);
