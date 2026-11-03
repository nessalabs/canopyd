---
id: "databases"
title: "Create and reset database forks"
kind: "flow"
status: "implemented"
summary: "Fork records and server observations differ; reset preflight reduces but does not remove destructive failure windows."
parent: "canopyd"
sources:
  [
    "src/db.rs",
    "src/db/postgres.rs",
    "src/db/mysql.rs",
    "src/db/redis.rs",
    "src/lib.rs",
    "src/bin/canopyd.rs",
  ]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Give a branch its own development data

`db fork` selects declared SQLite, Postgres, MySQL or Redis databases and creates branch-specific instances from a template, an empty source or another branch. Existing ready instances can be reused. The returned record contains its adapter, connection URL, lineage and observed status. `db ls` probes actual instances; the fast environment path trusts server records while checking local SQLite files, so a URL in env is not a new server-health check.

Template refresh rebuilds the shared seed source separately from existing branch forks. Engine adapters have different file/server/container procedures; Docker and installed engine support remain external dependencies. This atlas does not treat them as one atomic storage backend.

## Reset destroys before replacing

`db reset` validates selection and preflights known dependencies before dropping the selected fork. It then forks a replacement. Later failure can still leave the old data gone; there is no transactional rollback across those steps. Save data you need first, then inspect db ls and errors before retrying. Preflight is useful refusal ordering, not a promise nothing can fail after deletion.

Dropping forks also handles recorded databases removed from configuration when no explicit selection is supplied. It performs removals before writing the staying registry. A failure mid-list can leave some resources already removed and older records still present. Reconcile actual resources with the records instead of assuming all-or-nothing cleanup.

Fork URLs feed environment resolution. Restart or reconnect consumers that hold old connections; generating a new URL does not migrate live connections. File-backed fork deletion and remote server deletion need their respective owner's evidence.
