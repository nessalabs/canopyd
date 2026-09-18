# Command reference

Everything `canopyd` currently does. Commands marked *planned* are named here because their
error codes and output shapes are already reserved — the surface does not shift under you when a
milestone lands.

## Global options

| Flag | Effect |
|---|---|
| `--json` | Print one JSON envelope on stdout. See [the JSON interface](json-api.md). |
| `-C <path>` | Run as if started in `<path>`. |
| `-y`, `--yes` | Do not prompt. *(planned — nothing prompts yet)* |
| `-q`, `--quiet` | Suppress progress on stderr. |
| `-v` | More detail on stderr; repeatable. |
| `-h`, `--help` | Help for any command. |
| `-V`, `--version` | Version. |

A bad flag exits `2` without printing an envelope.

## `canopyd info`

What repository this is and where its pieces are.

```console
$ canopyd info
name        canopy
root        /Users/me/dev/canopy
common dir  /Users/me/dev/canopy/.git
git dir     /Users/me/dev/canopy/.git
worktrees   6
```

`common dir` is the same from every worktree in the repository, which is what makes it the thing
to key state on. Run it inside a linked worktree and `root` and `git dir` change while
`common dir` does not.

For a bare repository `root` is omitted entirely.

## `canopyd list`

Every worktree git knows about, main checkout first.

```console
$ canopyd list
main                     /Users/me/dev/canopy
feat/login               /Users/me/code/canopy.feat-login
(detached a950fe09)      /Users/me/code/canopy.spike
```

Detached worktrees show a shortened head; a bare entry shows `(bare)`.

```console
$ canopyd list --json | jq -r '.data[] | select(.branch) | .branch'
```

## `canopyd config check`

Validate the `canopy.yaml` in effect.

```console
$ canopyd config check
ok: 0 error(s), 0 warning(s)
```

```console
$ canopyd config check
/Users/me/app/canopy.yaml: warning: services.app.helth: unknown key `helth` — ignored
/Users/me/app/canopy.yaml: error: services.app.run: unknown port `${ports.wbe}`
invalid: 1 error(s), 1 warning(s)
```

Every diagnostic names the key it is about, as a dotted path.

A **parse** error also carries a position, printed as `path:line:column:` so editors and
terminals can jump straight to it:

```console
$ canopyd config check
/Users/me/app/canopy.yaml:1:10: error: <root>: invalid u32
invalid: 1 error(s), 0 warning(s)
```

Lint diagnostics — unknown ports, dependency cycles, everything semantic — carry the path but
not yet a position; spans for those land in M2b.

**Exits `1` when the config is invalid**, so CI needs no parsing:

```bash
canopyd config check || exit 1
```

### `--stdin`

Validate text on stdin and touch no file — for an editor checking a buffer that has not been
saved.

```bash
cat draft.yaml | canopyd config check --stdin --json
```

## `canopyd config show`

The config with every default filled in. What a service actually gets, not what the file says.

```console
$ canopyd config show --json | jq '.data.services.web'
{
  "run": "npx vite --host 127.0.0.1 --port ${ports.web} --strictPort",
  "cwd": "apps/web",
  "env": {},
  "ports": ["web"],
  "health": { "http": "…", "interval": "3s", "timeout": "3s", "retries": 10, "start_period": "10s" },
  "depends_on": ["daemon"],
  "restart": "on-failure",
  "autostart": true,
  "stop_signal": "SIGTERM",
  "stop_timeout": "10s"
}
```

Refuses an invalid config rather than returning part of one.

## `canopyd config path`

Which file is in effect, and which of the three search locations it came from.

```console
$ canopyd config path
/Users/me/dev/canopy/canopy.yaml  (this worktree)
```

