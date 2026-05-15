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
    ]
}
