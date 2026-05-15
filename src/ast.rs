//! Shared ontology AST + change-proposal types.
//!
//! Aligned with STACK.md Artifact 1 ("Ontology AST") but extended on the
//! proposal side to carry every field Feature 1 requires
//! (compatibility, ownership, tests, provenance).
//!
//! Serialization is canonical JSON via serde. These types are also what the
//! Anthropic structured-output prompt is shaped by.

use serde::{Deserialize, Serialize};

// -------- core ontology --------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum PolicyClass {
    Public,
    Internal,
    Sensitive,
    Pii,
}

impl Default for PolicyClass {
    fn default() -> Self {
        PolicyClass::Internal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum ProtoType {
    String,
    Int64,
    Bool,
    Bytes,
    Timestamp,
    Ref(String), // namespaced type ref e.g. "core.integrations.AuthenticationMethod"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    ManyToMany,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeRef {
    pub namespace: String,
    pub name: String,
}

impl TypeRef {
    pub fn fqn(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub proto_type: ProtoType,
    pub proto_number: u32,
    pub required: bool,
    pub since_version: u32,
    #[serde(default)]
    pub deprecated_in: Option<u32>,
    #[serde(default)]
    pub classification: PolicyClass,
    #[serde(default)]
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub name: String,
    pub from: TypeRef,
    pub to: TypeRef,
    pub cardinality: Cardinality,
    pub since_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Ownership {
    pub team: String,                // e.g. "integrations-platform"
    #[serde(default)]
    pub semantic_steward: Option<String>, // e.g. "core-ontology"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyType {
    pub namespace: String,
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub invariants: Vec<String>,
    pub ownership: Ownership,
    #[serde(default)]
    pub policy_class: PolicyClass,
    #[serde(default)]
    pub locality: Option<String>,
    #[serde(default)]
    pub doc: Option<String>,
}

// -------- change kinds --------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    AddField {
        type_ref: TypeRef,
        field: Field,
    },
    AddRelation {
        relation: Relation,
    },
    DeprecateField {
        type_ref: TypeRef,
        field_name: String,
    },
    ReclassifyField {
        type_ref: TypeRef,
        field_name: String,
        to: PolicyClass,
    },
    CreateType {
        spec: OntologyType,
    },
    /// Tighten an existing field's constraints (e.g. nullable→required, widening
    /// type narrowed). Distinct from AddField because the field already exists
    /// and the constraint change can invalidate historical rows — Feature 2's
    /// data-conformance axis is the one that decides whether real data
    /// survives this.
    TightenField {
        type_ref: TypeRef,
        field_name: String,
        /// Was the field optional before this proposal? (i.e. `required=false`)
        from_required: bool,
        /// Will the field be required after this proposal? (i.e. `required=true`)
        to_required: bool,
    },
}

impl Change {
    pub fn target(&self) -> TypeRef {
        match self {
            Change::AddField { type_ref, .. }
            | Change::DeprecateField { type_ref, .. }
            | Change::ReclassifyField { type_ref, .. }
            | Change::TightenField { type_ref, .. } => type_ref.clone(),
            Change::AddRelation { relation } => relation.from.clone(),
            Change::CreateType { spec } => TypeRef {
                namespace: spec.namespace.clone(),
                name: spec.name.clone(),
            },
        }
    }
}

// -------- semantic + compatibility --------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticContract {
    pub meaning_before: String,
    pub meaning_after: String,
    /// Free-text explanation of WHY this preserves (or changes) meaning.
    #[serde(default)]
    pub justification: Option<String>,
    /// Invariants the proposal commits to upholding once the change ships.
    /// E.g. "Every active BankIntegration has at least one supported AuthenticationMethod".
    /// The risk-gate workstream evaluates these via the LLM-axis check.
    /// Heuristic author MUST populate at least two; LLM author SHOULD populate two-plus.
    #[serde(default)]
    pub invariants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityClass {
    Additive,
    Refinement,
    Breaking,
    Dangerous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityDeclaration {
    pub shape: CompatibilityClass,
    pub semantic: CompatibilityClass,
    pub temporal: CompatibilityClass,
    pub policy: CompatibilityClass,
    pub api: CompatibilityClass,
    pub storage: CompatibilityClass,
}

impl Default for CompatibilityDeclaration {
    fn default() -> Self {
        Self {
            shape: CompatibilityClass::Additive,
            semantic: CompatibilityClass::Additive,
            temporal: CompatibilityClass::Additive,
            policy: CompatibilityClass::Additive,
            api: CompatibilityClass::Additive,
            storage: CompatibilityClass::Additive,
        }
    }
}

// -------- tests + provenance --------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalTest {
    pub name: String,
    pub kind: String, // "invariant" | "compatibility" | "smoke"
    pub assertion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub author: String,            // "agent://schema-broker-1" or "user://..."
    pub source_prompt: String,     // raw NL the user gave us
    pub model: String,             // e.g. "claude-sonnet-4-5"
    pub generated_at: String,      // RFC3339
    #[serde(default)]
    pub trace_id: Option<String>,
}

// -------- the proposal itself --------

pub type ProposalId = String; // canonical: "prop_<hex16>"

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyChangeProposal {
    pub id: ProposalId,
    pub domain: String,        // e.g. "integrations"
    pub namespace: String,     // e.g. "core.integrations"
    pub change_intent: String, // one-liner the LLM produces
    pub rationale: String,
    pub change: Change,
    pub semantic_contract: SemanticContract,
    pub compatibility: CompatibilityDeclaration,
    pub ownership: Ownership,
    #[serde(default)]
    pub tests: Vec<ProposalTest>,
    pub provenance: Provenance,
}

impl OntologyChangeProposal {
    pub fn target(&self) -> TypeRef {
        self.change.target()
    }

    /// Stable, human-readable signature used by the exact-match reuse layer.
    /// Two proposals with the same signature are considered exact duplicates.
    pub fn signature(&self) -> String {
        match &self.change {
            Change::AddField { type_ref, field } => format!(
                "add_field|{}|{}|{:?}",
                type_ref.fqn(),
                field.name,
                field.proto_type
            ),
            Change::AddRelation { relation } => format!(
                "add_relation|{}|{}|{}",
                relation.from.fqn(),
                relation.name,
                relation.to.fqn()
            ),
            Change::DeprecateField {
                type_ref,
                field_name,
            } => format!("deprecate_field|{}|{}", type_ref.fqn(), field_name),
            Change::ReclassifyField {
                type_ref,
                field_name,
                to,
            } => format!(
                "reclassify_field|{}|{}|{:?}",
                type_ref.fqn(),
                field_name,
                to
            ),
            Change::CreateType { spec } => {
                format!("create_type|{}.{}", spec.namespace, spec.name)
            }
            Change::TightenField {
                type_ref,
                field_name,
                from_required,
                to_required,
            } => format!(
                "tighten_field|{}|{}|{}->{}",
                type_ref.fqn(),
                field_name,
                if *from_required { "required" } else { "optional" },
                if *to_required { "required" } else { "optional" }
            ),
        }
    }
}
