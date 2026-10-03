# One project verification contract

Status: Accepted.

## Context

Independent hook recipes and CI commands drift, while repeating every check at push makes ordinary development expensive.
A successful check applies only to the exact source, configuration, selected tools, environment, and native target it exercised.

## Decision

Keep project verification in `.ci/verification.json` as explicit argument vectors, required native platforms, input scopes, phases, and bounded execution budgets.
Use mise for pinned tools, a thin `just check` entry point, and maintained Rust commands for procedural checks.
The shared Rust runner validates the actual just syntax tree and every CI job's binding before accepting the contract.
Hosted security, protected approval, aggregate, initialization, and deliberately scheduled campaigns retain separate explicit capabilities.
Unbound jobs, suppressed failures, incomplete native coverage, missing tools, and empty inventories fail the gate.

Commit checks use the exact staged tree with a total budget of thirty seconds.
Development checks have a total budget of thirty minutes; longer campaigns require separate coordination and permission.
Push checks use each supplied commit identity and reuse only matching successful evidence.
Mutable external checks run every time.
Global hooks retain signing, introduced-history secret checks, owner push holds, skill maintenance, and repository hooks.

Run checks in a private checkout whose HEAD identifies the captured source without changing the owner's index or refs.
Bound regular-file snapshots to 64 MiB, reject links and external repositories, and normalize native paths before invoking external tools.
Preserve executable basenames for dispatching proxies such as rustup.
Hash source bytes and executable modes, supporting configuration, explicit tools and binaries, effective allowlisted environment, native platform, and runner identity.
Record success atomically only after the command succeeds within its deadline, its process group is cleaned up, and source and tool identities remain unchanged.
Share dependency build outputs; keep proof models isolated for each exact verification run.

CI invokes the same contract's bound suites on supported native runners.
Cache dependencies without trusting cached workspace executables, save shared caches from main, and require every declared result in one stable aggregate gate.
Retain hosted permissions and release protections; ordinary project verification cannot authorize publication.

## Assurance and trust

Kani imports production decisions for phase selection, exact evidence reuse, cumulative budgets, complete coverage, deadline outcomes, and permitted Cargo operations.
Required proof inventories include reachable covers and a deliberately false claim that must fail with the expected assertion.
Native integration tests exercise staged HEAD identity, pushed revisions, cache invalidation, missing contracts, CI bypass attempts, and descendant cleanup after success, failure, and timeout.
Engine proofs check every 64-byte hash input and portable path validation for arbitrary byte strings up to 32 bytes, with complete unwinding and independent component checks.
Native tests exercise the public path constructor and deserialization boundary on Mac and Windows.
The filesystem, Git, mise, parsers, cryptographic hash implementation, operating system process isolation, tool outputs, and each project's implementation remain external trust boundaries.
Checks must declare their relevant input and environment dependencies; a receipt does not prove undeclared external behavior or malicious subprocess isolation.
