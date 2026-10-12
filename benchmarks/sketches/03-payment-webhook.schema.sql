-- Operator-owned application schema for `03-payment-webhook.krit` and
-- `03b-payment-webhook-buildable.krit`. Krit never creates, migrates, or
-- resets an application database, so this schema is applied out of band:
--
--   mkdir -p benchmarks/sketches/data && chmod 700 benchmarks/sketches/data
--   sqlite3 benchmarks/sketches/data/payments.db < benchmarks/sketches/03-payment-webhook.schema.sql
--   chmod 600 benchmarks/sketches/data/payments.db
--
-- The event id is the primary key, so the host statement's
-- `ON CONFLICT(event_id) DO NOTHING` records each delivery once.
CREATE TABLE IF NOT EXISTS payments (
    event_id TEXT PRIMARY KEY,
    intent_id TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    currency TEXT NOT NULL
);
