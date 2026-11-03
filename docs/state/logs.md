---
id: "logs"
title: "Read logs with an explicit cursor"
kind: "flow"
status: "implemented"
summary: "Log pages use byte offsets and complete lines; truncation and process lifetime are separate."
parent: "canopyd"
sources: ["src/service.rs", "src/proc.rs", "src/bin/canopyd.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Follow output without owning the service

Service stdout and stderr append to a file so the service does not depend on a reader draining a pipe. `logs` can tail or follow; library page/follow_from APIs carry byte offsets. The reader reports complete lines and keeps an unfinished trailing line for a later read. A cursor is a file position, not a durable globally unique event ID.

Catch-up reads do not intentionally sleep between nonempty pages; an idle follow polls. File truncation can reset the reader's position. Garbage collection truncates oversized logs in place rather than deleting an open file, but earlier content is then gone. Keep external archival when complete historical output matters.

Ending a log follower does not stop the service, and a quiet log does not prove it exited. Use ps and the process/health result separately. Log contents come from commands and may contain credentials; environment masking does not sanitize arbitrary child output.

The CLI's JSON interfaces distinguish ordinary one-result commands and explicit event streams. Integrators should follow [the interface contract](interface.md), not parse human progress text.
