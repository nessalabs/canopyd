---
id: "supervision"
title: "Keep services under one foreground owner"
kind: "statechart"
status: "implemented"
summary: "The run loop owns restart policy and explicit hold controls; health
  failures alone do not restart work."
parent: "canopyd"
sources: ["src/supervise.rs", "src/bin/canopyd.rs", "src/health.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
model: { "spec": "tla/SupervisorHold.tla", "config": "tla/SupervisorHold.cfg" }
diagramLinks: {}
diagramDetails:
  [
    {
      "diagram": 0,
      "node": "Held",
      "section": "Stop deliberately instead of fighting restart policy",
    },
    {
      "diagram": 0,
      "node": "Pending",
      "section": "Health and time are separate policy",
    },
  ]
modelCheck:
  sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
  checker: tlc-1.7.4
  date: 2026-11-03T09:20:29.682Z
  config: tla/SupervisorHold.cfg
  modelDigest: 3c4c425b9bda4c4c86d214b8e9c5592037542f1950d0dbe05eca9116324ef802
  states: 11
  held:
    - NoHeldAutoRestart
    - BackoffElapsed
  broke: []
  unchecked: []
---

# Keep the command running when you want automatic restarts

`run` starts the selected services, polls them and stays alive until its stop condition. The CLI owns signal handling; library callers supply should_stop, a clock, event callback and optional controls. `up` does not leave this loop behind.

```mermaid
stateDiagram-v2
    Pending --> Running: Dependencies settle and launch succeeds
    Running --> Waiting: Exit qualifies for restart
    Waiting --> Running: Backoff deadline reached
    Running --> Exited: Restart policy declines
    Waiting --> Held: Stop control succeeds
    Running --> Held: Stop control succeeds
    Held --> Running: Explicit start or restart succeeds
    Running --> GaveUp: Exit exhausts crash-loop budget
    GaveUp --> Running: Explicit start or restart succeeds
```

## Stop deliberately instead of fighting restart policy

A stop control asks the service owner to stop, then moves it to Held only when the stop result is accepted. Held services do not automatically restart. Explicit start/restart releases that hold and resets restart history. Unknown service names or invalid control lines emit a rejection; a declared service outside the initial selection can join through an explicit request.

The --control input protocol uses start/stop/restart plus service name. It acts through this loop, preserving child ownership and exit-status observation. In CLI control mode, stdin EOF requests global shutdown; merely closing the control pipe is not a way to detach supervision. Stopping the process independently through down can appear as a crash, causing restart:always to bring it back.

## Health and time are separate policy

Automatic restart requires an observed exit, eligible restart policy and remaining crash-loop budget. Backoff is bounded and deadline-based. Health changes are reported without killing/restarting a merely unhealthy process. A dependency with health checking delays its dependent only through a bounded health window; after timeout, or a settled failure/hold, the dependent can start anyway and fail on its own terms. It is not a guarantee every dependency is healthy before all launches.

Normal loop shutdown calls the finish path and stop owner. An early configuration, startup or control error can return after earlier effects, so inspect recorded services instead of assuming every error ran final cleanup. Process and container cleanup limits still apply. There is no guarantee this foreground supervisor survives its own process death.

## Model boundary

SupervisorHold starts with one restart pending at abstract tick 2, ticks 0..3 and an independent accepted hold. It checks automatic restart excludes Held and waits until deadline. Controls bypass hold and time guards. Hold represents successful stop-result acceptance, not a proof physical cleanup succeeded. Explicit start/reset, health, budget arithmetic, dependencies, process failures and multi-service shutdown are not modeled. Terminal deadlocks are disabled for finite completed states; these are safety properties with no liveness claim. [Evidence](verification.md) retains review and execution results.
