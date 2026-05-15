//! Three-layer reuse detection.
//!
//! Layer 1 — exact match on `OntologyChangeProposal::signature()`:
//!           identical change against an identical concept = `Duplicate`.
//! Layer 2 — Jaccard similarity over tokenised summaries:
//!           cheap, deterministic, surfaces obvious near-dupes.
//! Layer 3 — embedding cosine. Spec calls for fastembed-rs; see Cargo.toml
//!           for why the hackathon build uses a deterministic hashed-bag-of-
//!           words embedding behind an `Embedder` trait. Swap implementations
//!           by changing `default_embedder()` — call sites are stable.
//! (Bonus) — top-K hits are an LLM-judge candidate set; tagged TODO below.
//!
//! Classification: `New | Reuse | Refinement | Duplicate`.
//!
//! Per spec: ONE embedding call per reuse check. We embed the proposal's
//! summary once, then dot-product against pre-embedded concept cards.

use std::collections::HashSet;

use crate::ast::{Change, OntologyChangeProposal};
use crate::seed::ConceptCard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ReuseClass {
    New,
    Reuse,
    Refinement,
    Duplicate,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReuseHit {
    pub fqn: String,
    pub jaccard: f32,
    pub cosine: f32,
    /// Combined score — what the classifier ranks on. Currently
    /// `0.5 * jaccard + 0.5 * cosine`. Tunable.
    pub score: f32,
    pub layer: String, // "exact" | "jaccard" | "embedding"
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReuseReport {
    pub class: ReuseClass,
    pub top_hits: Vec<ReuseHit>,
    pub explanation: String,
}

// -------- public entry --------

pub fn classify(
    proposal: &OntologyChangeProposal,
    catalog: &[ConceptCard],
) -> ReuseReport {
    // ---------- Layer 1: exact match ----------
    let target_fqn = proposal.target().fqn();
    let proposal_sig = proposal.signature();

    if let Some(hit) = catalog.iter().find(|c| c.fqn == target_fqn) {
        // The proposal targets a concept that already exists. If it's
        // identical-shape AddField against an existing field, mark Duplicate;
        // otherwise it's a Refinement of the existing concept.
        let dup = match &proposal.change {
            Change::AddField { field, .. } => {
                hit.spec.fields.iter().any(|f| f.name == field.name)
            }
            Change::CreateType { .. } => true, // can't re-create an existing type
            _ => false,
        };
        let class = if dup { ReuseClass::Duplicate } else { ReuseClass::Refinement };
        return ReuseReport {
            class,
            top_hits: vec![ReuseHit {
                fqn: hit.fqn.clone(),
                jaccard: 1.0,
                cosine: 1.0,
                score: 1.0,
                layer: "exact".into(),
            }],
            explanation: match class {
                ReuseClass::Duplicate => format!(
                    "Proposal signature `{}` already exists in `{}`.",
                    proposal_sig, hit.fqn
                ),
                ReuseClass::Refinement => format!(
                    "Proposal targets existing concept `{}`; treat as a refinement.",
                    hit.fqn
                ),
                _ => String::new(),
            },
        };
    }

    // ---------- Layer 2 + 3: similarity over the corpus ----------
    let proposal_summary = summarise_proposal(proposal);
    let proposal_tokens = tokenise(&proposal_summary);

    let embedder = default_embedder();
    let proposal_vec = embedder.embed(&proposal_summary); // ONE embedding call.

    let mut hits: Vec<ReuseHit> = catalog
        .iter()
        .map(|c| {
            let card_tokens = tokenise(&c.summary);
            let jaccard = jaccard_similarity(&proposal_tokens, &card_tokens);
            let card_vec = embedder.embed(&c.summary); // pre-embedded in real impl
            let cosine = cosine_similarity(&proposal_vec, &card_vec);
            ReuseHit {
                fqn: c.fqn.clone(),
                jaccard,
                cosine,
                score: 0.5 * jaccard + 0.5 * cosine,
                layer: "embedding".into(),
            }
        })
        .collect();
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let top_k: Vec<ReuseHit> = hits.into_iter().take(3).collect();

    // TODO(bonus): pass `top_k` + proposal to an LLM judge for a final verdict.
    // Not implemented this beat — would be one extra Anthropic call.

    let (class, explanation) = if let Some(top) = top_k.first() {
        if top.score >= 0.85 {
            (
                ReuseClass::Duplicate,
                format!(
                    "Top match `{}` scored {:.2} (jaccard {:.2}, cosine {:.2}) — \
                     looks like a duplicate of an existing concept.",
                    top.fqn, top.score, top.jaccard, top.cosine
                ),
            )
        } else if top.score >= 0.55 {
            (
                ReuseClass::Reuse,
                format!(
                    "Top match `{}` scored {:.2} (jaccard {:.2}, cosine {:.2}) — \
                     consider linking the proposal to that existing concept.",
                    top.fqn, top.score, top.jaccard, top.cosine
                ),
            )
        } else {
            (
                ReuseClass::New,
                format!(
                    "No strong match in catalogue (best: `{}` at {:.2}). Treating as New.",
                    top.fqn, top.score
                ),
            )
        }
    } else {
        (
            ReuseClass::New,
            "Catalogue empty; treating as New.".to_string(),
        )
    };

    ReuseReport {
        class,
        top_hits: top_k,
        explanation,
    }
}

// -------- summary + tokenisation --------

/// Synthesise a short token-rich blurb from the proposal that the
/// similarity layers can compare against catalogue summaries.
pub fn summarise_proposal(p: &OntologyChangeProposal) -> String {
    let mut s = String::new();
    s.push_str(&p.namespace);
    s.push(' ');
    s.push_str(&p.change_intent);
    s.push(' ');
    s.push_str(&p.rationale);
    s.push(' ');
    s.push_str(&p.semantic_contract.meaning_after);
    s.push(' ');
    match &p.change {
        Change::AddField { type_ref, field } => {
            s.push_str(&type_ref.fqn());
            s.push(' ');
            s.push_str(&field.name);
            if let Some(d) = &field.doc {
                s.push(' ');
                s.push_str(d);
            }
        }
        Change::AddRelation { relation } => {
            s.push_str(&relation.from.fqn());
            s.push(' ');
            s.push_str(&relation.name);
            s.push(' ');
            s.push_str(&relation.to.fqn());
        }
        Change::ReclassifyField { type_ref, field_name, .. } => {
            s.push_str(&type_ref.fqn());
            s.push(' ');
            s.push_str(field_name);
        }
        Change::DeprecateField { type_ref, field_name } => {
            s.push_str(&type_ref.fqn());
            s.push(' ');
            s.push_str(field_name);
        }
        Change::CreateType { spec } => {
            s.push_str(&spec.namespace);
            s.push(' ');
            s.push_str(&spec.name);
            if let Some(d) = &spec.doc {
                s.push(' ');
                s.push_str(d);
            }
        }
    }
    s
}

fn tokenise(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3) // drop noise
        .map(|t| t.to_string())
        .collect()
}

fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    inter / union
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}

