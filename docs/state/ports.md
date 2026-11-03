---
id: "ports"
title: "Allocate named ports"
kind: "statechart"
status: "implemented"
summary: "The locked allocation transaction rereads current rows; a later stale
  save can still overwrite another writer."
parent: "canopyd"
sources: ["src/ports.rs", "src/lib.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
model:
  { "spec": "tla/PortTransaction.tla", "config": "tla/PortTransaction.cfg" }
diagramLinks: {}
diagramDetails:
  [
    {
      "diagram": 0,
      "node": "Risk",
      "section": "The lock does not cover a later save",
    },
    {
      "diagram": 0,
      "node": "Caller",
      "section": "The lock does not cover a later save",
    },
  ]
modelCheck:
  sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
  checker: tlc-1.7.4
  date: 2026-11-03T09:20:27.753Z
  config: tla/PortTransaction.cfg
  modelDigest: af1da191bef2dc07d3fdebef2e359ae5a4c16516a63fbd844d558b9bb3fd6882
  states: 7
  held:
    - UniquePorts
    - RetainCommitted
  broke: []
  unchecked: []
---

# Keep a named URL stable across restarts

A branch's existing named allocation is reused without probing. New names prefer their configured port when usable, otherwise walk a deterministic hash-seeded range past recorded and bound ports. Host probes check IPv4 and IPv6 where supported. A successful probe is not a permanent socket reservation; another unrelated process can bind afterward.

The default registry lives in the common git directory. CANOPYD_PORTS_FILE selects a shared registry, with repository ownership separating rows; CANOPYD_PORT_RANGE changes defaults. Explicit reservations are not bind-probed because a caller may deliberately reserve a port its service already holds.

```mermaid
flowchart TD
    Request[Allocate named ports] --> Lock[Acquire sidecar advisory lock]
    Lock --> Read[Reread current disk rows]
    Read --> Pick[Reuse or pick unclaimed ports]
    Pick --> Save[Atomic replacement of updated rows]
    Save --> Unlock[Release transaction lock]
    Unlock --> Caller[Caller receives allocations]
    Caller --> Risk[Later save may write stale snapshot]
```

## The lock does not cover a later save

Registry::allocate locks, rereads, selects and writes within one transaction. Registry::save separately locks and writes its existing in-memory rows without rereading. Canopy::ports_for calls allocate and then save. A second writer can allocate between those calls, and the first caller's later save can overwrite that newer allocation. The same stale-snapshot concern applies to standalone in-memory reserve/release followed by save.

Do not infer end-to-end concurrent allocation safety from the inner transaction or its model. Serialize higher-level registry writers when using this version, and inspect the retained source probe/evidence for the known lost-update case. Corrupt registries are errors, not silently reset empty tables. A lock timeout is a refusal, not permission to overwrite the registry manually.

A sidecar lock matters because atomic rename replaces the data inode. Atomic replacement prevents partial JSON visibility; it is not a proof of power-loss durability or an external socket lease.

## Model boundary

PortTransaction represents two owners and two ports inside atomic allocate transactions. It checks distinct nonzero ports and retention of earlier committed rows. One control admits an occupied port; another adds the actual problematic later stale snapshot save. The baseline deliberately excludes that later operation and therefore does not qualify Canopy::ports_for or arbitrary Registry::save callers. Lock/file implementation, multi-port batches, legacy empty owner rules, bind races and power loss remain outside this model. Terminal deadlocks are disabled for finite completed states; these are safety properties with no liveness claim. [Evidence](verification.md) retains review and execution results.
