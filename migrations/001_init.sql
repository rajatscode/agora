-- Agora M0 schema.
--
-- One Postgres instance carries:
--   1. Operational data tables (Account, BankIntegration, AuthenticationMethod) —
--      what Beat 6's data-conformance axis queries.
--   2. Registry tables (ontology_types, generated_artifacts) — what Beat 8's
--      explorer surfaces.
--   3. Append-only mutation_log — what Beat 7's verify checks for tampering.
--
-- This migration is idempotent: it uses IF NOT EXISTS so the F2 CLI can
-- run it on startup without trampling existing data.

-- ---------- operational tables ----------

-- Account is the Beat 6 risky-proposal target. The 47 NULL-email rows that
-- block "tighten Account.email optional→required" are seeded in 002.
CREATE TABLE IF NOT EXISTS accounts (
    id          TEXT PRIMARY KEY,
    email       TEXT,                                   -- intentionally nullable
    display_name TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS bank_integrations (
    id          TEXT PRIMARY KEY,
    provider    TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS authentication_methods (
    id              TEXT PRIMARY KEY,
    integration_id  TEXT NOT NULL REFERENCES bank_integrations(id),
    kind            TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------- registry tables ----------

CREATE TABLE IF NOT EXISTS ontology_types (
    id          TEXT PRIMARY KEY,
    namespace   TEXT NOT NULL,
    name        TEXT NOT NULL,
    version     INT  NOT NULL,
    spec_json   JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (namespace, name, version)
);

CREATE TABLE IF NOT EXISTS generated_artifacts (
    id            BIGSERIAL PRIMARY KEY,
    proposal_id   TEXT NOT NULL,
    kind          TEXT NOT NULL,      -- 'proto' | 'ddl' | 'handler' | 'openfga' | 'check_report'
    path          TEXT NOT NULL,
    payload       TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS generated_artifacts_proposal_idx
    ON generated_artifacts (proposal_id);

-- ---------- append-only mutation log ----------

CREATE TABLE IF NOT EXISTS mutation_log (
    seq               BIGSERIAL PRIMARY KEY,
    type_id           TEXT NOT NULL,
    ontology_version  INT NOT NULL,
    entity_id         TEXT NOT NULL,
    command           TEXT NOT NULL,   -- 'Create' | 'Update' | 'Deprecate'
    payload           JSONB NOT NULL,
    payload_proto_b64 TEXT,
    actor             TEXT NOT NULL,
    occurred_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS mutation_log_type_idx ON mutation_log (type_id, occurred_at);
CREATE INDEX IF NOT EXISTS mutation_log_entity_idx ON mutation_log (entity_id);