// -------- embedder trait + offline implementation --------

pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Returns the active embedder. Today: a deterministic hashed-bag-of-words
/// cosine model (256-d, no model file, no network). When fastembed is wired
/// up, return `FastembedEmbedder` instead — call sites in `classify()` don't
/// change.
pub fn default_embedder() -> Box<dyn Embedder> {
    Box::new(HashedBagOfWords::new(256))
}

/// Hashed-bag-of-words "embedding". Tokenises, hashes each token to a bucket,
/// counts, then L2-normalises. Behaves like a tiny `HashingVectorizer`.
/// Not as good as real embeddings, but produces a useful gradient — a proposal
/// about "biometric login" will land closer to `AuthenticationMethod` than to
/// `User` because of shared tokens. Deterministic, offline, zero deps beyond
/// std + sha2 (already in the tree).
pub struct HashedBagOfWords {
    dims: usize,
}

impl HashedBagOfWords {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }

    fn bucket(&self, token: &str) -> usize {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        let d = h.finalize();
        // first 8 bytes → u64 → mod dims
        let mut acc: u64 = 0;
        for b in &d[..8] {
            acc = (acc << 8) | (*b as u64);
        }
        (acc as usize) % self.dims
    }
}

impl Embedder for HashedBagOfWords {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0_f32; self.dims];
        for t in text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() >= 3)
        {
            v[self.bucket(t)] += 1.0;
        }
        // L2 normalise so cosine == dot product.
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            for x in &mut v {
                *x /= n;
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::baseline_concepts;
    use crate::llm::mock_proposal_from_prompt;

    #[test]
    fn biometric_login_matches_auth_method() {
        let prop = mock_proposal_from_prompt(
            "users need biometric login on mobile",
            "user://test",
            "prop_test".into(),
        );
        let report = classify(&prop, &baseline_concepts());
        let top = &report.top_hits[0];
        assert!(
            top.fqn.contains("Authentication") || top.fqn.contains("ProviderAuth"),
            "expected auth-related top hit, got {}",
            top.fqn
        );
    }

    #[test]
    fn empty_string_distance_is_zero() {
        let e = HashedBagOfWords::new(64);
        assert_eq!(cosine_similarity(&e.embed(""), &e.embed("")), 0.0);
    }

    #[test]
    fn novel_prompt_classifies_as_new() {
        // Regression: validator caught that "cosmic ray sensors" was being
        // force-fit onto BankIntegration → Refinement. Heuristic should now
        // emit a CreateType under a fresh `draft.*` namespace, which has no
        // exact match in the catalogue, and the embedding similarity to any
        // existing concept is low → classified as `New`.
        let prop = mock_proposal_from_prompt(
            "cosmic ray sensors",
            "user://test",
            "prop_novel".into(),
        );
        // It must be a CreateType — not an AddField on something pre-existing.
        match &prop.change {
            crate::ast::Change::CreateType { spec } => {
                assert!(
                    spec.namespace.starts_with("draft."),
                    "novel concept should land under draft.* namespace, got {}",
                    spec.namespace
                );
            }
            other => panic!("expected CreateType for novel prompt, got {:?}", other),
        }
        let report = classify(&prop, &baseline_concepts());
        assert_eq!(
            report.class,
            ReuseClass::New,
            "novel prompt should classify as New, got {:?} ({})",
            report.class,
            report.explanation
        );
    }

    #[test]
    fn semantic_contract_invariants_are_populated() {
        // Regression: validator caught that semantic_contract.invariants
        // was missing entirely from the schema. Heuristic author now must
        // emit ≥2 invariants on every proposal, both code paths.
        let add_field_prop = mock_proposal_from_prompt(
            "users need biometric login on mobile",
            "user://test",
            "prop_a".into(),
        );
        assert!(
            add_field_prop.semantic_contract.invariants.len() >= 2,
            "AddField proposal must carry ≥2 invariants, got {}",
            add_field_prop.semantic_contract.invariants.len()
        );

        let create_type_prop = mock_proposal_from_prompt(
            "cosmic ray sensors",
            "user://test",
            "prop_b".into(),
        );
        assert!(
            create_type_prop.semantic_contract.invariants.len() >= 2,
            "CreateType proposal must carry ≥2 invariants, got {}",
            create_type_prop.semantic_contract.invariants.len()
        );
    }

    #[test]
    fn embedding_path_classifies_novel_target() {
        // Hand-build a proposal whose target FQN isn't in the catalogue,
        // so Layer 1 (exact) misses and the embedding+jaccard path runs.
        use crate::ast::{
            Change, CompatibilityDeclaration, Field, OntologyChangeProposal, Ownership,
            PolicyClass, ProtoType, Provenance, SemanticContract, TypeRef,
        };
        let prop = OntologyChangeProposal {
            id: "prop_novel".into(),
            domain: "ledger".into(),
            namespace: "core.ledger".into(),
            change_intent: "Add 'biometric_enrolled' to LedgerSession authentication method"
                .into(),
            rationale: "Track biometric login enrolment per session.".into(),
            change: Change::AddField {
                type_ref: TypeRef {
                    namespace: "core.ledger".into(),
                    name: "LedgerSession".into(),
                },
                field: Field {
                    name: "biometric_enrolled".into(),
                    proto_type: ProtoType::Bool,
                    proto_number: 9,
                    required: false,
                    since_version: 2,
                    deprecated_in: None,
                    classification: PolicyClass::Internal,
                    doc: Some("Biometric login authentication capability flag.".into()),
                },
            },
            semantic_contract: SemanticContract {
                meaning_before: "n/a".into(),
                meaning_after: "Biometric authentication method capability per session.".into(),
                justification: None,
                invariants: vec![
                    "Biometric_enrolled is set only after a successful enrolment ceremony.".into(),
                    "Setting biometric_enrolled does not change LedgerSession's visibility class.".into(),
                ],
            },
            compatibility: CompatibilityDeclaration::default(),
            ownership: Ownership {
                team: "ledger-platform".into(),
                semantic_steward: None,
            },
            tests: vec![],
            provenance: Provenance {
                author: "user://test".into(),
                source_prompt: "test".into(),
                model: "test".into(),
                generated_at: "now".into(),
                trace_id: None,
            },
        };
        let report = classify(&prop, &baseline_concepts());
        // Either Reuse or New is acceptable — the point is that we
        // ranked through the embedding layer (not exact).
        assert_eq!(report.top_hits[0].layer, "embedding");
        // And the closest concept should be auth-related, since the
        // proposal mentions "biometric" and "authentication".
        let top = &report.top_hits[0];
        assert!(
            top.fqn.contains("Authentication") || top.fqn.contains("ProviderAuth"),
            "expected auth-related top hit via embedding, got {}",
            top.fqn
        );
    }
}
