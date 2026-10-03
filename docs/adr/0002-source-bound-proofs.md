# Source-bound module proofs

Status: Accepted

## Context

Project-specific proof commands must verify the production implementation and reject missing, unreachable, or stale evidence.
Copying the algorithm into a separate model weakens that connection, while sharing generated models across different checkouts can invalidate results.

## Decision

Provide a Rust command that snapshots one self-contained production module byte-for-byte and imports it into a fixed wrapper.
Require Kani 0.68.0, an exact nonempty inventory of positive harnesses and one rejecting counterexample, reachable assertions and cover witnesses, and fresh model directories for both outcomes.
Reject external source modules, include and environment macros, custom macro expansion, import aliases, and unsupported attributes through Rust syntax analysis.
Check that the original production source remains unchanged after verification.

The source-admission decision uses an exhaustive enum with a Kani proof.
The parser, verifier, standard library, and native filesystem remain explicit trust boundaries.
Projects retain behavioral and native integration tests for effects outside the proved module.

## Consequences

Repositories can share the verification driver while owning their production contracts, harnesses, bounds, and intentional false claim.
A timeout, unsupported input, inventory mismatch, tool error, unreachable contract, or unexpected counterexample outcome fails the gate.
Modules with external dependencies require a separately bound Cargo proof command rather than silently widening this module command's inputs.
