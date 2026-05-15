-- Feature 3: add deterministic checksum column to mutation_log.
--
-- The base mutation_log (migration 001) already records type_id, command,
-- payload, ontology_version, actor, occurred_at — every field we need to
-- detect drift EXCEPT the checksum itself. We compute SHA256 over canonical
-- JSON of (type_id|entity_id|command|payload|ontology_version) at write time
-- and store it here so `agora verify` can compare it later against a fresh
-- checksum of whatever sits in the entity table.
--
-- Idempotent: ADD COLUMN IF NOT EXISTS plus CREATE INDEX IF NOT EXISTS.
-- Safe to run on F2's already-deployed instance — no data loss, no rewrite.

ALTER TABLE mutation_log
    ADD COLUMN IF NOT EXISTS checksum TEXT;

CREATE INDEX IF NOT EXISTS mutation_log_checksum_idx
    ON mutation_log (checksum);
