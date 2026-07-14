# Operation Architecture Decisions

These accepted decisions live beside [`operation.rs`](../operation.rs) because
they constrain changes to the operation contract, journal, transfer executor,
and recovery UI:

- [ADR-0001: Durable operation invariants](ADR-0001-durable-operation-invariants.md)
- [ADR-0002: Operation state ownership](ADR-0002-operation-state-ownership.md)
- [ADR-0003: Failure and recovery policy](ADR-0003-failure-and-recovery-policy.md)

Changing an invariant requires changing the relevant ADR, implementation, and
verification fixture in the same review. A new UI path is not an exception to
the operation contract.
