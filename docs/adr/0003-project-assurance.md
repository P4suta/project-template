# One project verification contract

Status: Accepted.

## Context

Independent hook recipes and CI commands drift, while repeating every check at push makes ordinary development expensive.
A successful check applies only to the exact source, configuration, selected tools, environment, and native target it exercised.

## Decision

Install the common verification baseline and automatic project discovery globally through each native dotfiles profile.
A new repository does not need a Lefthook, just, mise, or `.ci/verification.json` file to receive the installed baseline.
Discover native compiler and test operations from supported manifests, include every supported ordinary CI command, and reject uncovered languages, conditional operations, actions, or environment requirements until their shared adapter is implemented.
Common checks inspect the immutable index, and pushes always include every workflow and local action even when the pushed diff changes neither.
Use mise for pinned native tools and maintained Rust commands for discovery, execution, and evidence.
Keep `.ci/verification.json` as an optional explicit contract for project-specific scopes, native targets, proofs, and other behavior the automatic adapter cannot infer.
For an explicit contract, validate the actual just syntax tree and every CI job's binding before accepting its declared suites.
Hosted security, protected approval, aggregate, initialization, and deliberately scheduled campaigns retain separate explicit capabilities.
Unbound jobs, suppressed failures, incomplete native coverage, missing tools, and empty inventories fail the gate.

Commit checks use the exact staged tree with a total budget of thirty seconds.
Development checks have a total budget of thirty minutes; longer campaigns require separate coordination and permission.
Push checks use each supplied commit identity and reuse only matching successful evidence.
Mutable external checks run every time.
Global hooks retain signing, introduced-history secret checks, owner push holds, skill maintenance, and repository hooks.
Repository hooks use the owner's current configuration while fixed verification commands inspect each exact pushed revision.

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
It also checks the required workflow scope and each discovered language's compiler and behavioral operation selection.
Required proof inventories include reachable covers and a deliberately false claim that must fail with the expected assertion.
Native integration tests exercise staged HEAD identity, pushed revisions, cache invalidation, missing contracts, CI bypass attempts, and descendant cleanup after success, failure, and timeout.
Configuration-free Rust fixtures exercise real Clippy, tests, new CI commands, unknown-command rejection, and the absence of repository setup artifacts.
Native Windows preparation starts with an isolated mise installation, uses the same pinned Cargo binary installer as CI, and runs an actual pinned Cargo extension through the production project runner.
Verification tools activate their complete pinned dependency set, including ShellCheck when Actionlint inspects a workflow.
Kani checks this production dependency selection, and native integration tests verify valid workflows and rejected ShellCheck violations with an empty global mise configuration.
Kani also checks hook-root selection by reference identity and NUL-delimited path encoding for arbitrary paths up to 32 bytes.
Integration tests exercise large file inventories, literal filenames, and native executable modes.
Engine proofs check every 64-byte hash input and portable path validation for arbitrary byte strings up to 32 bytes, with complete unwinding and independent component checks.
Native tests exercise the public path constructor and deserialization boundary on Mac and Windows.
The filesystem, Git, mise, parsers, cryptographic hash implementation, operating system process isolation, tool outputs, and each project's implementation remain external trust boundaries.
The checked boundary alternative is the required native integration suite against real Git, mise, Cargo, external validators, and process isolation; a proof over the Rust decision core does not establish those external implementations.
Checks must declare their relevant input and environment dependencies; a receipt does not prove undeclared external behavior or malicious subprocess isolation.
The engine rejects lint `allow` attributes and unused `expect` attributes.
Duplicate dependency exceptions name only the current upstream collisions in bitflags, hashbrown, syn, and unicode-width; new collisions still fail Clippy.
