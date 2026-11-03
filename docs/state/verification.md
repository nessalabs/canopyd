---
id: "verification"
title: "Read checks and known limits"
kind: "guide"
status: "implemented"
summary: "Passing tests and bounded models do not erase reproduced source gaps or a failing strict lint."
parent: "canopyd"
sources:
  ["Cargo.toml", "src/ports.rs", "src/lib.rs", "src/proc.rs", "src/copy.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Separate the evidence by what actually ran

Inspected source be0750cb553661c8c9021ffff88ad528b26f4fd9. The state pages describe this revision, including weaker branches and partial effects rather than an idealized implementation.

`cargo test --locked` passed: 1202 tests, zero failed or ignored in the emitted test summaries. `cargo fmt -- --check` passed. Strict `cargo clippy --locked --all-targets -- -D warnings` failed on the existing needless borrow at src/copy.rs:332 (`map_err(&fail)`) under the isolated Rust 1.99.1 toolchain. No runtime/test changes are included to hide that result. A passing test suite is not a passing lint gate. Warning-free Rustdoc (`RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps`) also failed on existing private/broken intra-doc links, including proc::identify, CommandExt::process_group and supervise::observe_exit; the retained log has the full diagnostics.

## Reproduced port lost update

The isolated source probe calls actual Registry APIs on a temporary file with a deterministic always-free bind probe. Owner A allocates; owner B then allocates; owner A calls save from its older snapshot. Two committed rows become one and owner B is lost. These are the boundaries of allocate followed by save in Canopy::ports_for, not a simulated database implementation.

Run `python3 scripts/state-docs/probe-stale-save.py` with cargo on PATH. It creates/deletes a temporary probe crate/registry and compiles against this checkout; it binds no real ports and changes no production registry. Exit zero means the current bug reproduced, not that concurrency is safe. The [port owner](ports.md) states the caller implication; the [retained source-probe result](../../verification/state-models/stale-save.txt) records the observed lost row.

## Other source-observed limits

Missing recorded process start time is admitted by identify; known mismatch is refused. Killed is returned after escalation attempts even if the final wait did not confirm exit. CLI removal ignores some service-stop errors and post-removal cleanup is best effort. These are source observations, not newly reproduced OS failure experiments. Hook post-checkout detects/notifies a new worktree but does not execute provisioning at this checkpoint.

## Documentation verification

Four finite models cover inner port-allocation transaction, conditional process admission, held restart/deadline and setup sequence. They do not execute Rust. The port model excludes the known later stale-save path; its negative control deliberately adds that path and must break retention. There is no whole-facade concurrency proof.

Set NESSA_DOCS_ROOT to a built canonical nessa-docs checkout, then run `node scripts/state-docs/check.mjs --models --write` and `node scripts/state-docs/probe-models.mjs`. Plain catalog validation does not invoke TLC. Controls mutate in-memory copies while preserving the semantic invariant, with retained counterexamples.

All four baselines passed eight invariants across 38 aggregate states, and eight semantic negative controls each broke its expected rule with none unchecked. [Baseline receipts](../../verification/state-models/baselines.json) and [control counterexamples](../../verification/state-models/negative-controls.json) retain digests/source/checker/date. [Browser verification](../../verification/state-models/browser.json) opened fifteen pages and five diagrams with no page or diagram errors. The tests, strict Clippy and Rustdoc logs are retained as [test evidence](../../verification/state-models/cargo-test.txt), [lint failure](../../verification/state-models/clippy.txt) and [Rustdoc failure](../../verification/state-models/rustdoc.txt). [Independent review](../../verification/state-models/independent-review.md) completed docs-only tasks before source/model inspection and independently reproduced the lost update, with no unresolved material documentation/model finding in scope. Current local macOS execution is not Linux qualification; no live Docker/cloud deployment or full cargo-mutants rerun is claimed. Historical mutation claims in existing docs are not fresh evidence from this task.
