---
id: "maintenance"
title: "Diagnose debris and install the hook bridge"
kind: "flow"
status: "implemented"
summary: "Doctor observes, gc removes selected debris, and opt-in hooks must preserve foreign programs."
parent: "canopyd"
sources: ["src/doctor.rs", "src/hook.rs", "src/bin/canopyd.rs"]
sourceRevision: "be0750cb553661c8c9021ffff88ad528b26f4fd9"
diagramLinks: {}
---

# Inspect before collecting stale state

`doctor` returns findings and severities without changing the state it diagnoses. Warning means debris can be considered for collection; error means a person must resolve facts such as corrupt records, occupied ports or missing git-listed checkouts. Its ok is false even for warnings: read the findings instead of equating that flag with command-execution failure.

`gc` uses git inventory and live process observations to select debris. It preserves live/stale-identity evidence and git-listed state rather than deleting everything old. Oversized logs are truncated in place. A diagnostic snapshot is not a global lock against later git/process changes; port persistence has the [known stale-save limit](ports.md). Inspect results rather than assuming collection qualifies all external resources clean.

## Bridge worktrees created outside the CLI

Hook installation is explicit. A foreign post-checkout program is not overwritten or automatically merged; the command reports a snippet for deliberate integration. Uninstall removes only a recognized owned hook. The installed script exits zero so a provisioning problem does not turn every git checkout into a failing hook.

Classification requires the new-worktree checkout flag, null old ref, linked-worktree .git file and absence of CANOPYD_NO_HOOK. Ordinary branch/file switches and main-clone checkout are ignored. canopyd new sets the guard itself. At this checkpoint the post-checkout handler classifies the trigger and prints a notice (or structured result); it does not invoke copy, setup or the Node daemon. Detection is implemented, automatic provisioning from this hook is not.

A successful git checkout/hook exit does not establish environment provisioning completed. Choose the provisioning commands explicitly and inspect their results.
