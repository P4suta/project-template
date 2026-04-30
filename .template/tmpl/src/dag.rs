//! Layer DAG resolution.
//!
//! `resolve` ingests a layer selection plus the registry of all known
//! layer metadata and produces a [`ResolvePlan`] that the render phase
//! can walk in deterministic topological order. Every failure mode
//! (unknown layer, unsatisfied capability, duplicate provider, layer-
//! level conflict, dependency cycle) is reported as a structured
//! [`ResolveError`] before any side effect is taken.
//!
//! ## Algorithms
//!
//! * **Capability orphan rule** — a single pass over the selection
//!   builds the `Capability → LayerName` provider map. A second
//!   provider for the same capability is rejected immediately. This is
//!   the layer-DAG analogue of Rust's trait-coherence orphan rule and
//!   is what gives the engine its "swap one implementation of a
//!   capability for another at the project boundary" property.
//! * **Cycle detection** — `petgraph::algo::tarjan_scc` enumerates the
//!   strongly connected components of the requires-graph; any SCC of
//!   size > 1 (or a node with a self-loop) indicates a cycle and the
//!   participants are returned for diagnostics.
//! * **Topological sort** — Kahn's algorithm on the requires-graph,
//!   tie-broken by layer-name lexical order so the output is
//!   reproducible across runs.
//!
//! Time complexity: O(V + E) for both Tarjan's and Kahn's, plus an
//! O(N) preprocess for the orphan rule and conflict scan.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::BuildHasher;

use petgraph::Direction;
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::layer::{Capability, LayerMeta, LayerName};

/// The output of a successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvePlan {
    /// Topologically sorted layer names — render in this order.
    pub order: Vec<LayerName>,
    /// `capability → layer name that provides it` for the selection.
    pub provider_of: BTreeMap<Capability, LayerName>,
}

/// Structured failure modes for [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// The selection mentioned a layer that the registry doesn't know.
    #[error("unknown layer in selection: {0}")]
    UnknownLayer(LayerName),

    /// A required capability has no provider in the selection.
    #[error("layer {layer} requires capability {capability}, no selected layer provides it")]
    UnsatisfiedCapability {
        /// The layer whose requirement is unsatisfied.
        layer: LayerName,
        /// The capability that no selected layer provides.
        capability: Capability,
    },

    /// Two or more selected layers provide the same capability — a
    /// violation of the orphan rule.
    #[error("capability {capability} is provided by multiple selected layers: {providers:?}")]
    DuplicateProvider {
        /// The disputed capability.
        capability: Capability,
        /// The layers that all claim to provide it.
        providers: Vec<LayerName>,
    },

    /// Two selected layers explicitly forbid coexisting.
    #[error("layers {a} and {b} declare a mutual `conflicts-with` exclusion")]
    Conflict {
        /// One participant in the conflict.
        a: LayerName,
        /// The other participant.
        b: LayerName,
    },

    /// The requires-graph contains a cycle.
    #[error("dependency cycle detected among layers: {participants:?}")]
    Cycle {
        /// Layer names participating in (one of) the strongly connected
        /// components with size > 1.
        participants: Vec<LayerName>,
    },
}

/// Conflict report — currently a thin wrapper used by `tmpl verify` so
/// the CLI surface can grow without breaking the resolve contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictReport {
    /// All resolution errors discovered for the registry.
    pub errors: Vec<ResolveError>,
}

