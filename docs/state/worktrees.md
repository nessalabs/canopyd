---
id: "worktrees"
title: "Create or remove a checkout"
kind: "flow"
status: "implemented"
summary: "Checkout creation is distinct from provisioning, and CLI removal adds best-effort cleanup around the library operation."
parent: "canopyd"
sources:
  [
    "src/worktree.rs",
    "src/lib.rs",
    "src/bin/canopyd.rs",
    "src/repo.rs",
    "src/paths.rs",
  ]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Create a branch checkout, then provision it

`new` creates or checks out a branch at the configured path template or explicit path. It refuses an existing path and a branch already checked out. A new branch uses the explicit or configured/default base. This command does not itself execute the whole copy/setup/database/service chain; callers choose those operations afterward. Its own git invocation sets the hook recursion guard so the checkout bridge does not duplicate the work.

`info`, `list` and `path` expose repository discovery, git worktree inventory and calculated paths. Bare repositories and detached checkouts have distinct missing-root/branch cases; commands requiring a branch resolve it explicitly rather than inventing one. A calculated path is not evidence the checkout exists.

## Removal has several effects

The CLI checks removability before stopping resources: main checkout is refused and dirty/untracked work requires force. It then attempts service stop, calls the library checkout removal, releases ports, drops database forks and removes branch state. Pre-removal stop errors are swallowed; post-removal cleanup failures can become warnings. Inspect those results and actual resource owners rather than treating removed checkout as proof all services stopped.

The library `Canopy::remove` delegates to git worktree removal/prune and branch policy. It does not perform all the CLI's surrounding resource cleanup. There is no Node application's salvage-trash feature here. Save required files before forced removal. An error can follow earlier side effects; a successful result distinguishes branch_deleted from checkout removal.

Branch policy never/if-merged/always controls deletion separately. If-merged refusal leaves the branch without turning checkout removal into a failure. Git prune is best effort. A deleted/missing path may still have git administration records to clean.
