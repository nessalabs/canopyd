---
id: "containers"
title: "Run Docker and Compose services"
kind: "flow"
status: "implemented"
summary: "Foreground Docker CLIs share host supervision; ordinary stop and destructive purge differ."
parent: "canopyd"
sources: ["src/container.rs", "src/service.rs", "src/bin/canopyd.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Choose a container runtime without changing the supervisor owner

A docker service launches a foreground docker run process with --rm and --init; Compose uses foreground compose up. The Docker CLI is the host process recorded and supervised, and its output reaches the service log. The container is not a detached process with an independent restart loop inside canopyd.

Configuration supplies image, mounts, ports and environment or the Compose file/settings. Deterministic names derive from the worktree and service. Missing Docker or rejected launch can produce a failed service while unrelated host services remain available. A fake-Docker test establishes command construction, not daemon/engine behavior.

Ordinary Compose down handling stops its project; worktree purge uses down -v --remove-orphans and can remove volumes. The CLI's removability check precedes that destructive cleanup, but force authorizes deletion despite dirty work. Preserve database/volume data separately when required.

Container cleanup is a backstop around host signaling and can ignore errors. A stopped host CLI or removed worktree record is not conclusive evidence every container/network/volume vanished. Inspect the named runtime resources after a failed stop or killed supervisor. This feature does not isolate hostile repository commands from the user's Docker authority.
