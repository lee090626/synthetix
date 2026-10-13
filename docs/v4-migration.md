# v4 Migration Notes

v4 freezes the scalar proof boundary. It is a contract and artifact migration,
not a promise that every earlier experimental syntax remains supported.

## Result states

Consumers must handle the three result families separately:

- `ContractCheck`: `VALID`, `INVALID`, `UNSATISFIABLE_PRECONDITION`,
  `UNSATISFIABLE_POSTCONDITION`
- `SynthesisResult`: `FOUND`, `NO_CANDIDATE`, `TIMEOUT`, `RESOURCE_LIMIT`
- `ProofResult`: `VERIFIED`, `COUNTEREXAMPLE`, `SOLVER_UNKNOWN`

`AMBIGUOUS_CANDIDATES` and `CALL_PRECONDITION_VIOLATION` are diagnostics. They
are not replacements for a synthesis or proof state.

## Declaration boundary

- `contract` contains a specification and is a synthesis target.
- `fn` contains a user implementation and is verified against its specification.
- `extern fn` is a runtime adapter boundary. HTTP, storage, browser APIs,
  servers, databases, and authentication internals are outside the proof core.

## Artifact compatibility

The report semantics identifier is `v4.1-scalar-baseline`. A
`verification_artifact_hash` or `wasm_hash` identifies artifact integrity only;
neither value is a correctness proof. Correctness requires `ProofResult.VERIFIED`
and the backend conformance checks documented in [the v4 record](history.md#history-roadmap-v4-v7-record).

## Not part of the v4 migration

Division, remainder, right shift, loops, arrays, collections, recursion, WASI,
Node, Edge, and host imports need a later semantics decision. They must not be
silently interpreted using a different backend rule.

## Verification

From a fresh checkout, run the commands in [setup.md](setup.md), then inspect
the negative fixtures in `examples/errors/` and the conformance tests in
`axiom-synth`, `axiom-wasm`, and `axiom-cli`.
