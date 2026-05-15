-- F9: third domain — Compliance / GRC.
--
-- Adds the `audit_findings` entity table so:
--   * F2's data-conformance axis can run a real SQL count for the
--     "tighten AuditFinding.resolved_at to required" risky proposal
--     (4 open/investigating rows have NULL resolved_at → block).
--   * F5's policy enforcement works against a non-Customer / non-Bank
--     concept (`POST /entities/AuditFinding` with actor selectors).
--   * F6's agent loop runs end-to-end on a compliance-domain prompt
--     with no domain-specific code in agent.rs.
--   * F4-style verify oob detection surfaces unmanaged rows the same
--     way it does for accounts / customers.
--
-- Idempotent: every statement is IF NOT EXISTS / ON CONFLICT DO NOTHING.

-- ---------- entity table ----------

CREATE TABLE IF NOT EXISTS audit_findings (
    id                  TEXT PRIMARY KEY,
    rule_id             TEXT NOT NULL,
    severity            TEXT NOT NULL,
    status              TEXT NOT NULL,
    opened_at           TIMESTAMPTZ NOT NULL,
    resolved_at         TIMESTAMPTZ,                    -- intentionally nullable
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS audit_findings_status_idx
    ON audit_findings (status);
-- Partial index lets us quickly find still-open findings (the NULL bucket
-- the risky-tighten proposal targets).
CREATE INDEX IF NOT EXISTS audit_findings_open_idx
    ON audit_findings (id)
    WHERE resolved_at IS NULL;

-- ---------- seed: 15 rows; 4 have NULL resolved_at ----------
-- Mix of severities × frameworks × statuses so the explorer has variety.

INSERT INTO audit_findings (id, rule_id, severity, status, opened_at, resolved_at, notes) VALUES
    -- 11 closed findings (status='resolved' or 'accepted_risk') with resolved_at set.
    ('af_001', 'SOC2-CC6.1',  'high',     'resolved',       '2025-11-01T10:00:00Z', '2025-11-15T16:30:00Z', 'Patched access-review automation.'),
    ('af_002', 'SOC2-CC7.2',  'medium',   'resolved',       '2025-11-03T09:15:00Z', '2025-11-10T11:00:00Z', 'Log retention extended to 365d.'),
    ('af_003', 'GDPR-Art32',  'critical', 'resolved',       '2025-10-15T14:00:00Z', '2025-10-22T18:45:00Z', 'Encryption-at-rest enabled on staging.'),
    ('af_004', 'PCI-DSS-3.4', 'critical', 'resolved',       '2025-10-20T08:00:00Z', '2025-10-25T17:00:00Z', 'PAN tokenisation rolled out.'),
    ('af_005', 'SOC2-CC8.1',  'low',      'resolved',       '2025-12-01T11:00:00Z', '2025-12-02T11:00:00Z', 'Changelog metadata added to deploy pipeline.'),
    ('af_006', 'GDPR-Art30',  'medium',   'resolved',       '2025-11-20T09:00:00Z', '2025-12-05T15:00:00Z', 'Records-of-processing inventory completed.'),
    ('af_007', 'HIPAA-164.308', 'high',   'accepted_risk',  '2025-09-10T12:00:00Z', '2025-09-20T17:00:00Z', 'Compensating control documented; risk accepted by CISO.'),
    ('af_008', 'SOC2-CC6.6',  'medium',   'resolved',       '2025-11-12T13:00:00Z', '2025-11-18T16:00:00Z', 'MFA enforced on all admin consoles.'),
    ('af_009', 'PCI-DSS-1.3', 'high',     'resolved',       '2025-10-30T10:00:00Z', '2025-11-05T14:00:00Z', 'Network segmentation review passed.'),
    ('af_010', 'GDPR-Art33',  'critical', 'resolved',       '2025-08-15T08:30:00Z', '2025-08-15T19:30:00Z', 'Breach-notification SOP rehearsed; under 72h.'),
    ('af_011', 'SOC2-CC9.2',  'low',      'resolved',       '2025-12-08T10:00:00Z', '2025-12-09T10:00:00Z', 'Vendor-risk questionnaire automated.'),
    -- 4 still-open findings with NULL resolved_at. These are what the
    -- risky proposal would invalidate.
    ('af_012', 'SOC2-CC6.1',  'high',     'investigating',  '2025-12-15T09:00:00Z', NULL,                    'Suspected privileged-access misuse; under review.'),
    ('af_013', 'GDPR-Art32',  'critical', 'open',           '2026-01-03T11:00:00Z', NULL,                    NULL),
    ('af_014', 'PCI-DSS-8.2', 'medium',   'open',           '2026-02-10T08:00:00Z', NULL,                    'Password policy below 12 chars in legacy app.'),
    ('af_015', 'HIPAA-164.312','low',     'investigating',  '2026-03-01T14:00:00Z', NULL,                    'Audit-log archive lookup time exceeding SLA.')
ON CONFLICT (id) DO NOTHING;
