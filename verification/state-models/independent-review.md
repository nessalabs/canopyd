# Independent Canopyd reader/source/model review

Source be0750cb553661c8c9021ffff88ad528b26f4fd9;15-page draft. Markdown docs-only tasks recorded first; model and source checked afterward. Agent simulation, not a human study.

## Reader tasks

Root → Worktrees/Setup: new creates a checkout, not the complete environment. Run selected copy/setup/database/service operations explicitly and inspect partial effects. Hook recursion guard prevents duplicate handling, not proof setup happened.
Root → Services/Supervision: up exits without a restart loop. run must remain foreground for restarts. Use its accepted stop control to hold a service; independent down may look like a crash and trigger restart:always.
Root → Identity: missing recorded start time is admitted by current code for positive/live PID, unlike known mismatch. Killed records an escalation attempt even when final wait misses exit; inspect actual owner rather than infer full cleanup.
Root → Ports: inner allocate is locked, but later stale save can overwrite another allocation. Higher-level serialization is needed in this version. Model scope excludes that facade sequence; the real probe supplies separate evidence.
Root → Worktrees: rm can finish with warnings/ignored stop errors, and library remove has less cleanup than CLI. Save needed data before forced removal; no Node-app salvage feature is implied.
Root → Maintenance: installed hook detects new linked checkouts and prints a notice, but does not provision. A successful hook exit is not successful setup.

All six tasks have explicit answers with ownership and supported next actions. No guessed rollback or automatic retry is needed. Implementation review below checks those statements.

## Source, controls and reproduction

Inspected proc identify/stop, Registry allocate/save and Canopy::ports_for composition, supervisor hold/run_with, setup loop, CLI rm and post-checkout. The task answers match source. The recorded-token model deliberately admits absent-token identity as source does; the port model deliberately limits its pass to the inner allocate transaction. These are visible limitations, not idealized fixes.

All4models/8controls reviewed. ProcessAdmission independently varies known/current tokens and target positivity. SupervisorHold separates time from accepted stop, while physical cleanup stays outside it. SetupOrder independently varies settle/failure before second-stage entry. PortTransaction protects atomic allocation only; its extra stale-save transition breaks retention without weakening the invariant. No model claims end-to-end concurrent ports_for safety.

Independently executed python3 scripts/state-docs/probe-stale-save.py using the shared Rust1.99.1 toolchain and a temporary registry. It printed: reproduced: second owner's committed allocation was overwritten by first owner's later save (2 rows -> 1). Inspected probe uses actual Registry APIs with a fake free-port probe, so it tests persistence ordering without claiming a real socket race or live service deployment. This is a confirmed source behavior, not merely a TLC counterexample.

No material documentation/model finding in this sampled scope. Existing runtime limitations and strict-toolchain diagnostics remain open product/check issues and must remain in the PR. Actual full runtime/model suites were run by author, not repeated here. Exclusions: every config/database/container path, actual provider systems, cleanup guarantees and platform portability. Parent visual spot-check follows final author chart rendering separately.
