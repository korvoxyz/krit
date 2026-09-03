-- Operator-owned application schema for `examples/database-webhook.krit`.
-- Krit never creates, migrates, or resets an application database, so this
-- schema is applied out of band before the example is invoked:
--
--   mkdir -p examples/data && chmod 700 examples/data
--   sqlite3 examples/data/catalog.db < examples/database-webhook.schema.sql
--   chmod 600 examples/data/catalog.db
--
-- The table holds no real or sensitive data; it only records request paths.
CREATE TABLE IF NOT EXISTS visits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL
);
