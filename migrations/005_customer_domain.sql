-- F8: second domain — Customer 360.
--
-- Adds the `customers` entity table so:
--   * F2's data-conformance axis can run a real SQL count for the
--     "tighten Customer.email to required" risky proposal (5 NULL emails
--     out of 20 → block).
--   * F5's policy enforcement works against a non-bank-integration concept
--     (`POST /entities/Customer` with actor selectors).
--   * F6's agent loop runs end-to-end on a customer-domain prompt with no
--     domain-specific code in agent.rs.
--
-- Idempotent: every statement is IF NOT EXISTS / ON CONFLICT DO NOTHING so
-- a re-run on an existing DB is a no-op.

-- ---------- entity table ----------

CREATE TABLE IF NOT EXISTS customers (
    id              TEXT PRIMARY KEY,
    email           TEXT,                                   -- intentionally nullable
    display_name    TEXT,
    signup_source   TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The risky-proposal scenario relies on email being nullable AND some rows
-- having NULL values. The partial index just keeps lookups by email fast
-- (and the data-conformance axis only counts NULL rows, so it's not used by
-- the count itself).
CREATE INDEX IF NOT EXISTS customers_email_idx
    ON customers (email)
    WHERE email IS NOT NULL;

-- ---------- seed: 20 rows, 5 with NULL email ----------
-- Mix of signup_sources + display_names so the explorer view has variety.
-- Using ON CONFLICT DO NOTHING keeps the migration idempotent: if someone
-- runs it twice (or the rows already exist from a previous demo) we don't
-- re-insert and we don't error out.

INSERT INTO customers (id, email, display_name, signup_source) VALUES
    ('cust_001', 'ada@example.com',       'Ada Lovelace',     'web'),
    ('cust_002', 'alan@example.com',      'Alan Turing',      'web'),
    ('cust_003', 'grace@example.com',     'Grace Hopper',     'app'),
    ('cust_004', 'linus@example.com',     'Linus Torvalds',   'app'),
    ('cust_005', 'donald@example.com',    'Donald Knuth',     'partner'),
    ('cust_006', 'rich@example.com',      'Richard Hamming',  'web'),
    ('cust_007', 'edsger@example.com',    'Edsger Dijkstra',  'partner'),
    ('cust_008', 'barbara@example.com',   'Barbara Liskov',   'web'),
    ('cust_009', 'leslie@example.com',    'Leslie Lamport',   'app'),
    ('cust_010', 'fran@example.com',      'Fran Allen',       'app'),
    ('cust_011', 'tim@example.com',       'Tim Berners-Lee',  'partner'),
    ('cust_012', 'guido@example.com',     'Guido van Rossum', 'web'),
    ('cust_013', 'james@example.com',     'James Gosling',    'web'),
    ('cust_014', 'rasmus@example.com',    'Rasmus Lerdorf',   'app'),
    ('cust_015', 'yukihiro@example.com',  'Yukihiro Matz',    'partner'),
    -- 5 NULL-email rows: pure import-source customers whose emails were
    -- never collected. These are what the risky proposal would invalidate.
    ('cust_016', NULL,                    'Anonymous A',      'import'),
    ('cust_017', NULL,                    'Anonymous B',      'import'),
    ('cust_018', NULL,                    'Anonymous C',      'import'),
    ('cust_019', NULL,                    NULL,               'import'),
    ('cust_020', NULL,                    NULL,               'import')
ON CONFLICT (id) DO NOTHING;