Worth asking before editing. See [where the file is found](configuration.md#where-the-file-is-found).

## `canopyd config schema`

The JSON Schema for `canopy.yaml`, generated from the Rust types so it cannot drift from the
parser.

```bash
canopyd config schema > canopy.schema.json
```

Point an editor at it and you get completion and inline validation for free:

```yaml
# yaml-language-server: $schema=./canopy.schema.json
version: 1
```

It is also how a program in any language validates a config without running `canopyd`. Two
deliberate limits: the schema cannot express what the linter checks (`depends_on` cycles,
exactly-one-probe in `health`, unresolvable `${ports.x}`), and it leaves `additionalProperties`
open because unknown keys are warnings here, not errors. `config check` remains the authority.

Generated TypeScript bindings ship alongside it in `types/`, so a consumer stops hand-writing
types that have to match a Rust struct by eyeball.

## `canopyd config init`

Print a starter `canopy.yaml` on stdout. **Writes nothing** — redirect it yourself:

```bash
canopyd config init > canopy.yaml
```

Printing rather than writing is the difference between a command you can pipe and one that
clobbers the file you were editing. The starter passes `config check` with no warnings.

The only command that works outside a git repository.

## `canopyd ports [<branch>]`

The ports allocated to a branch, allocating them on first ask. Idempotent — the numbers do not
move once a branch has them.

```console
$ canopyd ports
api          14100
web          11189
```

Allocation starts from a hash of the branch and walks on, skipping anything the registry holds
and anything that fails a bind test on **both** `127.0.0.1` and `::1`. A port free on one stack
and busy on the other is a failure that looks like a broken service.

`ports.<name>.preferred` is honoured when it is free *and inside the range*; one outside is
ignored rather than silently widening the range. `--all` prints the whole registry — that is
what another program reads instead of keeping its own table. `--release` hands a branch's ports
back to the pool.

The registry lives in `.git/canopy/ports.json`, so every worktree of the repository sees one
table.

## `canopyd env [<branch>]`

The resolved environment: Canopy's own facts, then `defaults.env`, then `env:`, last wins.

```console
$ canopyd env
CANOPY_BRANCH=main
CANOPY_PORT_API=14100
CANOPY_PORT_WEB=11189
CANOPY_PROJECT=demo
CANOPY_WORKTREE=demo
CANOPY_WORKTREE_PATH=/Users/me/code/demo
PUBLIC_URL=http://127.0.0.1:11189
```

Output is sorted and deterministic: writing twice produces byte-identical files, so it never
shows up as a spurious diff.

- `--export` prints `export K='v'` lines for `eval "$(canopyd env --export)"`
- `--write` writes the file named by `env_file:` into the worktree
- `--json` **masks** values that look like secrets; the file and `--export` keep the real ones,
  because masking is presentation, not storage
- `--json --reveal` prints the real values, for an embedder that stores the table and masks it
  itself
- `--env KEY=VALUE` (repeatable) adds the caller's own variables as the last layer, tagged
  `override`. `up`, `down`, `ps`, `run` and `setup` take the same flag, so a value this tool
  cannot resolve — a database URL from a fork somebody else made — reaches every service, its
  health checks and the written file alike
- `--env KEY` with no value takes it from the environment `canopyd` was started with, like
  `docker run -e KEY`. That is how to pass a secret: arguments are visible to every user on
  the machine through `ps`, an environment is not

Values are single-quoted when they need it. Double quotes would not do: `sh` still expands `$`,
backticks and `\` inside them, so a password containing `$` would not survive a round trip
through `. ./.env.canopy`.

## `canopyd up [<branch>]`

Start the worktree's services, in dependency order.

```console
$ canopyd up feat/login
api              running    48210
web              running    48214
```

`up` spawns and **exits**; the services keep running. There is no daemon holding them — each is
its own process group with its output redirected to a file, so nothing needs to stay alive to
pump a pipe.

It waits for each service's health check by default, so a green result means the thing actually
serves. `--no-wait` returns as soon as each process is spawned; a service with a health check
then reports `starting` rather than `running`, because alive is not the same as serving.

A service that dies immediately is reported `exited`, not `running` — there is a short grace
period after spawn precisely to catch that. A second `up` is a no-op that returns the existing
pids, so a race loses gracefully instead of double-starting. `--only <name>` starts a subset,
and `autostart: false` keeps a service registered but unstarted unless you name it.

### Containers

`runtime: docker` and `compose:` services are started the same way and supervised by the same
code, because nothing is detached: a docker service is `docker run --rm --init …` **attached**,
and a compose service is `docker compose up` attached. The `docker` CLI is then an ordinary
process — its output is the service's log, its exit code is the service's, and SIGTERM to its
group is forwarded to the container — so health checks, `restart:`, `logs` and `down` work
without knowing a container is involved.

- the container is `canopy-<worktree>-<service>`, labelled `canopy=true`, on the shared `canopy`
  network, with the worktree mounted at `docker.workdir` (`/workspace`) and each of the
  service's ports published under the same number on both sides
- environment values reach it as `-e KEY`, taken from the CLI's own environment, so a secret is
  never in an argument list `ps` would show
- `docker.dockerfile` is built once under a tag that is the Dockerfile's content hash, and the
  build's output goes into the service's log
- `--init` is what lets `down` stop it promptly: a shell that is PID 1 ignores SIGTERM
- every stop is followed by `docker rm -f` (or `compose stop`), and every start preceded by one,
  so a container left by a crash never blocks the next start. `rm` takes a compose stack's
  volumes with it (`compose down -v`)
- no docker, or a daemon that is not running, is a `failed` service carrying docker's own words;
  the host services beside it start as usual

`CANOPYD_DOCKER` names the binary, for `podman` or a test.

## `canopyd down [<branch>]`

Stop them, in reverse dependency order: `stop_signal` (default SIGTERM) to the whole process
group, escalating to SIGKILL after `stop_timeout`.

The group, not the process, is the point — `run: npm start` that backgrounds a watcher would
otherwise leave the watcher running. A record whose pid has been reused by an unrelated process
is **refused**, never killed.

## `canopyd ps [<branch>]`

What is running, re-verified from the OS rather than trusted from the record file.

```console
$ canopyd ps feat/login --json | jq '.data[] | {name, state, pid, health}'
{ "name": "web", "state": "running", "pid": 48214, "health": { "status": "healthy" } }
```

## `canopyd logs <service> [<branch>]`

stdout and stderr, interleaved in one file — which is what you want when reading why something
died.

- `-n <count>` how many lines to show (default 200)
- `-f` keep printing as new lines arrive
- `--offsets` with `--json`: `data` becomes `{ lines: [{ offset, text }], next_offset,
  truncated }` instead of a list of strings. `offset` is the byte a line starts at
- `--since <offset>` reads from an offset an earlier read returned as `next_offset`, so a reader
  that went away comes back for exactly what it missed. `truncated: true` means the log got
  shorter in the meantime (`gc` does that) and the read started over from the beginning
- `-f --json` is a stream rather than an envelope: one `{"event":"line","offset":…,"text":…}`
  per line until the caller goes away, and `{"event":"reset","next_offset":0}` if the log is
  truncated underneath it

## `canopyd copy [<branch>]`

Carry the gitignored files a worktree needs — the `.env` your app reads, and optionally the
dependency directories that cost minutes to rebuild.

```console
$ canopyd copy feat/login
copied     .env (41 bytes, 0ms)
cloned     node_modules/react/index.js (6212 bytes, 1ms)
```

Candidates come from git, so only **gitignored** files are eligible and a tracked file is never
touched — it arrived with the checkout and reflects what the branch actually says.

`strategy: clone` uses a copy-on-write clone where the filesystem supports it. The report
distinguishes `cloned` from `copied` so a silent fallback is visible rather than just slow.

Nothing is overwritten: an existing file, directory or symlink at the target is reported
`skipped`. One unreadable source does not abort the rest — it lands in `failures` alongside the
paths that worked.

- `--from <path>` copies from another checkout instead of the main one
- `--dry-run` reports the plan and writes nothing

## `canopyd setup [<branch>]`

Run the worktree's `setup:` steps, in order, with the resolved environment.

```console
$ canopyd setup feat/login
skipped  install — lockfile.txt unchanged
ran      build (1240ms)
```

Step output streams to **stderr** as it happens — a four-minute `npm ci` that prints nothing
until it finishes looks like a hang — so stdout stays clean for `--json`. `--quiet` suppresses
the stream without suppressing the result.

`if_changed` is what makes this cheap to re-run: a step is skipped when the files it names are
byte-for-byte identical to the main checkout's. Content, never mtime — every file in a fresh
worktree has a new mtime, which would make the check useless.

- `--force` runs every step anyway
- `--only <name>` runs just that step, repeatable. An unknown name is an **error**, not a silent
  clean run — a typo that reports instant success is the failure mode that costs an hour
- `--timeout 5m` gives up on any single step, killing its whole process group

A failing step stops the run; later steps do not run. Exit is `1`, and `--json` reports a
verdict: `ok: false` with `setup_failed`, while `data` still carries every step with the tail of
the failing one's output.

## `canopyd run [<branch>]`

Keeps the services alive in the foreground until Ctrl-C. This is the **only** place restart
policy and continuous polling exist.

```console
$ canopyd run
api started, pid 48210
web started, pid 48214
api exited with code 1
api restarting in 1000ms, attempt 1
api started, pid 48260
^C
web stopped
api stopped
stopped after 1 restart(s)
```

Separate from `up` on purpose: an agent or a Makefile wants fire-and-forget, and a human in a
terminal or a CI job wants a process to babysit. Conflating the two is what forces a daemon.

- **restart policy** comes from each service's `restart:` — `never`, `on-failure`, `always`
- **backoff** starts at `--backoff` (1s) and doubles to `--backoff-max` (30s); a service that
  outlives the cap has demonstrably started, so its next failure begins at the base again
- **crash-loop budget**: `--restarts` (5) inside `--restart-window` (60s), then it gives up.
  Without that, one broken command pins a core forever
- `--no-restart` reports exits without acting on them

**A dependent waits for its dependency to be serving.** A service whose `depends_on` names one
with a `health:` block is not started until that check passes — which is what lets a web server
read the token its API writes at boot. It does not wait forever: once the dependency has had its
whole health window (`start_period` plus `retries` intervals) and five seconds more, the
dependent starts anyway, and the failing check stays reported on the dependency. A dependency
that has exited, given up or been stopped holds nobody up, and a `start` request never waits.

**A failing health check is reported, never acted on.** A subtly wrong check — a `localhost`
that resolves to `::1` first on macOS — would otherwise become an infinite kill loop against a
service that is working perfectly.

### `--control`: steering one service

`run --control` reads requests from stdin, one per line: `start <service>`, `stop <service>`,
`restart <service>`. This is how a program that embeds `run` stops a single service — calling
`down` from another process would look like a crash, and `restart: always` would undo it.

- a requested **stop holds** the service: the restart policy does not apply until a `start` or
  `restart` names it again, and it reports `stopped` once, not again at shutdown
- a requested **start** wipes the service's restart history, so one that gave up gets a fresh
  budget; starting a service that is already running does nothing
- a service outside `--only` **joins** when it is named, and is stopped with everything else
- a request that cannot be honoured is a `rejected` event carrying the request and the reason;
  the run carries on
- **end of input stops everything**, exactly like Ctrl-C — so an embedder that dies takes its
  services with it instead of orphaning them

Ctrl-C flips a flag the loop checks rather than killing the supervisor where it stands, because
services would otherwise be left running with nothing watching them. Shutdown stops everything
in reverse dependency order, and a backgrounded grandchild does not survive it — there is a test
that asserts exactly that.

Events go to stderr as they happen (one JSON object per line under `--json`), so stdout stays
the final envelope.

## `canopyd db`

Database forks: a private copy of each database `canopy.yaml` declares, per worktree. See
[Databases](configuration.md#databases) for what a fork is and how its URL reaches a service.

```console
$ canopyd db fork feat/login
main             sqlite   ready    file:/…/state/feat-login/db/main.db
```

- `db fork [<branch>]` forks what does not have a fork yet. One that exists is **left alone** —
  the data in it is somebody's work. `--only <name>` restricts it; `--from` says what a new fork
  starts as: `template` (the seed, the default), `empty`, or a **branch name** to copy that
  worktree's fork
- `db ls [<branch>]` lists forks, re-checked against the disk: one whose file is gone is `missing`
- `db reset <name> [<branch>]` throws a fork away and makes it again; takes `--from` too
- `db template [<branch>]` rebuilds seed templates from their seeds, all or `--only`. Forks that
  exist keep their data; the next one is made from the new template. SQLite has none to rebuild
- `db drop [<branch>]` removes forks, all or `--only`. `rm` does this for the whole worktree,
  including dropping a fork that lives on a server
- `db reset` checks that a new fork *can* be made before it drops the old one, so docker being
  off costs you nothing

An adapter this version cannot drive is an error (`db_unsupported`) for the **whole** call, and
nothing is forked: half a set of forks leaves a worktree pointed at a shared database without
saying so.

## `canopyd doctor`

What is wrong with this repository's canopyd state. Read-only — it reports, it never fixes.

```console
$ canopyd doctor
warning  port_row_stale             the port registry holds 1 port(s) for feat/old, which has no worktree

nothing here needs a person: `canopyd gc` sweeps all of it
```

| Check | Is |
|---|---|
| `port_row_stale` | the registry holds ports for a branch with no worktree |
| `port_duplicate` | two branches hold the same number (the registry is corrupt) |
| `port_foreign` | a registered port is held by something we did not start |
| `port_registry_unreadable` | the registry exists and cannot be parsed |
| `state_dir_orphan` | state for a worktree git no longer lists |
| `service_record_orphan` | a record whose process is dead, or whose pid was reused |
| `service_record_unreadable` | a record that cannot be parsed |
| `log_oversized` | a log past the 8 MiB cap |
| `worktree_missing` | git lists a worktree that is not on disk |
| `reflink_unsupported` | `strategy: clone` will silently degrade to a byte copy here |

**Severity decides the exit code.** A *warning* is debris `gc` sweeps as a matter of course and
exits `0` — failing CI over it would make `doctor` useless in the place you most want it. An
*error* is something only a person can settle, exits `1`, and reports `repository_unhealthy`.

## `canopyd gc`

Sweeps what `doctor` reports as debris: stale registry rows, dead service records, state for
worktrees git no longer lists, and logs over the cap (truncated, never deleted).

It removes only what it can prove is dead. A record whose process is alive, a state directory
for a worktree git still lists, and anything it could not parse are all left exactly where they
are — an unreadable record is precisely the one not to delete on a guess.

## `canopyd hook install`

Installs a `post-checkout` hook so a worktree created by plain `git worktree add` is noticed.

```console
$ canopyd hook install
installed /Users/me/dev/app/.git/hooks/post-checkout
```

Opt-in, never automatic: installing a hook into someone's repository as a side effect of
creating a worktree is a surprise, and the hooks directory is **shared by every linked
worktree**, so one bad hook breaks them all.

Which is why the generated script always exits `0`, and calls the binary rather than `exec`ing
it — with `exec`, a missing binary would leave `/bin/sh` exiting 127 and break every checkout in
the repository. git cannot abort a checkout on our say-so anyway.

It fires on a worktree add and on nothing else. All four of these must hold:

1. `$3 == 1` — a branch checkout, not a file checkout
2. `$1` is the null ref — this is what separates `worktree add` from an ordinary `git checkout`
3. `.git` in the new directory is a **file**, not a directory — separates it from `git clone`
4. `CANOPYD_NO_HOOK` is unset — `canopyd new` sets it on its own `git worktree add`, so the
   hook cannot recurse

`hook uninstall` removes only ours and leaves a foreign hook alone; `hook status` says whether
it is installed. Installing over a hook that is not ours is refused, with the snippet to paste.

## Planned

| Command | Milestone |
|---|---|
| `config schema`, `config set` | M2b |
| `run` (foreground supervisor with restart policy) | M10 |

## Exit codes

`0` success · `1` the operation failed · `2` usage error · `3` another process holds the lock
(retryable).

Full list of error codes: [the JSON interface](json-api.md#errors).