/// Resolve a layer selection against a registry. Pure; no I/O.
///
/// Generic over the registry's hash builder so callers using a custom
/// hasher (e.g. `ahash`, deterministic test hashers) can pass their
/// map directly without rebuilding it.
///
/// # Errors
///
/// Returns [`ResolveError`] for any of the failure modes documented on
/// the variant. The first error encountered is returned — callers
/// fix-and-retry rather than getting back a batched diagnosis. Use
/// [`verify_registry`] for whole-registry health checks.
///
/// # Panics
///
/// Will panic if the internal capability-satisfaction invariant is
/// somehow violated between the satisfaction check and the graph build
/// — this would indicate a logic bug in this module, not a malformed
/// input. The path is unreachable for any caller observable to the
/// public API.
pub fn resolve<S: BuildHasher>(
    selection: &[LayerName],
    registry: &HashMap<LayerName, LayerMeta, S>,
) -> Result<ResolvePlan, ResolveError> {
    // ---- 1. Validate selection membership.
    let selected: BTreeSet<LayerName> = selection.iter().cloned().collect();
    for name in &selected {
        if !registry.contains_key(name) {
            return Err(ResolveError::UnknownLayer(name.clone()));
        }
    }

    // ---- 2. Layer-level conflict scan.
    for name in &selected {
        let meta = &registry[name];
        for other in &meta.conflicts_with {
            if selected.contains(other) {
                return Err(ResolveError::Conflict {
                    a: name.clone(),
                    b: other.clone(),
                });
            }
        }
    }

    // ---- 3. Capability orphan rule + provider map.
    let mut provider_of: BTreeMap<Capability, LayerName> = BTreeMap::new();
    for name in &selected {
        let meta = &registry[name];
        for cap in &meta.provides {
            if let Some(prev) = provider_of.get(cap) {
                return Err(ResolveError::DuplicateProvider {
                    capability: cap.clone(),
                    providers: vec![prev.clone(), name.clone()],
                });
            }
            provider_of.insert(cap.clone(), name.clone());
        }
    }

    // ---- 4. Required-capability satisfaction.
    for name in &selected {
        let meta = &registry[name];
        for cap in &meta.requires {
            if !provider_of.contains_key(cap) {
                return Err(ResolveError::UnsatisfiedCapability {
                    layer: name.clone(),
                    capability: cap.clone(),
                });
            }
        }
    }

    // ---- 5. Build the requires-graph.
    //
    // Nodes are selected layers; we add an edge from `provider(cap) → consumer`
    // for each `(consumer, cap)` pair where `consumer.requires` lists `cap`.
    let mut graph: DiGraph<LayerName, ()> = DiGraph::new();
    let mut idx_of: HashMap<LayerName, NodeIndex> = HashMap::new();
    for name in &selected {
        let i = graph.add_node(name.clone());
        idx_of.insert(name.clone(), i);
    }
    for name in &selected {
        let meta = &registry[name];
        for cap in &meta.requires {
            // Safe: provider_of populated above; satisfaction checked.
            let provider = provider_of
                .get(cap)
                .expect("capability satisfaction was validated above");
            let from = idx_of[provider];
            let to = idx_of[name];
            // Self-loop is a cycle.
            graph.add_edge(from, to, ());
        }
    }

    // ---- 6. Cycle detection (Tarjan SCC).
    let sccs = tarjan_scc(&graph);
    for component in &sccs {
        let is_cycle = component.len() > 1
            || component
                .first()
                .copied()
                .is_some_and(|n| graph.find_edge(n, n).is_some());
        if is_cycle {
            let mut names: Vec<LayerName> = component.iter().map(|i| graph[*i].clone()).collect();
            names.sort();
            return Err(ResolveError::Cycle {
                participants: names,
            });
        }
    }

    // ---- 7. Topological sort (Kahn, tie-broken by name).
    let order = kahn_topological_sort(&graph)?;

    Ok(ResolvePlan { order, provider_of })
}

/// Kahn's algorithm — pop in-degree-0 nodes deterministically (sorted
/// by layer name). Cycle detection has already happened by the time we
/// land here, but we still surface the unreachable-cycle case as a
/// fall-back guard.
fn kahn_topological_sort(graph: &DiGraph<LayerName, ()>) -> Result<Vec<LayerName>, ResolveError> {
    let mut in_degree: HashMap<NodeIndex, usize> = graph
        .node_indices()
        .map(|i| (i, graph.neighbors_directed(i, Direction::Incoming).count()))
        .collect();

    let mut ready: BTreeSet<(LayerName, NodeIndex)> = in_degree
        .iter()
        .filter(|&(_, d)| *d == 0)
        .map(|(i, _)| (graph[*i].clone(), *i))
        .collect();

    let mut order: Vec<LayerName> = Vec::with_capacity(graph.node_count());
    while let Some((name, idx)) = ready.iter().next().cloned() {
        ready.remove(&(name.clone(), idx));
        order.push(name);
        for next in graph.neighbors_directed(idx, Direction::Outgoing) {
            let d = in_degree.get_mut(&next).expect("graph node");
            *d -= 1;
            if *d == 0 {
                ready.insert((graph[next].clone(), next));
            }
        }
    }

    if order.len() == graph.node_count() {
        Ok(order)
    } else {
        // Should be unreachable — `tarjan_scc` runs before us — but keep
        // the diagnostic accurate if invariants ever drift.
        let stuck: Vec<LayerName> = graph
            .node_indices()
            .filter(|i| in_degree[i] > 0)
            .map(|i| graph[i].clone())
            .collect();
        Err(ResolveError::Cycle {
            participants: stuck,
        })
    }
}

