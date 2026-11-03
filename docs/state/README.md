---
id: "canopyd"
title: "Run a worktree environment"
kind: "system"
status: "implemented"
summary: "The Rust CLI and library manage checkout resources; foreground supervision is separate from detached starts."
parent: null
sources: ["src/lib.rs", "src/bin/canopyd.rs", "Cargo.toml"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks:
  {
    "Repo": "worktrees",
    "Config": "configuration",
    "Ports": "ports",
    "Setup": "setup",
    "DB": "databases",
    "Up": "services",
    "Run": "supervision",
    "Identity": "identity",
    "Cleanup": "maintenance",
  }
---

# Give each branch its own environment

canopyd is a Unix Rust library and CLI for git worktrees, ports, setup, services and database forks. Git worktrees are separate checkouts sharing one repository. canopy.yaml describes the environment each checkout should run. Git remains the checkout inventory; files retain the process, port and resource facts git cannot know.

This is the Rust executable used by the separate Canopy application. It is not Canopy's Node daemon, desktop UI or agent service. Most commands run and exit. `up` leaves service processes running; `run` stays in the foreground to supervise and restart them. There is no implicit permanent management daemon after every command.

```mermaid
flowchart TD
    CLI[CLI or Rust caller] --> Repo[Discover git and choose checkout]
    Repo --> Config[Read config and resolve environment]
    Config --> Ports[Allocate named ports]
    Config --> Setup[Copy and run setup]
    Config --> DB[Database forks]
    Config --> Up[Detached service start]
    Config --> Run[Foreground supervision]
    Up --> Identity[Saved process identity]
    Run --> Identity
    Repo --> Cleanup[Remove diagnose and collect debris]
```

Start with the task:

- [Create or remove a checkout](worktrees.md), then [choose configuration](configuration.md).
- [Assign ports](ports.md), [copy files and run setup](setup.md), or [fork a database](databases.md).
- [Start detached services](services.md), [supervise them in the foreground](supervision.md), and [interpret process identity](identity.md).
- [Use Docker/Compose](containers.md), [read logs](logs.md), or [diagnose and install hooks](maintenance.md).
- [Integrate the CLI/library](interface.md), [find feature ownership](inventory.md), and [check evidence and known issues](verification.md).

A successful command can contain failed service statuses or cleanup warnings. A Killed outcome is not independent proof every descendant disappeared. Read each result and its owner before retrying an operation with effects.
