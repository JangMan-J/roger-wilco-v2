# Coding standards

The core gate is `scripts/check`: fmt, clippy with warnings denied, the test suite,
`scripts/check-harness-free`, and `scripts/check-no-poll`. It names no harness.
`scripts/check-all` runs it, then every `shims/<name>/check` it finds; a shim
brings its own check and the core gate never changes for it. Nothing lands on
main until `scripts/check-all` exits 0. Clippy's `all` group is enabled in `Cargo.toml` `[lints]`; the
gate turns those warnings into errors.

## Review rules

These are INVARIANTS.md read as rules for a diff. The frozen files win over this one.

1. **No loss, no duplicate.** Any path that touches a Mail between Accepted and the
   recipient's Ack keeps the body durable across coordinator restart, daemon restart,
   and recipient SIGKILL, and the Store holds each body once. A change here extends
   `tests/process.rs` and keeps `scripts/acceptance cross-host` green.
2. **Event-driven only.** No fixed-interval scan on the correctness path. Mail,
   receipts, and claim expiry follow committed events or a Register. A sweep may
   exist only as cleanup that can be disabled without losing or duplicating work.
   `scripts/check-no-poll` is the check; a non-correctness timed wait carries
   `// no-poll: <reason>`.
3. **Opaque integers stay opaque.** Nothing in `src/` compares, branches on, ranks,
   looks up by, or names the integers in a Reference or on a session. They pass
   through byte-identical. `scripts/acceptance references` is the check.
4. **No harness in the core.** The core names no harness, reads no harness store, and
   `[dependencies]` stays empty. Only `shims/` may know a harness. The core is every
   file outside `shims/` and `VECTORS/`: source, tests, scripts, deploy units, and
   docs; `scripts/acceptance` alone is exempt, since proofs 2 and 5 run real
   harnesses. `scripts/check-harness-free` is the check.
5. **No workflow.** The core stores and routes; it never decides what work exists or
   who takes it.

## Code

- One binary, standard library only. Prefer a plain function over a trait and a
  small struct over a builder.
- Tests per proof: each KERNEL.md proof has a `scripts/acceptance <name>` subcommand
  that exercises real processes, plus the narrowest unit or process test that would
  catch a regression. Match test depth to the risk the proof names.
- Errors are `io::Result`; a failure names what was expected and what was seen.
- Claim deadlines are wall-clock epoch milliseconds read on the coordinator (`now_ms`),
  stored absolute so they survive a restart; a monotonic clock alone cannot. A forward
  clock step frees keys early and a backward one extends them. Nothing in `src/` reads
  another clock for a claim.
- Keep `src/main.rs` dispatch flat: add an arm, do not restructure.
- Test-only hooks live behind the `test-hooks` feature and never reach a deploy build.
- Vectors, not comments, record what was tried; GLOSSARY.md, not comments, defines terms.
