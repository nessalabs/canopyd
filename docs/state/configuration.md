---
id: "configuration"
title: "Find config and resolve variables"
kind: "flow"
status: "implemented"
summary: "Specific checkout config wins; generated environment carries real values despite display masking."
parent: "canopyd"
sources:
  [
    "src/config/load.rs",
    "src/config/parse.rs",
    "src/config/lint.rs",
    "src/config/schema.rs",
    "src/env.rs",
    "src/lib.rs",
  ]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Check which file is in effect before changing it

Search order is the current worktree, the main checkout, then user configuration under XDG_CONFIG_HOME/canopyd (or ~/.config/canopyd). CANOPYD_CONFIG replaces that last location; it does not override a committed checkout file. Both canopy.yaml and canopy.yml are recognized. The library caches its resolved configuration in a Canopy instance; use a new instance when a long-lived caller needs a fresh read.

`config path/show/check/init/schema` exposes location, defaults, lint, starter creation and JSON Schema. `check --stdin` inspects an unsaved buffer. Unknown keys become warnings rather than errors, including nested keys. This supports version tolerance but does not mean a misspelled option takes effect. Schema and optional generated TypeScript bindings describe accepted shapes; semantic lint remains separate.

## Resolve once, then pass values deliberately

Environment layers distinguish built-in facts, defaults, top-level config, caller overrides, database URLs and service-specific values. The table records the winning source per variable. Template substitution is single-pass: inserted text is data and is not recursively expanded. Unresolved references remain literal for downstream interpretation, including shell syntax.

Masking is presentation, not secure storage. Export, dotenv output and process environments carry actual values. Generated files are overwritten; put durable changes in configuration instead of hand-editing .env.canopy. Setup/service commands run shell snippets with inherited environment plus resolved overrides, not an argument-only sandbox.

A parsed configuration or known path does not prove external Docker, database or setup dependencies are installed. Validate the relevant owner and inspect per-service/step outcomes when executing it.
