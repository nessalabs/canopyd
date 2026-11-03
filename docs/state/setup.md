---
id: "setup"
title: "Copy files and run setup steps"
kind: "flow"
status: "implemented"
summary: "Copy can report partial failures; setup stops after a failed step and
  does not undo effects."
parent: "canopyd"
sources: ["src/copy.rs", "src/setup.rs", "src/bin/canopyd.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
model: { "spec": "tla/SetupOrder.tla", "config": "tla/SetupOrder.cfg" }
diagramLinks: {}
modelCheck:
  sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
  checker: tlc-1.7.4
  date: 2026-11-03T09:20:29.066Z
  config: tla/SetupOrder.cfg
  modelDigest: e243f3dbcc8c4581669a30caa5f6e1e34cf493e53453e8d9ae128d62b7828fe1
  states: 4
  held:
    - FirstSettledBeforeSecond
    - FailureStopsSequence
  broke: []
  unchecked: []
---

# Carry local files, then install dependencies

`copy` applies configured rules to eligible source files, with .canopyinclude narrowing what is carried. Results distinguish cloned, copied, symlinked, skipped and planned paths. Existing destination content is not overwritten. A clone strategy may fall back to copying, so inspect the actual result rather than assuming filesystem copy-on-write support or a fixed speedup. Per-path failures can be reported while other paths continue.

`setup` runs selected labeled shell steps in order and streams stdout/stderr separately. `if_changed` compares file bytes against the source, not checkout mtimes; force bypasses that skip. An unknown selected label is refused before reporting an empty success. A failing step stops later steps, preserves a bounded diagnostic tail, and marks the outcome unsuccessful.

## Failure is not rollback

Setup can change files or remote systems before returning failure. A timeout sends termination to the step's process group, then escalates after a grace period; it does not reverse completed writes or hostile processes that escaped that group. Inspect logs and partial effects before rerunning a non-idempotent step. A spawn/configuration error differs from a completed command with nonzero exit, and both differ from a skipped step.

Copy dry-run plans writes; it is not a test that later execution will have the same permissions or free space. Shell cwd may be created for a step. The tool runs with the user's authority, so repository setup is executable configuration rather than safe untrusted input.

## Model boundary

SetupOrder models two selected sequential steps and independent success/failure of the first. It checks the second starts only after the first settles and does not continue after failure. Controls skip settling or ignore failure. It excludes skipped labels, file comparison correctness, timeout/group cleanup and arbitrary shell effects; no rollback or idempotency property is claimed. Terminal deadlocks are disabled for finite completed states; these are safety properties with no liveness claim. [Evidence](verification.md) retains review and execution results.
