---
id: "interface"
title: "Call the CLI or embed the library"
kind: "contract"
status: "implemented"
summary: "Structured results carry verdicts and warnings; CLI orchestration differs from a low-level library call."
parent: "canopyd"
sources:
  [
    "src/bin/canopyd.rs",
    "src/wire.rs",
    "src/error.rs",
    "src/lib.rs",
    "src/config/schema.rs",
  ]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Parse the result that answers your question

Ordinary --json commands emit the version 1 envelope with command, ok, data/error and warnings. Progress belongs on stderr. A verdict command such as config check or setup can report ok:false with diagnostic data because the operation ran and found an unsuccessful result. CLI usage errors are handled by clap and exit 2 before this envelope path. JSON log-follow emits line events on stdout. JSON run emits live events on stderr and its final envelope on stdout. Parse the selected mode rather than treating every line as an ordinary result envelope.

Read stable error codes, per-service states, per-step outcomes and warnings. A successful top-level result can still contain a failed service or incomplete cleanup. A timeout/error after effects does not automatically make blind retry safe. See the existing [JSON contract](../json-api.md) for complete field and command details.

The Rust Canopy facade discovers the repository and composes public modules. Injected probes, git binary, clock, callbacks and stop controls enable deterministic testing/embedding. Module APIs are public but do not automatically perform the CLI's orchestration: notably library remove does not reproduce the CLI's service/database/port cleanup sequence. A library caller owns that composition.

The crate is Unix-only. The ts feature exports configuration TypeScript types; normal library builds need not include this generator. JSON Schema 2020-12 and types support consumers but do not replace semantic lint, process authorization or compatibility testing with another application. Existing Canopy shared Zod schemas are a separate consumer boundary.
