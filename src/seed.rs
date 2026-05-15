//! Seed concept catalog the CLI consults when reuse-detecting offline.
//!
//! In the live system, the registry is queried via `GET /concepts`. For the
//! demo + offline mode we keep a hand-curated baseline of canonical concepts
//! so reuse detection always has something to match against.
//!
//! Two of these (`AuthenticationMethod`, `ProviderAuthFlag`) are intentionally
//! near-duplicates so DEMO.md Beat 2 ("link/conflict must be machine-detected,
//! not hand-curated for the demo") works against real signal — the CLI doesn't
//! know which of them the LLM-authored proposal is closer to.

use crate::ast::{
    Field, Ownership, OntologyType, PolicyClass, ProtoType,
};

#[derive(Debug, Clone)]
pub struct ConceptCard {
    pub fqn: String,
    pub summary: String, // semi-structured tokens used by Jaccard + embeddings
    pub spec: OntologyType,
}

pub fn baseline_concepts() -> Vec<ConceptCard> {
    vec![
        ConceptCard {
            fqn: "core.integrations.BankIntegration".into(),
            summary: "bank integration third-party account-aggregation provider \
                connection ledger plaid mx finicity yodlee account access"
                .into(),
            spec: OntologyType {
                namespace: "core.integrations".into(),
                name: "BankIntegration".into(),
                version: 3,
                fields: vec![
                    Field {
                        name: "id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("Stable id".into()),
                    },
                    Field {
                        name: "provider".into(),
                        proto_type: ProtoType::String,
                        proto_number: 2,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("'plaid' | 'mx' | 'finicity'".into()),
                    },
                ],
                relations: vec![],
                invariants: vec![
                    "Every active BankIntegration has at least one supported \
                     AuthenticationMethod"
                        .into(),
                ],
                ownership: Ownership {
                    team: "integrations-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Internal,
                locality: Some("region".into()),
                doc: Some("Canonical bank-integration record.".into()),
            },
        },
        ConceptCard {
            fqn: "core.integrations.AuthenticationMethod".into(),
            summary: "authentication method oauth password sso mfa biometric \
                webauthn passkey login credential mechanism"
                .into(),
            spec: OntologyType {
                namespace: "core.integrations".into(),
                name: "AuthenticationMethod".into(),
                version: 1,
                fields: vec![
                    Field {
                        name: "kind".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("'oauth'|'password'|'sso'|'mfa'".into()),
                    },
                ],
                relations: vec![],
                invariants: vec![],
                ownership: Ownership {
                    team: "integrations-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Internal,
                locality: None,
                doc: Some("How a user proves identity to a BankIntegration.".into()),
            },
        },
        ConceptCard {
            fqn: "core.integrations.ProviderAuthFlag".into(),
            summary: "provider auth flag boolean enabled allowed login method \
                supports oauth biometric mfa password capability"
                .into(),
            spec: OntologyType {
                namespace: "core.integrations".into(),
                name: "ProviderAuthFlag".into(),
                version: 1,
                fields: vec![Field {
                    name: "supports_biometric".into(),
                    proto_type: ProtoType::Bool,
                    proto_number: 1,
                    required: true,
                    since_version: 1,
                    deprecated_in: None,
                    classification: PolicyClass::Internal,
                    doc: Some("Provider self-reported biometric capability.".into()),
                }],
                relations: vec![],
                invariants: vec![],
                ownership: Ownership {
                    team: "integrations-platform".into(),
                    semantic_steward: None,
                },
                policy_class: PolicyClass::Internal,
                locality: None,
                doc: Some(
                    "Per-provider boolean capability flags (legacy, near-duplicate \
                     of AuthenticationMethod relations)."
                        .into(),
                ),
            },
        },
        ConceptCard {
            fqn: "core.users.Account".into(),
            summary: "account customer profile email login authentication \
                    identity holder ledger record nullable optional"
                .into(),
            spec: OntologyType {
                namespace: "core.users".into(),
                name: "Account".into(),
                version: 2,
                fields: vec![
                    Field {
                        name: "id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("Stable id".into()),
                    },
                    // email is intentionally OPTIONAL in the baseline — this is
                    // what makes Beat 6's risky proposal (optional→required)
                    // a real semantic refinement, and what makes the 47 NULL
                    // rows a real data-conformance violation.
                    Field {
                        name: "email".into(),
                        proto_type: ProtoType::String,
                        proto_number: 2,
                        required: false,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Pii,
                        doc: Some("Contact email. Currently optional; many \
                                   legacy rows have NULL.".into()),
                    },
                ],
                relations: vec![],
                invariants: vec![],
                ownership: Ownership {
                    team: "identity-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Pii,
                locality: Some("region".into()),
                doc: Some("Canonical customer account record.".into()),
            },
        },
        // ====================================================================
        // F8 — second domain: Customer 360.
        //
        // Three concepts owned by `customer-platform` (Customer, LoyaltyTier)
        // and `analytics-platform` (PurchaseHistory). They exist to prove
        // Agora's plumbing is domain-generic — the same risk gate, agent loop,
        // policy enforcement, and explorer all work against these without a
        // line of domain-specific code in `agent.rs` / `check.rs` / `verify.rs`.
        //
        // Customer.email is intentionally OPTIONAL (mirroring Account.email)
        // so a Beat-6-style "tighten Customer.email to required" proposal can
        // be driven through the data-conformance axis against the seeded
        // `customers` table (migrations/005). 5 of the 20 seeded rows carry
        // NULL email so the violation count is real.
        // ====================================================================
        ConceptCard {
            fqn: "core.customer.Customer".into(),
            summary: "customer 360 profile email contact loyalty signup source \
                      crm marketing identity holder retail consumer"
                .into(),
            spec: OntologyType {
                namespace: "core.customer".into(),
                name: "Customer".into(),
                version: 1,
                fields: vec![
                    Field {
                        name: "id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("Stable customer id".into()),
                    },
                    Field {
                        name: "email".into(),
                        proto_type: ProtoType::String,
                        proto_number: 2,
                        required: false,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Pii,
                        doc: Some(
                            "Contact email. Optional today; the second risky \
                             proposal demonstrates tightening this to required."
                                .into(),
                        ),
                    },
                    Field {
                        name: "display_name".into(),
                        proto_type: ProtoType::String,
                        proto_number: 3,
                        required: false,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Pii,
                        doc: Some("Human-readable name for CRM displays.".into()),
                    },
                    Field {
                        name: "signup_source".into(),
                        proto_type: ProtoType::String,
                        proto_number: 4,
                        required: false,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some(
                            "Where this customer came from: 'web' | 'app' | \
                             'partner' | 'import'."
                                .into(),
                        ),
                    },
                ],
                relations: vec![],
                invariants: vec![
                    "Every Customer has a stable `id`.".into(),
                    "If `email` is set it is unique within the customers table."
                        .into(),
                ],
                ownership: Ownership {
                    team: "customer-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Pii,
                locality: Some("region".into()),
                doc: Some(
                    "Canonical customer record for the Customer 360 domain."
                        .into(),
                ),
            },
        },
        ConceptCard {
            fqn: "core.customer.LoyaltyTier".into(),
            summary: "loyalty tier rewards discount membership level customer \
                      gold silver bronze platinum"
                .into(),
            spec: OntologyType {
                namespace: "core.customer".into(),
                name: "LoyaltyTier".into(),
                version: 1,
                fields: vec![
                    Field {
                        name: "tier_name".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Public,
                        doc: Some("e.g. 'gold' | 'silver' | 'bronze'.".into()),
                    },
                    Field {
                        name: "discount_pct".into(),
                        proto_type: ProtoType::Int64,
                        proto_number: 2,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Public,
                        doc: Some(
                            "Whole-number percentage discount (0..=100) applied \
                             to a Customer's eligible purchases."
                                .into(),
                        ),
                    },
                ],
                relations: vec![],
                invariants: vec![
                    "`discount_pct` is bounded by 0..=100.".into(),
                    "`tier_name` is unique across LoyaltyTier rows.".into(),
                ],
                ownership: Ownership {
                    team: "customer-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Public,
                locality: None,
                doc: Some(
                    "Canonical loyalty-tier definition. Customer 360 domain."
                        .into(),
                ),
            },
        },
        ConceptCard {
            fqn: "core.customer.PurchaseHistory".into(),
            summary: "purchase history transaction order amount cents customer \
                      analytics revenue order log buy retail"
                .into(),
            spec: OntologyType {
                namespace: "core.customer".into(),
                name: "PurchaseHistory".into(),
                version: 1,
                fields: vec![
                    Field {
                        name: "id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("Stable purchase-event id.".into()),
                    },
                    Field {
                        name: "customer_id".into(),
                        proto_type: ProtoType::Ref(
                            "core.customer.Customer".into(),
                        ),
                        proto_number: 2,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some(
                            "References Customer.id; orphans are illegal.".into(),
                        ),
                    },
                    Field {
                        name: "amount_cents".into(),
                        proto_type: ProtoType::Int64,
                        proto_number: 3,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Sensitive,
                        doc: Some(
                            "Purchase amount in cents (no currency conversion at \
                             this layer)."
                                .into(),
                        ),
                    },
                    Field {
                        name: "occurred_at".into(),
                        proto_type: ProtoType::Timestamp,
                        proto_number: 4,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some(
                            "Wall-clock time of the purchase, server-recorded.".into(),
                        ),
                    },
                ],
                relations: vec![],
                invariants: vec![
                    "`customer_id` must reference an existing Customer row."
                        .into(),
                    "`amount_cents` is non-negative.".into(),
                    "`occurred_at` is monotonically increasing per customer_id."
                        .into(),
                ],
                ownership: Ownership {
                    team: "analytics-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Sensitive,
                locality: Some("region".into()),
                doc: Some(
                    "Per-customer purchase-event ledger; consumed by the \
                     analytics domain to compute CLV/CAC."
                        .into(),
                ),
            },
        },
        ConceptCard {
            fqn: "core.users.User".into(),
            summary: "user account person profile identity email name customer".into(),
            spec: OntologyType {
                namespace: "core.users".into(),
                name: "User".into(),
                version: 5,
                fields: vec![
                    Field {
                        name: "id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: None,
                    },
                    Field {
                        name: "email".into(),
                        proto_type: ProtoType::String,
                        proto_number: 2,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Pii,
                        doc: None,
                    },
                ],
                relations: vec![],
                invariants: vec![],
                ownership: Ownership {
                    team: "identity-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Pii,
                locality: Some("region".into()),
                doc: Some("Canonical user record.".into()),
            },
        },
        // ====================================================================
        // F9 — third domain: Compliance / GRC.
        //
        // Two concepts owned by `compliance-platform`. Same property F8
        // exercised on Customer 360: same risk gate, agent loop, policy
        // enforcement, explorer, and verify operate on Compliance without
        // a line of domain-specific logic in agent.rs / check.rs (grep-
        // verified at commit time).
        //
        // `AuditFinding.resolved_at` is intentionally OPTIONAL — the
        // Beat-6-equivalent risky proposal "tighten resolved_at to
        // required" exercises the data-conformance axis against the
        // seeded rows whose findings are still open / under investigation.
        // ====================================================================
        ConceptCard {
            fqn: "core.compliance.AuditFinding".into(),
            summary: "audit finding compliance gdpr soc2 pci dss control \
                      violation severity status incident remediation grc \
                      regulatory review"
                .into(),
            spec: OntologyType {
                namespace: "core.compliance".into(),
                name: "AuditFinding".into(),
                version: 1,
                fields: vec![
                    Field {
                        name: "id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("Stable audit-finding id".into()),
                    },
                    Field {
                        name: "rule_id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 2,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some(
                            "References the ComplianceRule that was violated."
                                .into(),
                        ),
                    },
                    Field {
                        name: "severity".into(),
                        proto_type: ProtoType::String,
                        proto_number: 3,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("'critical' | 'high' | 'medium' | 'low'.".into()),
                    },
                    Field {
                        name: "status".into(),
                        proto_type: ProtoType::String,
                        proto_number: 4,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some(
                            "'open' | 'investigating' | 'resolved' | 'accepted_risk'."
                                .into(),
                        ),
                    },
                    Field {
                        name: "opened_at".into(),
                        proto_type: ProtoType::Timestamp,
                        proto_number: 5,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some("When the auditor first flagged this finding.".into()),
                    },
                    Field {
                        name: "resolved_at".into(),
                        proto_type: ProtoType::Timestamp,
                        proto_number: 6,
                        required: false,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Internal,
                        doc: Some(
                            "When the finding was closed out. NULL for open / \
                             investigating findings — the risky proposal demonstrates \
                             tightening this to required."
                                .into(),
                        ),
                    },
                    Field {
                        name: "notes".into(),
                        proto_type: ProtoType::String,
                        proto_number: 7,
                        required: false,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Pii,
                        doc: Some(
                            "Free-text remediation notes. Classified PII because \
                             audit details often reference customer records."
                                .into(),
                        ),
                    },
                ],
                relations: vec![],
                invariants: vec![
                    "Every AuditFinding has a non-empty `rule_id` matching an \
                     active ComplianceRule."
                        .into(),
                    "`status = 'resolved'` requires `resolved_at` to be non-null."
                        .into(),
                    "`opened_at <= resolved_at` when both are set.".into(),
                ],
                ownership: Ownership {
                    team: "compliance-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Pii,
                locality: Some("region".into()),
                doc: Some(
                    "Canonical audit-finding record for the Compliance / GRC \
                     domain. Backs SOC2 / GDPR / PCI-DSS findings."
                        .into(),
                ),
            },
        },
        ConceptCard {
            fqn: "core.compliance.ComplianceRule".into(),
            summary: "compliance rule policy soc2 gdpr pci dss hipaa control \
                      framework regulatory active deprecated standard"
                .into(),
            spec: OntologyType {
                namespace: "core.compliance".into(),
                name: "ComplianceRule".into(),
                version: 1,
                fields: vec![
                    Field {
                        name: "rule_id".into(),
                        proto_type: ProtoType::String,
                        proto_number: 1,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Public,
                        doc: Some("Stable rule identifier, e.g. 'SOC2-CC6.1'.".into()),
                    },
                    Field {
                        name: "description".into(),
                        proto_type: ProtoType::String,
                        proto_number: 2,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Public,
                        doc: Some("Human-readable description of the control.".into()),
                    },
                    Field {
                        name: "regulatory_framework".into(),
                        proto_type: ProtoType::String,
                        proto_number: 3,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Public,
                        doc: Some(
                            "Which framework the rule comes from: 'SOC2' | 'GDPR' \
                             | 'PCI-DSS' | 'HIPAA' | etc."
                                .into(),
                        ),
                    },
                    Field {
                        name: "active".into(),
                        proto_type: ProtoType::Bool,
                        proto_number: 4,
                        required: true,
                        since_version: 1,
                        deprecated_in: None,
                        classification: PolicyClass::Public,
                        doc: Some(
                            "False for rules that have been deprecated by their \
                             framework owners."
                                .into(),
                        ),
                    },
                ],
                relations: vec![],
                invariants: vec![
                    "`rule_id` is globally unique across all frameworks.".into(),
                    "`regulatory_framework` is non-empty.".into(),
                ],
                ownership: Ownership {
                    team: "compliance-platform".into(),
                    semantic_steward: Some("core-ontology".into()),
                },
                policy_class: PolicyClass::Public,
                locality: None,
                doc: Some(
                    "Canonical compliance-rule definition. Compliance / GRC \
                     domain."
                        .into(),
                ),
            },
        },
    ]
}
