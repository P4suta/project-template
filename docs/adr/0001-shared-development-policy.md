# Shared development policy

Status: Accepted.

Repository templates do not prevent later drift or cover newly created repositories.
Personal skill instructions also require maintained checks for their observable obligations.

Use one public Rust policy command from this repository.
Native dotfiles install an immutable source revision and invoke it from global Git hooks while retaining signing, project checks, and push holds.
Required reusable CI calls the same workflow decisions and validators.
Machine-local skill maintenance remains under `skill-ops`; public CI does not consume the owner's private observation history.

Commit verification reads an immutable Git index tree.
Push verification reads the supplied ref identity and scans the introduced history, including secrets subsequently removed from the candidate tree.
Tracked checker configuration and local action metadata come from that same candidate.
Tool versions are compiled from `tools.json`, and hooks never install missing tools implicitly.

GitHub's `$/` action syntax resolves the workflow's own source revision.
Actionlint 1.7.12 requires a read-only projection to `./` for its older grammar.
Only `uses` references are projected, the parsed result must equal the explicitly expected transformation, and executable workflow files are preserved.
Remove this adapter when a tested upstream release supports the syntax.

Kani verifies the production gate transition, bounded gate sequences, exact commit and installation identity, read-only checkout credential decisions, exact initialization-file retention, monotonic check selection, and proof-property acceptance.
The gate transition provides the inductive rejection invariant beyond the eight-element sequence harness.
It does not prove Git, operating-system I/O, parser libraries, external validators, or arbitrary project implementations.
Native integration tests exercise those boundaries.
Initialization renders into private storage and produces a Git patch with additions and deletions; the final indexed paths and bytes must equal the engine's nonempty rendered-file inventory.
Publish the patch atomically without overwriting an existing artifact, and leave the source repository untouched.
Gitlink pointers retain their own exact identities without claiming to scan external submodule contents.
Previously fetched remote history is trusted when selecting introduced commits for signature and secret checks.
Proof runs use private source snapshots and fresh model directories, require the complete declared harness inventory and reachable contract assertions, and reject an intentional empty-gate claim.
Unreachable guards inside the pinned standard library and Kani model are distinguished from unreachable production contracts.

Skill maintenance uses the existing catalog hashes, actual observations, evidence-backed dispositions, and current-content invalidation.
Instructions requiring judgment or authorization remain instructions; observed loading and successful static checks do not prove that an agent followed every instruction.
