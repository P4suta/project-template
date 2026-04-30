<!-- Title: Conventional Commit subject. The squashed commit message
     uses this title verbatim. -->

## Summary

<!-- What changes, and why. Link to the issue(s) being addressed. -->

## Scope

- [ ] Engine change (`.template/tmpl/`)
- [ ] Layer change (`.template/layers/<name>/`)
- [ ] Template-side documentation, CI, or hygiene

## Verification

<!-- For engine changes: which test(s) cover the change. For layer
     changes: which `insta` snapshot was updated and why. -->

- [ ] `just lint`
- [ ] `just test`
- [ ] `just verify-template`
- [ ] `just coverage` (if behaviour changed)

## ADR

<!-- Link to a new or existing ADR if this PR makes an architectural
     decision (engine module boundary, layer DAG shape, capability
     model, …). -->
