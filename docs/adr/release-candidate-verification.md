---
date: 2026-10-25
---

# Complete verification for release candidates

## Status

Accepted.

## Context

Pull requests need the existing native tests, lint, dependency audit, coverage, template checks, and workflow policy checks.
Production proofs can reach their configured stop without establishing a property.
Running the complete proof inventory on every routine change consumes hosted resources and blocks unrelated maintenance with an inconclusive verdict.

## Decision

Commit and development checks keep their existing [entrypoints](../../Justfile) and commands.
The three production proof checks belong to a typed candidate phase.
Candidate checks require an exact committed revision and preserve every existing proof command, platform, symbolic domain, assertion, cover, solver, unwind bound, and configured stop in the [verification contract](../../.ci/verification.json).

A manually requested [controller](../../.github/workflows/candidate-verification.yml) runs from the default branch and validates a full candidate commit containing that branch.
The controller passes the validated commit to the ordinary three-platform workflow and the existing Linux and macOS proof matrix.
Qualification requires successful complete summaries from both workflows and matching source identities from every role.
Missing, failed, skipped, cancelled, malformed, or mismatched results can't qualify a candidate.

The repository has no release publisher.
Candidate qualification provides evidence for one exact commit and doesn't enforce publication performed elsewhere.
A future publisher must require successful qualification of its exact source before publication.

## Consequences

Routine checks keep their required summary without starting production proofs.
CodeQL retains pull request and main scans without a weekly duplicate scan.
Proofs that stop remain unverified and prevent candidate qualification.
The phase proof harnesses cover the extended selection rules, but their verification remains unrun until an actual candidate campaign supplies a verdict.

Keeping proofs in the routine phase would leave ordinary maintenance dependent on long proof campaigns.
A standalone manual proof workflow would lack the complete ordinary results and common source identity required for candidate qualification.
