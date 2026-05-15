-- F5: policy enforcement.
--
-- We extend the append-only mutation_log with a `denial_reason TEXT` column.
-- Allowed writes get this NULL; denied attempts get the verbatim reason from
-- `policy::PolicyDecision::Deny`. Denials are recorded as their own rows
-- (operation = 'DenyAttempt') so the audit trail captures who tried what.
--
-- This migration is idempotent: it uses IF NOT EXISTS so a re-run is safe.

ALTER TABLE mutation_log
    ADD COLUMN IF NOT EXISTS denial_reason TEXT NULL;

-- Index helps the explorer / audit views surface denials quickly without
-- a full mutation_log scan.
CREATE INDEX IF NOT EXISTS mutation_log_denial_idx
    ON mutation_log (type_id, entity_id)
    WHERE denial_reason IS NOT NULL;
