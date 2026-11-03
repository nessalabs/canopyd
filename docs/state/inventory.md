---
id: "inventory"
title: "Find each command and owner"
kind: "guide"
status: "implemented"
summary: "The map includes current DB/container/control APIs beyond historical milestone headings."
parent: "canopyd"
sources: ["src/lib.rs", "src/bin/canopyd.rs", "Cargo.toml"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Follow the command to its implementation

| Surface                                                                   | Owner                                      | Page and implementation evidence                                                    |
| ------------------------------------------------------------------------- | ------------------------------------------ | ----------------------------------------------------------------------------------- |
| info/list/path/new/rm, repository and branch discovery                    | repo,worktree,paths,git; CLI orchestration | [Checkouts](worktrees.md); discovery/worktree/library/embedder_cli tests            |
| config check/show/path/init/schema, diagnostics/defaults, TS exports      | config parse/load/lint/schema              | [Configuration](configuration.md); config/config_cli/schema/docs tests              |
| env/write/export/reveal, interpolation and layer sources                  | env,lib,CLI                                | [Configuration](configuration.md); ports_env/embedder_cli                           |
| ports allocation/reserve/release/shared registry                          | ports,lib                                  | [Ports](ports.md); unit/ports_env and retained stale-save source probe              |
| copy rules/strategies/include/dryrun and setup labels/conditions/timeouts | copy,setup                                 | [Setup](setup.md); copy_cli/setup_cli                                               |
| up/down/ps, dependency/autostart/health                                   | service,health                             | [Detached services](services.md); service_cli                                       |
| run restart/backoff/budget/controls/events/shutdown                       | supervise,CLI                              | [Foreground owner](supervision.md); supervisor unit and CLI edge tests              |
| saved PID/start-time/launch records, signals, logs                        | proc                                       | [Identity](identity.md), [Logs](logs.md); process/service tests                     |
| host/docker/compose command construction and cleanup                      | container,service                          | [Containers](containers.md); container_cli with scripted engine boundary            |
| db fork/ls/reset/template/drop, SQLite/Postgres/MySQL/Redis               | db and engine submodules                   | [Databases](databases.md); db_cli/db_postgres_cli/db_servers_cli                    |
| doctor/gc findings/collection                                             | doctor                                     | [Maintenance](maintenance.md); doctor unit/CLI tests                                |
| hook install/uninstall/status/post-checkout classification                | hook,CLI                                   | [Maintenance](maintenance.md); hook tests; classifier is not automatic provisioning |
| envelopes, stable codes, human output, stream exceptions                  | wire,error,CLI                             | [Interface](interface.md); contract/errors/human_output/cli_edges                   |

The existing command/config/JSON references remain the exhaustive syntax and field contracts. The README milestone table records prior work; its early milestone headings do not enumerate newer database/container/control APIs. The source inventory above maps those implemented APIs without retroactively asserting a new milestone or whole-suite mutation qualification.

External git, shell, Docker, database servers and filesystem capabilities are boundaries, not code owned by this crate. Tests using fake tools prove the integration contract they exercise. The Canopy application's UI, agent adapters, salvage trash and Node environment registry belong to its separate repository.
