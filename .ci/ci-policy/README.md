# ci-policy

Shared Rust checks for local Git hooks and reusable GitHub Actions.

```console
ci-policy install-tools
ci-policy activate --expected-revision "$REVISION"
ci-policy doctor --expected-revision "$REVISION"
ci-policy verify-index
ci-policy pre-push origin
ci-policy check .
ci-policy gate --needs "$RESULTS" --require-json '["test", "proofs"]'
ci-policy prove --manifest-path .ci/ci-policy/Cargo.toml
```

`pre-push` reads Git's ref updates from stdin.
Local checks verify staged blobs or the exact pushed revision and include the machine's `skill-ops check`.
`check` verifies committed workflows and actions without accessing personal skill history.
Missing validators and unsuccessful child commands fail the gate.
Native profiles compile the reviewed revision into the binary and activate a receipt for both global hooks.
The hook entry points reject an unidentified binary, altered hooks, repository hook overrides, and pending skill maintenance.
