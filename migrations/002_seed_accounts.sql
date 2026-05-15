-- Seed Beat 6's data: 47 Account rows with email=NULL, plus a handful with
-- email set, so the data-conformance axis returns a real violation count.
--
-- IMPORTANT: the 47 NULL rows are the literal data the demo blocks on. Do
-- not parameterise away from 47 — the demo script (Beat 6) cites the count
-- verbatim. If this seed is ever rerun, it tolerates re-runs via
-- ON CONFLICT DO NOTHING.

-- 47 NULL-email accounts (acct_null_001..047).
INSERT INTO accounts (id, email, display_name)
SELECT
    'acct_null_' || LPAD(i::text, 3, '0'),
    NULL,
    'Legacy Account ' || i
FROM generate_series(1, 47) AS i
ON CONFLICT (id) DO NOTHING;

-- A few accounts with email populated, for contrast in the violation report.
INSERT INTO accounts (id, email, display_name) VALUES
    ('acct_alice',    'alice@example.com',    'Alice Anderson'),
    ('acct_bob',      'bob@example.com',      'Bob Baker'),
    ('acct_carol',    'carol@example.com',    'Carol Chen'),
    ('acct_dave',     'dave@example.com',     'Dave Davis')
ON CONFLICT (id) DO NOTHING;

-- Pre-seed bank_integrations + authentication_methods so Beat 4's generated
-- HTTP handler has something to FK against in Beat 7's happy write.
INSERT INTO bank_integrations (id, provider) VALUES
    ('bi_plaid_demo', 'plaid'),
    ('bi_mx_demo',    'mx')
ON CONFLICT (id) DO NOTHING;

INSERT INTO authentication_methods (id, integration_id, kind) VALUES
    ('am_plaid_oauth', 'bi_plaid_demo', 'oauth'),
    ('am_mx_password', 'bi_mx_demo',    'password')
ON CONFLICT (id) DO NOTHING;