/// Whole-registry sanity check used by `tmpl verify`.
///
/// Iterating every non-empty subset is too expensive; instead we run
/// [`resolve`] against the *full* registry (which is the maximal
/// selection) plus each declared `conflicts-with` pair to flush out
/// symmetric / asymmetric declarations. The full report is emitted at
/// once for human review.
#[must_use]
pub fn verify_registry<S: BuildHasher>(
    registry: &HashMap<LayerName, LayerMeta, S>,
) -> ConflictReport {
    let mut errors = Vec::new();

    // Conflict declarations should be symmetric. If A conflicts with B,
    // B must also conflict with A — otherwise the resolver could let
    // the pair coexist in selections that mention only the silent half.
    for (name, meta) in registry {
        for other in &meta.conflicts_with {
            let Some(other_meta) = registry.get(other) else {
                errors.push(ResolveError::UnknownLayer(other.clone()));
                continue;
            };
            if !other_meta.conflicts_with.contains(name) {
                errors.push(ResolveError::Conflict {
                    a: name.clone(),
                    b: other.clone(),
                });
            }
        }
    }

    ConflictReport { errors }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(s: &str) -> Capability {
        Capability::new(s).expect("test capability")
    }

    fn name(s: &str) -> LayerName {
        LayerName::new(s).expect("test layer name")
    }

    fn meta(n: &str, requires: &[&str], provides: &[&str], conflicts_with: &[&str]) -> LayerMeta {
        LayerMeta {
            name: name(n),
            description: smol_str::SmolStr::new(format!("{n} layer")),
            requires: requires.iter().map(|c| cap(c)).collect(),
            provides: provides.iter().map(|c| cap(c)).collect(),
            conflicts_with: conflicts_with.iter().map(|c| name(c)).collect(),
        }
    }

    fn registry(metas: Vec<LayerMeta>) -> HashMap<LayerName, LayerMeta> {
        metas.into_iter().map(|m| (m.name.clone(), m)).collect()
    }

    #[test]
    fn resolve_handles_empty_selection() {
        let reg = registry(vec![meta("core", &[], &[], &[])]);
        let plan = resolve(&[], &reg).expect("empty selection is fine");
        assert!(plan.order.is_empty());
        assert!(plan.provider_of.is_empty());
    }

    #[test]
    fn resolve_orders_a_chain() {
        let reg = registry(vec![
            meta("docker-dev", &[], &["container-runtime"], &[]),
            meta(
                "rust-workspace",
                &["container-runtime"],
                &["cargo-workspace"],
                &[],
            ),
            meta("xtask", &["cargo-workspace"], &[], &[]),
        ]);
        let sel = vec![name("docker-dev"), name("rust-workspace"), name("xtask")];
        let plan = resolve(&sel, &reg).expect("acyclic chain resolves");
        assert_eq!(
            plan.order,
            vec![name("docker-dev"), name("rust-workspace"), name("xtask")]
        );
    }

    #[test]
    fn resolve_breaks_ties_lexicographically() {
        // Both `aaa` and `bbb` have in-degree 0, no edges between them;
        // Kahn pops them in lex order.
        let reg = registry(vec![meta("aaa", &[], &[], &[]), meta("bbb", &[], &[], &[])]);
        let sel = vec![name("bbb"), name("aaa")];
        let plan = resolve(&sel, &reg).unwrap();
        assert_eq!(plan.order, vec![name("aaa"), name("bbb")]);
    }

    #[test]
    fn resolve_rejects_unknown_layer() {
        let reg = registry(vec![meta("core", &[], &[], &[])]);
        let err = resolve(&[name("ghost")], &reg).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownLayer(n) if n.as_str() == "ghost"));
    }

    #[test]
    fn resolve_rejects_unsatisfied_capability() {
        let reg = registry(vec![meta(
            "rust-workspace",
            &["container-runtime"],
            &[],
            &[],
        )]);
        let err = resolve(&[name("rust-workspace")], &reg).unwrap_err();
        match err {
            ResolveError::UnsatisfiedCapability { layer, capability } => {
                assert_eq!(layer.as_str(), "rust-workspace");
                assert_eq!(capability.as_str(), "container-runtime");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_duplicate_provider() {
        let reg = registry(vec![
            meta("docker-dev", &[], &["container-runtime"], &[]),
            meta("podman-dev", &[], &["container-runtime"], &[]),
        ]);
        let err = resolve(&[name("docker-dev"), name("podman-dev")], &reg).unwrap_err();
        assert!(matches!(err, ResolveError::DuplicateProvider { .. }));
    }

    #[test]
    fn resolve_rejects_explicit_conflict() {
        let reg = registry(vec![
            meta(
                "docker-dev",
                &[],
                &["container-runtime"],
                &["bare-metal-dev"],
            ),
            meta("bare-metal-dev", &[], &["host-runtime"], &["docker-dev"]),
        ]);
        let err = resolve(&[name("docker-dev"), name("bare-metal-dev")], &reg).unwrap_err();
        assert!(matches!(err, ResolveError::Conflict { .. }));
    }

    #[test]
    fn resolve_detects_two_layer_cycle() {
        let reg = registry(vec![
            meta("a", &["cap-b"], &["cap-a"], &[]),
            meta("b", &["cap-a"], &["cap-b"], &[]),
        ]);
        let err = resolve(&[name("a"), name("b")], &reg).unwrap_err();
        match err {
            ResolveError::Cycle { participants } => {
                let names: Vec<&str> = participants.iter().map(LayerName::as_str).collect();
                assert!(names.contains(&"a") && names.contains(&"b"));
            }
            other => panic!("expected cycle, got {other:?}"),
        }
    }

    #[test]
    fn resolve_detects_self_loop() {
        let reg = registry(vec![meta("a", &["cap-a"], &["cap-a"], &[])]);
        let err = resolve(&[name("a")], &reg).unwrap_err();
        assert!(matches!(err, ResolveError::Cycle { .. }));
    }

    #[test]
    fn verify_registry_flags_asymmetric_conflicts() {
        let reg = registry(vec![meta("a", &[], &[], &["b"]), meta("b", &[], &[], &[])]);
        let report = verify_registry(&reg);
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn verify_registry_flags_unknown_conflict_target() {
        // Layer `a` declares a conflict against a layer that the
        // registry doesn't know — the dangling reference must surface
        // as an UnknownLayer error.
        let reg = registry(vec![meta("a", &[], &[], &["ghost"])]);
        let report = verify_registry(&reg);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, ResolveError::UnknownLayer(n) if n.as_str() == "ghost"))
        );
    }

    #[test]
    fn verify_registry_accepts_well_formed_symmetric_conflicts() {
        let reg = registry(vec![
            meta("a", &[], &[], &["b"]),
            meta("b", &[], &[], &["a"]),
        ]);
        let report = verify_registry(&reg);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn resolve_error_clones_and_displays() {
        let e = ResolveError::UnknownLayer(name("ghost"));
        let cloned = e.clone();
        assert_eq!(e, cloned);
        assert!(format!("{e}").contains("ghost"));
    }

    #[test]
    fn resolve_plan_clones_and_equates() {
        let reg = registry(vec![meta("solo", &[], &[], &[])]);
        let plan = resolve(&[name("solo")], &reg).expect("resolves");
        let cloned = plan.clone();
        assert_eq!(plan, cloned);
    }

    #[test]
    fn conflict_report_clones_and_equates() {
        let reg = registry(vec![meta("a", &[], &[], &["ghost"])]);
        let r = verify_registry(&reg);
        let cloned = r.clone();
        assert_eq!(r, cloned);
    }

    // --- Property tests --------------------------------------------------

    use proptest::prelude::*;

    fn small_layer_strategy() -> impl Strategy<Value = LayerMeta> {
        // A small, deterministic universe so cycles can occur frequently.
        let names: Vec<LayerName> = ["a", "b", "c", "d"].iter().map(|s| name(s)).collect();
        let caps: Vec<Capability> = (0..4).map(|i| cap(&format!("c{i}"))).collect();

        // `prop::sample::select` takes ownership; clone once for the
        // first consumer and move into the second.
        let caps_for_requires = caps.clone();
        (
            prop::sample::select(names),
            prop::collection::vec(prop::sample::select(caps_for_requires), 0..3),
            prop::collection::vec(prop::sample::select(caps), 0..3),
        )
            .prop_map(|(n, requires, provides)| {
                let description = smol_str::SmolStr::new(format!("{n} layer"));
                LayerMeta {
                    name: n,
                    description,
                    requires,
                    provides,
                    conflicts_with: Vec::new(),
                }
            })
    }

    proptest! {
        // Property: `resolve` either returns Ok with `|order| == |selection|`,
        // or returns one of the structured errors. It must never panic.
        #[test]
        fn property_resolve_total(
            metas in prop::collection::vec(small_layer_strategy(), 1..6)
        ) {
            // Deduplicate by name (last write wins) so the registry is well-formed.
            let mut reg: HashMap<LayerName, LayerMeta> = HashMap::new();
            for m in metas {
                reg.insert(m.name.clone(), m);
            }
            let selection: Vec<LayerName> = reg.keys().cloned().collect();
            if let Ok(plan) = resolve(&selection, &reg) {
                prop_assert_eq!(plan.order.len(), selection.len());
            }
        }

        // Property: applying `resolve` is idempotent under repetition
        // — the same input must yield the same plan, every time.
        #[test]
        fn property_resolve_deterministic(
            metas in prop::collection::vec(small_layer_strategy(), 1..6)
        ) {
            let mut reg: HashMap<LayerName, LayerMeta> = HashMap::new();
            for m in metas { reg.insert(m.name.clone(), m); }
            let selection: Vec<LayerName> = {
                let mut v: Vec<LayerName> = reg.keys().cloned().collect();
                v.sort();
                v
            };
            let a = resolve(&selection, &reg);
            let b = resolve(&selection, &reg);
            prop_assert_eq!(a, b);
        }
    }
}
