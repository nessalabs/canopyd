//! `canopyd` — the CLI.
//!
//! Deliberately thin: parse, call one library method, print. Anything that looks like logic
//! belongs in the library, where the other consumers of this crate can reach it.

use std::io::Write;
use std::process::ExitCode;

use camino::Utf8PathBuf;
use canopyd::error::Result;
use canopyd::wire::Envelope;
use canopyd::{Canopy, Error};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "canopyd", version, about = "Git worktree dev environments driven by canopy.yaml")]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "PATH")]
    directory: Option<Utf8PathBuf>,

    /// Print one JSON envelope on stdout instead of human output.
    #[arg(long, global = true)]
    json: bool,

    /// Suppress progress on stderr. Results still go to stdout.
    #[arg(long, short, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Show what repository this is and where its pieces live.
    Info,
    /// List every worktree git knows about.
    List,
    /// Read and validate canopy.yaml.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Print where a branch's worktree would live. The branch need not exist.
    Path {
        branch: String,
        /// Directory name; defaults to the sanitized branch.
        #[arg(long)]
        name: Option<String>,
    },
    /// Create a worktree for a branch.
    New {
        branch: String,
        /// Branch to fork from. Implies creating the branch.
        #[arg(long)]
        base: Option<String>,
        /// Check out an existing branch instead of creating one.
        #[arg(long, conflicts_with = "base")]
        existing: bool,
        /// Put the worktree here, ignoring the `worktree.path` template.
        #[arg(long)]
        path: Option<Utf8PathBuf>,
        /// Directory name; defaults to the sanitized branch.
        #[arg(long)]
        name: Option<String>,
    },
    /// Show the ports allocated to a branch, allocating them if needed.
    Ports {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// Show the whole registry instead of one branch.
        #[arg(long, conflicts_with = "branch")]
        all: bool,
        /// Hand a branch's ports back to the pool.
        #[arg(long, conflicts_with_all = ["all"])]
        release: bool,
        /// Pin a port for this branch, as `NAME=PORT`. Repeatable. For an embedder that picks
        /// some numbers itself and needs them in the registry, so nothing else is handed them.
        #[arg(long = "reserve", value_name = "NAME=PORT", conflicts_with_all = ["all", "release"])]
        reserve: Vec<String>,
    },
    /// Show the resolved environment for a branch's worktree.
    Env {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// Print `export K='v'` lines for `eval`.
        #[arg(long, conflicts_with = "write")]
        export: bool,
        /// Write the file named by `env_file:` into the worktree.
        #[arg(long)]
        write: bool,
        /// With `--json`, print secrets as they are instead of masked. For an embedder that
        /// stores the table and does its own masking; a terminal has `--export` for that.
        #[arg(long, conflicts_with_all = ["export", "write"])]
        reveal: bool,
        /// Add or override an environment variable, as `KEY=VALUE`. Repeatable. Layered last,
        /// for an embedder that knows things this crate cannot resolve — see `setup --env`.
        #[arg(long = "env")]
        env_overrides: Vec<String>,
    },
    /// Start a worktree's services.
    Up {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// Start only these services. Repeatable.
        #[arg(long)]
        only: Vec<String>,
        /// Return as soon as each service is spawned, without waiting for its health check.
        #[arg(long)]
        no_wait: bool,
        /// Add or override an environment variable, as `KEY=VALUE`. Repeatable. Layered last,
        /// for an embedder that knows things this crate cannot resolve — see `setup --env`.
        #[arg(long = "env")]
        env_overrides: Vec<String>,
    },
    /// Stop a worktree's services.
    Down {
        branch: Option<String>,
        #[arg(long)]
        only: Vec<String>,
        /// Add or override an environment variable, as `KEY=VALUE`. Repeatable. Layered last,
        /// for an embedder that knows things this crate cannot resolve — see `setup --env`.
        #[arg(long = "env")]
        env_overrides: Vec<String>,
    },
    /// Show what is running for a worktree.
    Ps {
        branch: Option<String>,
        /// Add or override an environment variable, as `KEY=VALUE`. Repeatable. Layered last,
        /// for an embedder that knows things this crate cannot resolve — see `setup --env`.
        #[arg(long = "env")]
        env_overrides: Vec<String>,
    },
    /// Show a service's log.
    Logs {
        service: String,
        branch: Option<String>,
        /// Keep printing as new lines arrive.
        #[arg(long, short)]
        follow: bool,
        /// How many existing lines to show first.
        #[arg(long, short = 'n', default_value_t = 200)]
        lines: usize,
        /// Start from this byte offset instead of from the newest lines: the `next_offset` an
        /// earlier read returned. Implies `--offsets`.
        #[arg(long)]
        since: Option<u64>,
        /// With `--json`, report each line's offset and where to continue from, so a reader can
        /// come back for exactly what it has not seen.
        #[arg(long)]
        offsets: bool,
    },
    /// Carry gitignored files into a worktree, per the `copy:` rules.
    Copy {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// The checkout to copy from. Defaults to the main checkout.
        #[arg(long)]
        from: Option<Utf8PathBuf>,
        /// Report the plan and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Use these rules instead of the config's `copy:`, as `pattern` or `pattern=strategy`
        /// where strategy is copy, clone or symlink. Repeatable. For an embedder that keeps its
        /// own rules, the way `--path` exists for one that owns its own layout.
        #[arg(long = "rule")]
        rules: Vec<String>,
    },
    /// Run the `setup:` steps for a worktree.
    Setup {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// Run every step, even ones `if_changed` would skip.
        #[arg(long, short)]
        force: bool,
        /// Run only these steps, by name. Repeatable.
        #[arg(long)]
        only: Vec<String>,
        /// Give up on any single step after this long, e.g. `5m`.
        #[arg(long)]
        timeout: Option<String>,
        /// Add or override an environment variable, as `KEY=VALUE`. Repeatable. For an embedder
        /// whose environment is richer than this crate can resolve — Canopy's database URLs,
        /// say — the way `--rule` exists for one that keeps its own copy rules.
        #[arg(long = "env")]
        env_overrides: Vec<String>,
    },
    /// Keep a worktree's services alive in the foreground until Ctrl-C.
    Run {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// Supervise only these services. Repeatable.
        #[arg(long)]
        only: Vec<String>,
        /// Report exits without restarting anything.
        #[arg(long)]
        no_restart: bool,
        /// How often to check on things, e.g. `250ms`.
        #[arg(long)]
        poll: Option<String>,
        /// First restart delay; it doubles up to `--backoff-max`.
        #[arg(long)]
        backoff: Option<String>,
        #[arg(long)]
        backoff_max: Option<String>,
        /// Give up after this many restarts inside `--restart-window`.
        #[arg(long)]
        restarts: Option<u32>,
        #[arg(long)]
        restart_window: Option<String>,
        /// Take `start <service>`, `stop <service>` and `restart <service>` on stdin, one per
        /// line. End of input stops everything, so an embedder that dies takes its services
        /// with it instead of orphaning them.
        #[arg(long)]
        control: bool,
        /// Add or override an environment variable, as `KEY=VALUE`. Repeatable. Layered last,
        /// for an embedder that knows things this crate cannot resolve — see `setup --env`.
        #[arg(long = "env")]
        env_overrides: Vec<String>,
    },
    /// Report anything wrong with this repository's canopyd state.
    Doctor,
    /// Sweep what `doctor` reports as debris. Removes only what it can prove is dead.
    Gc {
        /// Truncate logs larger than this many bytes.
        #[arg(long)]
        log_cap: Option<u64>,
    },
    /// Database forks: a private copy of each declared database for a worktree.
    #[command(subcommand)]
    Db(DbCommand),
    /// The git post-checkout bridge, so a worktree made by plain `git worktree add` is noticed.
    #[command(subcommand)]
    Hook(HookCommand),
    /// Remove a worktree, by branch name or path.
    Rm {
        target: String,
        /// Remove even with uncommitted changes, discarding them.
        #[arg(long, short)]
        force: bool,
        /// What to do with the branch afterwards.
        #[arg(long, value_enum, default_value = "never")]
        delete_branch: DeleteBranchArg,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum DbCommand {
    /// Fork the databases that do not have a fork yet. One that does is left as it is.
    Fork {
        /// Defaults to the branch of the worktree you are in.
        branch: Option<String>,
        /// Fork only these databases. Repeatable.
        #[arg(long)]
        only: Vec<String>,
        /// What a new fork starts as: `template` (the project's seed), `empty`, or the name of
        /// a branch whose fork to copy.
        #[arg(long, default_value = "template")]
        from: String,
    },
    /// Show the worktree's forks, checked against what is on disk.
    Ls { branch: Option<String> },
    /// Throw one fork away and make it again.
    Reset {
        name: String,
        branch: Option<String>,
        #[arg(long, default_value = "template")]
        from: String,
    },
    /// Rebuild seed templates from their seeds. Existing forks are untouched.
    Template {
        branch: Option<String>,
        /// Rebuild only these. Repeatable.
        #[arg(long)]
        only: Vec<String>,
    },
    /// Remove the worktree's forks.
    Drop {
        branch: Option<String>,
        /// Remove only these. Repeatable.
        #[arg(long)]
        only: Vec<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum HookCommand {
    /// Install the post-checkout hook. Refuses to overwrite one that is not ours.
    Install,
    /// Remove it. Leaves a hook that is not ours alone.
    Uninstall,
    /// Whether it is installed.
    Status,
    /// Called by the installed hook. Always exits 0 — git cannot abort a checkout anyway, and
    /// the hooks directory is shared, so a failure here would break every worktree at once.
    PostCheckout { old: String, new: String, flag: String },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum DeleteBranchArg {
    /// Leave the branch alone.
    Never,
    /// Delete it only if git agrees its work is already merged.
    IfMerged,
    /// Delete it regardless.
    Always,
}

impl From<DeleteBranchArg> for canopyd::DeleteBranch {
    fn from(value: DeleteBranchArg) -> Self {
        match value {
            DeleteBranchArg::Never => canopyd::DeleteBranch::Never,
            DeleteBranchArg::IfMerged => canopyd::DeleteBranch::IfMerged,
            DeleteBranchArg::Always => canopyd::DeleteBranch::Always,
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
enum ConfigCommand {
    /// Validate the config and report errors and warnings.
    Check {
        /// Validate text on stdin instead of the file on disk — for an editor validating a
        /// buffer that has not been saved.
        #[arg(long)]
        stdin: bool,
    },
    /// Print the effective config, with every default filled in.
    Show,
    /// Print the path of the canopy.yaml in effect, and where it was found.
    Path,
    /// Print a starter canopy.yaml on stdout. Never writes; redirect it yourself.
    Init,
    /// Print the JSON Schema for canopy.yaml.
    Schema,
}

impl Command {
    /// The `command` field of the envelope. Stable; consumers match on it.
    fn name(&self) -> &'static str {
        match self {
            Command::Info => "info",
            Command::List => "list",
            Command::Config(ConfigCommand::Check { .. }) => "config check",
            Command::Config(ConfigCommand::Show) => "config show",
            Command::Config(ConfigCommand::Path) => "config path",
            Command::Config(ConfigCommand::Init) => "config init",
            Command::Config(ConfigCommand::Schema) => "config schema",
            Command::Path { .. } => "path",
            Command::New { .. } => "new",
            Command::Ports { .. } => "ports",
            Command::Env { .. } => "env",
            Command::Up { .. } => "up",
            Command::Down { .. } => "down",
            Command::Ps { .. } => "ps",
            Command::Logs { .. } => "logs",
            Command::Copy { .. } => "copy",
            Command::Setup { .. } => "setup",
            Command::Run { .. } => "run",
            Command::Doctor => "doctor",
            Command::Gc { .. } => "gc",
            Command::Db(DbCommand::Fork { .. }) => "db fork",
            Command::Db(DbCommand::Ls { .. }) => "db ls",
            Command::Db(DbCommand::Reset { .. }) => "db reset",
            Command::Db(DbCommand::Template { .. }) => "db template",
            Command::Db(DbCommand::Drop { .. }) => "db drop",
            Command::Hook(HookCommand::Install) => "hook install",
            Command::Hook(HookCommand::Uninstall) => "hook uninstall",
            Command::Hook(HookCommand::Status) => "hook status",
            Command::Hook(HookCommand::PostCheckout { .. }) => "hook post-checkout",
            Command::Rm { .. } => "rm",
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let name = cli.command.name();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if cli.json {
                // A failure is still a well-formed envelope on stdout: a consumer parses one
                // shape whatever happened, and reads `ok` to find out which.
                let envelope = Envelope::err(name, &error);
                println!("{}", serde_json::to_string(&envelope).expect("envelope is serializable"));
            } else {
                let _ = writeln!(std::io::stderr(), "canopyd: {error}");
            }
            ExitCode::from(error.code().exit_code())
        }
    }
}

/// Returns the process exit code. Commands that report a *verdict* rather than a fault —
/// `config check` on an invalid file — print their own envelope and return a non-zero code,
/// so exactly one envelope reaches stdout either way.
fn run(cli: &Cli) -> Result<u8> {
    let cwd = match &cli.directory {
        Some(dir) => dir.clone(),
        None => current_dir()?,
    };

    // `config init` and `config schema` describe the *format*, not a repository: `init` is what
    // you run to create the first file, often in a directory you have just made, and `schema` is
    // what you point an editor at while writing it. Both are answered here when there is no
    // repository to open, and by the ordinary path when there is — the answer does not depend
    // on one either way.
    let canopy = match Canopy::open(&cwd) {
        Ok(canopy) => canopy,
        Err(error) => {
            return match &cli.command {
                Command::Config(config) => config.document().map(print_document).ok_or(error),
                _ => Err(error),
            };
        }
    };

    match cli.command {
        Command::Info => {
            let info = canopy.info()?;
            if cli.json {
                emit("info", &info);
            } else {
                println!("name        {}", info.name);
                println!("root        {}", info.root.as_ref().map(|p| p.as_str()).unwrap_or("(bare)"));
                println!("common dir  {}", info.common_dir);
                println!("git dir     {}", info.git_dir);
                println!("worktrees   {}", info.worktrees);
            }
        }
        Command::List => {
            let worktrees = canopy.list()?;
            if cli.json {
                emit("list", &worktrees);
            } else {
                for entry in &worktrees {
                    let what = match (&entry.branch, entry.bare, entry.detached) {
                        (Some(branch), _, _) => branch.clone(),
                        (None, true, _) => "(bare)".to_owned(),
                        (None, _, true) => format!("(detached {})", short(entry.head.as_deref())),
                        _ => "(no branch)".to_owned(),
                    };
                    println!("{:<24} {}", what, entry.path);
                }
            }
        }
        Command::Config(ref config_command) => return run_config(cli, &canopy, config_command),

        Command::Path { ref branch, ref name } => {
            let path = canopy.path_for(branch, name.as_deref())?;
            if cli.json {
                emit("path", &serde_json::json!({ "branch": branch, "path": path }));
            } else {
                println!("{path}");
            }
        }

        Command::New { ref branch, ref base, existing, ref path, ref name } => {
            let spec = if existing {
                canopyd::BranchSpec::Existing { name: branch.clone() }
            } else {
                // Without an explicit base, fork from the repository's default branch. Falling
                // back to "wherever HEAD happens to be" would silently branch off whatever the
                // main checkout was last left on.
                canopyd::BranchSpec::New { name: branch.clone(), base: base.clone().or_else(|| canopy.default_base()) }
            };
            let options = canopyd::CreateOptions { path: path.clone(), name: name.clone() };
            let outcome = canopy.create(&spec, &options)?;
            if cli.json {
                emit("new", &outcome);
            } else {
                let from = outcome.base.as_deref().map(|b| format!(" from {b}")).unwrap_or_default();
                let verb = if outcome.created_branch { "created" } else { "checked out" };
                println!("{verb} {}{from}", outcome.branch);
                println!("{}", outcome.path);
            }
        }

        Command::Ports { ref branch, all, release, ref reserve } => {
            if all {
                // This repository's rows: a shared registry holds other projects' too.
                let rows = canopy.port_registry()?.own_rows();
                if cli.json {
                    emit("ports", &rows);
                } else if rows.is_empty() {
                    println!("no ports allocated");
                } else {
                    for row in &rows {
                        println!("{:<24} {:<12} {}", row.branch, row.name, row.port);
                    }
                }
            } else {
                let branch = resolve_branch(&canopy, branch.as_deref())?;
                if !reserve.is_empty() {
                    let table = canopy.reserve_ports(&branch, &parse_reservations(reserve)?)?;
                    if cli.json {
                        emit("ports", &table);
                    } else {
                        for (name, port) in &table {
                            println!("{name:<12} {port}");
                        }
                    }
                } else if release {
                    let removed = canopy.release_ports(&branch)?;
                    if cli.json {
                        emit("ports", &serde_json::json!({ "branch": branch, "released": removed }));
                    } else {
                        println!("released {removed} port(s) for {branch}");
                    }
                } else {
                    let table = canopy.ports_for(&branch)?;
                    if cli.json {
                        emit("ports", &table);
                    } else {
                        for (name, port) in &table {
                            println!("{name:<12} {port}");
                        }
                    }
                }
            }
        }

        Command::Env { ref branch, export, write, reveal, ref env_overrides } => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let worktree = worktree_path_for_branch(&canopy, &branch)?;
            let table = canopy.env_for_with(&branch, &worktree, &overrides(env_overrides)?)?;
            if write {
                let default_config = canopyd::config::CanopyConfig::empty();
                let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
                let written = canopyd::env::write_env_file(config, &worktree, &table)?;
                match written {
                    Some(path) if cli.json => emit("env", &serde_json::json!({ "written": path })),
                    Some(path) => println!("wrote {path}"),
                    // `env_file: false` is a choice, not a failure.
                    None if cli.json => emit("env", &serde_json::json!({ "written": null })),
                    None => println!("env_file is disabled; nothing written"),
                }
            } else if cli.json && reveal {
                emit("env", &table.vars());
            } else if cli.json {
                // Secrets are masked for display; the file and `--export` keep the real values.
                emit("env", &table.masked());
            } else if export {
                print!("{}", table.to_export());
            } else {
                print!("{}", table.to_dotenv());
            }
        }

        Command::Up { ref branch, ref only, no_wait, ref env_overrides } => {
            let (branch, worktree, state, env, facts_owner) =
                service_context(&canopy, branch.as_deref(), env_overrides)?;
            let default_config = canopyd::config::CanopyConfig::empty();
            let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
            let facts = facts_owner.facts();
            let ctx = canopyd::ServiceContext { worktree: &worktree, state: &state, env: &env, facts: &facts };
            let only = selection(only);
            let statuses = canopyd::service::up(&config.services, only.as_ref(), &ctx, !no_wait)?;
            report_services(cli, "up", &statuses);
            let _ = branch;
        }

        Command::Down { ref branch, ref only, ref env_overrides } => {
            let (_, worktree, state, env, facts_owner) = service_context(&canopy, branch.as_deref(), env_overrides)?;
            let default_config = canopyd::config::CanopyConfig::empty();
            let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
            let facts = facts_owner.facts();
            let ctx = canopyd::ServiceContext { worktree: &worktree, state: &state, env: &env, facts: &facts };
            let only = selection(only);
            let statuses = canopyd::service::down(&config.services, only.as_ref(), &ctx)?;
            report_services(cli, "down", &statuses);
        }

        Command::Ps { ref branch, ref env_overrides } => {
            let (_, worktree, state, env, facts_owner) = service_context(&canopy, branch.as_deref(), env_overrides)?;
            let default_config = canopyd::config::CanopyConfig::empty();
            let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
            let facts = facts_owner.facts();
            let ctx = canopyd::ServiceContext { worktree: &worktree, state: &state, env: &env, facts: &facts };
            let statuses = canopyd::service::status(&config.services, None, &ctx)?;
            report_services(cli, "ps", &statuses);
        }

        Command::Logs { ref service, ref branch, follow, lines, since, offsets } => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let state = canopy.state_dir(&branch);
            if follow && cli.json {
                // A stream, so there is no envelope to end it with: one event per line, until
                // the caller goes away. `--json` and `-f` together used to mean plain lines,
                // which is not something a program asking for JSON can use.
                let interrupted = interrupt_flag()?;
                let mut out = |event: canopyd::service::LogEvent| {
                    println!("{}", serde_json::to_string(&event).expect("event is serializable"));
                };
                canopyd::service::follow_from(
                    &state,
                    service,
                    since,
                    lines,
                    canopyd::service::FOLLOW_POLL,
                    &mut out,
                    &|| !interrupted.load(std::sync::atomic::Ordering::Relaxed),
                )?;
            } else if follow {
                // Ctrl-C has to land even on a service that has gone quiet, so the stop
                // condition is checked on every poll, not only between lines.
                let interrupted = interrupt_flag()?;
                let mut out = |line: &str| println!("{line}");
                canopyd::service::follow(&state, service, lines, canopyd::service::FOLLOW_POLL, &mut out, &|| {
                    !interrupted.load(std::sync::atomic::Ordering::Relaxed)
                })?;
            } else if since.is_some() || offsets {
                let page = canopyd::service::page(&state, service, since, lines)?;
                if cli.json {
                    emit("logs", &page);
                } else {
                    for line in &page.lines {
                        println!("{}", line.text);
                    }
                }
            } else {
                let tail = canopyd::service::logs(&state, service, lines)?;
                if cli.json {
                    emit("logs", &tail);
                } else {
                    for line in &tail {
                        println!("{line}");
                    }
                }
            }
        }

        Command::Copy { ref branch, ref from, dry_run, ref rules } => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let target = worktree_path_for_branch(&canopy, &branch)?;
            if !target.exists() {
                return Err(Error::WorktreeNotFound(format!("{branch} has no checkout at {target}")));
            }
            // The main checkout is what a worktree was made from, so it is where its
            // gitignored files come from unless told otherwise.
            let source = match from {
                Some(path) => path.clone(),
                None => canopy.repo().root.clone().ok_or_else(|| {
                    Error::WorktreeNotFound("a bare repository has no checkout to copy from".to_owned())
                })?,
            };
            let default_config = canopyd::config::CanopyConfig::empty();
            let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
            // `--rule` wins over the config's `copy:`, for an embedder that keeps its own rules
            // — the same reason `--path` exists for one that owns its own layout.
            let overrides = parse_rules(rules)?;
            let rules = if overrides.is_empty() { &config.copy } else { &overrides };
            let options = canopyd::copy::CopyOptions { dry_run, source: source.clone() };
            let outcome =
                canopyd::copy::copy_ignored(&canopyd::git::Git::default(), &source, &target, rules, &options)?;

            if cli.json {
                emit("copy", &outcome);
            } else {
                for entry in &outcome.entries {
                    println!(
                        "{:<10} {} ({} bytes, {}ms)",
                        format!("{:?}", entry.result).to_lowercase(),
                        entry.path,
                        entry.bytes,
                        entry.millis
                    );
                }
                for failure in &outcome.failures {
                    println!("failed     {} — {}", failure.path, failure.message);
                }
                if outcome.entries.is_empty() && outcome.failures.is_empty() {
                    println!("nothing to copy");
                }
            }
        }

        Command::Setup { ref branch, force, ref only, ref timeout, ref env_overrides } => {
            // The same branch, checkout, environment and `${…}` scope `up` resolves: a step that
            // says `${ports.web}` or `${db.main.url}` means what the service that runs next does.
            let (_branch, worktree, _state, table, owner) = service_context(&canopy, branch.as_deref(), env_overrides)?;
            let default_config = canopyd::config::CanopyConfig::empty();
            let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
            let timeout = match timeout {
                Some(text) => Some(canopyd::config::Duration::parse(text).map_err(|error| Error::Module {
                    code: canopyd::ErrorCode::ConfigInvalid,
                    message: error.to_string(),
                })?),
                None => None,
            };
            let facts = owner.facts();
            let steps: Vec<canopyd::config::SetupStep> = config
                .setup
                .iter()
                .map(|step| canopyd::config::SetupStep {
                    run: canopyd::env::interpolate(&step.run, &facts, &table),
                    env: step
                        .env
                        .iter()
                        .map(|(key, value)| (key.clone(), canopyd::env::interpolate(value, &facts, &table)))
                        .collect(),
                    ..step.clone()
                })
                .collect();
            let env = table.to_map();
            let options = canopyd::SetupOptions {
                worktree: &worktree,
                // The main checkout is what a worktree was made from, so it is what
                // `if_changed` compares against.
                source: canopy.repo().root.as_deref(),
                env: &env,
                force,
                only: (!only.is_empty()).then(|| only.clone()),
                timeout,
            };

            // Streamed to stderr as it happens: a four-minute `npm ci` that prints nothing
            // until it finishes looks like a hang. stdout stays clean for the envelope.
            let quiet = cli.quiet;
            let mut on_line = |_stream: canopyd::Stream, text: &str| {
                if !quiet {
                    let _ = writeln!(std::io::stderr(), "{text}");
                }
            };
            let outcome = canopyd::run_setup(&steps, &options, &mut on_line)?;

            if cli.json {
                // A verdict, like `config check`: the run happened, and the answer may be no.
                // `data` carries every step either way, so a caller reads one shape.
                let failure = outcome.steps.iter().find_map(|step| match &step.result {
                    canopyd::StepResult::Failed { status, .. } => Some((step.name.clone(), status.clone())),
                    _ => None,
                });
                let error = failure.map(|(name, status)| canopyd::wire::ErrorBody {
                    code: canopyd::ErrorCode::SetupFailed.as_str(),
                    message: format!("setup step {name} failed ({status})"),
                    details: None,
                });
                let envelope = Envelope::verdict("setup", outcome.ok, &outcome, error);
                println!("{}", serde_json::to_string(&envelope).expect("envelope is serializable"));
            } else {
                for step in &outcome.steps {
                    println!("{}", describe_step(step));
                }
            }
            // A failed step is a failed command, so CI does not have to read the summary.
            return Ok(if outcome.ok { 0 } else { 1 });
        }

        Command::Run {
            ref branch,
            ref only,
            no_restart,
            ref poll,
            ref backoff,
            ref backoff_max,
            restarts,
            ref restart_window,
            control,
            ref env_overrides,
        } => {
            let (_, worktree, state, env, facts_owner) = service_context(&canopy, branch.as_deref(), env_overrides)?;
            let default_config = canopyd::config::CanopyConfig::empty();
            let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
            let facts = facts_owner.facts();
            let ctx = canopyd::ServiceContext { worktree: &worktree, state: &state, env: &env, facts: &facts };

            let mut opts = canopyd::SuperviseOptions { restart: !no_restart, ..Default::default() };
            if let Some(text) = poll {
                opts.poll = duration_arg(text)?;
            }
            if let Some(text) = backoff {
                opts.backoff.base = duration_arg(text)?;
            }
            if let Some(text) = backoff_max {
                opts.backoff.max = duration_arg(text)?;
            }
            if let Some(count) = restarts {
                opts.budget.restarts = count;
            }
            if let Some(text) = restart_window {
                opts.budget.window = duration_arg(text)?;
            }

            // Ctrl-C has to reach a supervisor that may be asleep between polls, so it flips a
            // flag the loop checks rather than killing us where we stand — services would
            // otherwise be left running with nothing watching them.
            let stopping = interrupt_flag()?;

            let quiet = cli.quiet;
            let json = cli.json;
            let mut on_event = |event: canopyd::Event| {
                if json {
                    // One event per line on stderr: stdout is the final envelope, and a caller
                    // following along wants the events as they happen rather than at the end.
                    let _ = writeln!(
                        std::io::stderr(),
                        "{}",
                        serde_json::to_string(&event).expect("event is serializable")
                    );
                } else if !quiet {
                    let _ = writeln!(std::io::stderr(), "{event}");
                }
            };

            // Requests arrive on a thread of their own because a blocking read of stdin and a poll
            // loop cannot share one. The loop drains whatever has arrived, once per poll.
            let (requests, inbox) = std::sync::mpsc::channel::<String>();
            if control {
                let stopping = stopping.clone();
                std::thread::spawn(move || {
                    for line in std::io::stdin().lines().map_while(std::io::Result::ok) {
                        let _ = requests.send(line);
                    }
                    stopping.store(true, std::sync::atomic::Ordering::Relaxed);
                });
            }
            let mut controls = || {
                inbox
                    .try_iter()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| {
                        line.parse::<canopyd::Control>()
                            .map_err(|detail| canopyd::Rejection { request: line.trim().to_owned(), detail })
                    })
                    .collect::<Vec<_>>()
            };

            let clock = canopyd::health::SystemClock::new();
            let outcome = canopyd::supervise::run_with(
                &config.services,
                selection(only).as_ref(),
                &ctx,
                &opts,
                &clock,
                &|| stopping.load(std::sync::atomic::Ordering::Relaxed),
                &mut controls,
                &mut on_event,
            )?;

            if cli.json {
                emit("run", &outcome);
            } else {
                println!("stopped after {} restart(s)", outcome.restarts);
            }
        }

        Command::Db(DbCommand::Fork { ref branch, ref only, ref from }) => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let forks = canopy.db_fork(&branch, selection(only).as_ref(), &canopyd::DbFrom::parse(from))?;
            report_databases(cli, "db fork", &forks);
        }

        Command::Db(DbCommand::Ls { ref branch }) => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            report_databases(cli, "db ls", &canopy.db_list(&branch)?);
        }

        Command::Db(DbCommand::Reset { ref name, ref branch, ref from }) => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let fork = canopy.db_reset(&branch, name, &canopyd::DbFrom::parse(from))?;
            if cli.json {
                emit("db reset", &fork);
            } else {
                report_databases(cli, "db reset", std::slice::from_ref(&fork));
            }
        }

        Command::Db(DbCommand::Template { ref branch, ref only }) => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let rebuilt = canopy.db_refresh_templates(&branch, selection(only).as_ref())?;
            if cli.json {
                emit("db template", &serde_json::json!({ "rebuilt": rebuilt }));
            } else if rebuilt.is_empty() {
                println!("no templates to rebuild");
            } else {
                for name in &rebuilt {
                    println!("rebuilt {name}");
                }
            }
        }

        Command::Db(DbCommand::Drop { ref branch, ref only }) => {
            let branch = resolve_branch(&canopy, branch.as_deref())?;
            let dropped = canopy.db_drop(&branch, selection(only).as_ref())?;
            if cli.json {
                emit("db drop", &serde_json::json!({ "dropped": dropped }));
            } else if dropped.is_empty() {
                println!("no forks");
            } else {
                for name in &dropped {
                    println!("dropped {name}");
                }
            }
        }

        Command::Doctor => {
            let report = canopyd::doctor::diagnose(canopy.repo(), &canopy.state_root(), &canopy.ports_path())?;
            // Only an error fails the command. A warning is debris `gc` sweeps as a matter of
            // course, and exiting non-zero for it would make `doctor` useless in CI — the place
            // you actually want it to mean "someone has to look at this".
            let errors = report.findings.iter().filter(|f| f.severity == canopyd::FindingSeverity::Error).count();

            if cli.json {
                let error = (errors > 0).then(|| canopyd::wire::ErrorBody {
                    code: canopyd::ErrorCode::RepositoryUnhealthy.as_str(),
                    message: format!("{errors} finding(s) need attention"),
                    details: None,
                });
                let envelope = Envelope::verdict("doctor", errors == 0, &report, error);
                println!("{}", serde_json::to_string(&envelope).expect("envelope is serializable"));
            } else if report.findings.is_empty() {
                println!("no problems found");
            } else {
                for finding in &report.findings {
                    println!(
                        "{:<8} {:<26} {}",
                        format!("{:?}", finding.severity).to_lowercase(),
                        finding.check,
                        finding.message
                    );
                }
                if errors == 0 {
                    println!("\nnothing here needs a person: `canopyd gc` sweeps all of it");
                }
            }
            return Ok(if errors == 0 { 0 } else { 1 });
        }

        Command::Gc { log_cap } => {
            let state_root = canopy.state_root();
            let ports = canopy.ports_path();
            let swept = match log_cap {
                Some(cap) => canopyd::doctor::gc_with(canopy.repo(), &state_root, &ports, cap)?,
                None => canopyd::doctor::gc(canopy.repo(), &state_root, &ports)?,
            };
            if cli.json {
                emit("gc", &swept);
            } else {
                println!(
                    "released {} port(s), removed {} record(s) and {} state dir(s), truncated {} log(s)",
                    swept.ports_released, swept.records_removed, swept.state_dirs_removed, swept.logs_truncated
                );
            }
        }

        Command::Hook(ref hook_command) => return run_hook(cli, &canopy, hook_command),

        Command::Rm { ref target, force, delete_branch } => {
            // Refused before anything else happens: stopping a compose stack below also removes
            // its volumes, which a removal that is then refused for uncommitted work must not do.
            canopy.check_removable(target, force)?;
            // Stop anything still running before the checkout goes, or a dev server keeps
            // writing into a directory that no longer exists.
            let stopped = stop_services_for(&canopy, target).unwrap_or_default();
            let options = canopyd::RemoveOptions { force, delete_branch: delete_branch.into() };
            let outcome = canopy.remove(target, &options)?;
            // Everything after this point is best effort: the checkout is gone, so reporting the
            // command as failed would tell the caller a removal that happened did not.
            let mut warnings = Vec::new();
            // Hand the ports back. Without this the registry accumulates rows for worktrees
            // that no longer exist and slowly exhausts the range.
            let released = match outcome.branch.as_deref().map(|branch| canopy.release_ports(branch)).transpose() {
                Ok(released) => released.unwrap_or(0),
                Err(error) => {
                    warnings.push(format!("ports were not released: {error}"));
                    0
                }
            };
            // A file-backed fork goes with the state directory below. One on a server does not:
            // it has to be dropped while the record that names it still exists.
            let dropped = match outcome.branch.as_deref().map(|branch| canopy.db_drop(branch, None)).transpose() {
                Ok(dropped) => dropped.unwrap_or_default(),
                Err(error) => {
                    warnings.push(format!("database forks were not dropped: {error}"));
                    Vec::new()
                }
            };
            let state = outcome.branch.as_deref().map(|branch| canopy.state_dir(branch));
            if let Some(state) = state.filter(|path| path.exists()) {
                // The records and logs describe a worktree that is gone.
                let _ = std::fs::remove_dir_all(&state);
            }
            if cli.json {
                let data = serde_json::json!({
                    "path": outcome.path,
                    "branch": outcome.branch,
                    "branch_deleted": outcome.branch_deleted,
                    "ports_released": released,
                    "services_stopped": stopped,
                    "databases_dropped": dropped,
                });
                println!(
                    "{}",
                    serde_json::to_string(&Envelope::ok("rm", data, warnings)).expect("envelope is serializable")
                );
            } else {
                for warning in &warnings {
                    let _ = writeln!(std::io::stderr(), "warning: {warning}");
                }
                println!("removed {}", outcome.path);
                if outcome.branch_deleted {
                    println!("deleted branch {}", outcome.branch.as_deref().unwrap_or("?"));
                }
                if released > 0 {
                    println!("released {released} port(s)");
                }
            }
        }
    }
    Ok(0)
}

fn run_config(cli: &Cli, canopy: &Canopy, command: &ConfigCommand) -> Result<u8> {
    match command {
        ConfigCommand::Init | ConfigCommand::Schema => {
            return Ok(command.document().map(print_document).unwrap_or(0));
        }

        ConfigCommand::Path => {
            let Some((located, _)) = canopy.config() else {
                return Err(Error::ConfigNotFound(searched_description(canopy)));
            };
            if cli.json {
                emit("config path", located);
            } else {
                println!("{}  ({})", located.path, source_label(located.source));
            }
        }

        ConfigCommand::Show => {
            let Some((_, parsed)) = canopy.config() else {
                return Err(Error::ConfigNotFound(searched_description(canopy)));
            };
            let Some(config) = &parsed.config else {
                return Err(Error::ConfigInvalid(parsed.error_count()));
            };
            if cli.json {
                emit("config show", config);
            } else {
                // The human form of "show" is the YAML you would have written with every
                // default spelled out, which is the question people actually have.
                print!("{}", serde_json::to_string_pretty(config).expect("config is serializable"));
                println!();
            }
        }

        ConfigCommand::Check { stdin } => {
            let (label, parsed) = if *stdin {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
                ("<stdin>".to_owned(), canopyd::parse_str(&text))
            } else {
                let Some((located, parsed)) = canopy.config() else {
                    return Err(Error::ConfigNotFound(searched_description(canopy)));
                };
                (located.path.to_string(), parsed.clone())
            };

            let report = CheckReport {
                path: &label,
                valid: parsed.is_valid(),
                errors: parsed.error_count(),
                warnings: parsed.warning_count(),
                diagnostics: &parsed.diagnostics,
            };

            if cli.json {
                // One envelope, whatever the verdict. `ok` is the verdict and `data` always
                // carries the diagnostics, so a caller reads warnings off a passing file the
                // same way it reads errors off a failing one.
                let envelope = Envelope::verdict(
                    "config check",
                    parsed.is_valid(),
                    &report,
                    (!parsed.is_valid()).then(|| canopyd::wire::ErrorBody {
                        code: canopyd::ErrorCode::ConfigInvalid.as_str(),
                        message: format!("canopy.yaml has {} error(s)", parsed.error_count()),
                        details: None,
                    }),
                );
                println!("{}", serde_json::to_string(&envelope).expect("envelope is serializable"));
            } else {
                for diagnostic in &parsed.diagnostics {
                    let severity = match diagnostic.severity {
                        canopyd::Severity::Error => "error",
                        canopyd::Severity::Warning => "warning",
                    };
                    let position = match (diagnostic.line, diagnostic.column) {
                        (Some(line), Some(column)) => format!("{label}:{line}:{column}"),
                        _ => label.clone(),
                    };
                    println!("{position}: {severity}: {}: {}", diagnostic.path, diagnostic.message);
                }
                println!(
                    "{}: {} error(s), {} warning(s)",
                    if parsed.is_valid() { "ok" } else { "invalid" },
                    parsed.error_count(),
                    parsed.warning_count()
                );
            }
            // Exit 1 on an invalid config so CI does not have to parse the summary line. The
            // envelope has already been printed, which is why this returns a code rather than
            // an error.
            return Ok(if parsed.is_valid() { 0 } else { 1 });
        }
    }
    Ok(0)
}

#[derive(serde::Serialize)]
struct CheckReport<'a> {
    path: &'a str,
    valid: bool,
    errors: usize,
    warnings: usize,
    diagnostics: &'a [canopyd::Diagnostic],
}

fn source_label(source: canopyd::ConfigSource) -> &'static str {
    match source {
        canopyd::ConfigSource::Worktree => "this worktree",
        canopyd::ConfigSource::MainCheckout => "the main checkout",
        canopyd::ConfigSource::UserConfig => "your user config",
    }
}

/// Where we looked, so "no config" is actionable rather than just a refusal.
fn searched_description(canopy: &Canopy) -> String {
    let root = canopy.repo().root.as_ref().map(|p| p.to_string()).unwrap_or_else(|| "(bare repo)".to_owned());
    format!(
        "looked in {root}, the main checkout, and your user config; `canopyd config init > canopy.yaml` writes a starter"
    )
}

/// `KEY=VALUE`, the spelling `--env` takes. An empty value is legal; an absent `=` is not.
/// `--reserve web=40100` as `("web", 40100)`.
fn parse_reservations(raw: &[String]) -> Result<Vec<(String, u16)>> {
    let invalid = |text: &str| Error::Module {
        code: canopyd::ErrorCode::ConfigInvalid,
        message: format!("--reserve {text}: expected NAME=PORT, e.g. db-main=40100"),
    };
    raw.iter()
        .map(|text| {
            let (name, port) = text.split_once('=').ok_or_else(|| invalid(text))?;
            let port: u16 = port.parse().map_err(|_| invalid(text))?;
            if name.is_empty() { Err(invalid(text)) } else { Ok((name.to_owned(), port)) }
        })
        .collect()
}

fn parse_env(raw: &[String]) -> Result<Vec<(String, String)>> {
    let invalid = |message: String| Error::Module { code: canopyd::ErrorCode::ConfigInvalid, message };
    raw.iter()
        .map(|text| match text.split_once('=') {
            Some(("", _)) => Err(invalid("--env needs a name before the =".to_owned())),
            Some((key, value)) => Ok((key.to_owned(), value.to_owned())),
            // A bare name means "the value I was started with", the way `docker run -e KEY` does.
            // It is how a secret is handed over: an argument is readable by every user on the
            // machine through `ps`, and an environment is not.
            None => match std::env::var(text) {
                Ok(value) => Ok((text.clone(), value)),
                Err(_) => {
                    Err(invalid(format!("--env {text}: not KEY=VALUE, and {text} is not set in the environment")))
                }
            },
        })
        .collect()
}

/// `--env` flags as the override layer [`Canopy::env_for_with`] takes. A key given twice keeps
/// its last value, which is what repeating a flag means everywhere else.
fn overrides(raw: &[String]) -> Result<std::collections::BTreeMap<String, String>> {
    Ok(parse_env(raw)?.into_iter().collect())
}

/// `pattern` or `pattern=strategy`, the spelling `--rule` takes.
fn parse_rules(raw: &[String]) -> Result<Vec<canopyd::config::CopyRule>> {
    raw.iter()
        .map(|text| {
            let (pattern, strategy) = match text.split_once('=') {
                Some((pattern, strategy)) => (pattern, strategy),
                None => (text.as_str(), "copy"),
            };
            let strategy = match strategy {
                "copy" => canopyd::config::CopyStrategy::Copy,
                "clone" => canopyd::config::CopyStrategy::Clone,
                "symlink" => canopyd::config::CopyStrategy::Symlink,
                other => {
                    return Err(Error::Module {
                        code: canopyd::ErrorCode::ConfigInvalid,
                        message: format!("unknown copy strategy {other:?}; use copy, clone or symlink"),
                    });
                }
            };
            if pattern.is_empty() {
                return Err(Error::Module {
                    code: canopyd::ErrorCode::ConfigInvalid,
                    message: "a copy rule needs a pattern".to_owned(),
                });
            }
            Ok(canopyd::config::CopyRule { pattern: pattern.to_owned(), strategy })
        })
        .collect()
}

/// Everything the service commands need, with the borrowed pieces kept alive by the caller.
struct FactsOwner {
    name: String,
    worktree: Utf8PathBuf,
    branch: String,
    project: String,
    project_path: Utf8PathBuf,
    ports: std::collections::BTreeMap<String, u16>,
    databases: Vec<canopyd::db::DbInstance>,
}

impl FactsOwner {
    fn facts(&self) -> canopyd::env::Facts<'_> {
        canopyd::env::Facts {
            worktree_name: &self.name,
            worktree_path: &self.worktree,
            branch: &self.branch,
            project: &self.project,
            project_path: &self.project_path,
            ports: &self.ports,
            databases: &self.databases,
        }
    }
}

type ServiceSetup = (String, Utf8PathBuf, Utf8PathBuf, canopyd::EnvTable, FactsOwner);

/// Resolves the branch, its checkout, its state directory and its environment in one place,
/// since every service command needs all four.
fn service_context(canopy: &Canopy, given: Option<&str>, env_overrides: &[String]) -> Result<ServiceSetup> {
    let branch = resolve_branch(canopy, given)?;
    let worktree = worktree_path_for_branch(canopy, &branch)?;
    if !worktree.exists() {
        return Err(Error::WorktreeNotFound(format!("{branch} has no checkout at {worktree}")));
    }
    let state = canopy.state_dir(&branch);
    std::fs::create_dir_all(&state)?;
    let env = canopy.env_for_with(&branch, &worktree, &overrides(env_overrides)?)?;
    let owner = FactsOwner {
        name: worktree.file_name().unwrap_or(&branch).to_owned(),
        worktree: worktree.clone(),
        branch: branch.clone(),
        project: canopy.project_name(),
        project_path: canopy.repo().root.clone().unwrap_or_else(|| canopy.repo().common_dir.clone()),
        ports: canopy.ports_for(&branch)?,
        // The forks that are there, which is what `env_for_with` just resolved against too.
        databases: canopy
            .db_list(&branch)?
            .into_iter()
            .filter(|db| db.status == canopyd::db::ForkStatus::Ready)
            .collect(),
    };
    Ok((branch, worktree, state, env, owner))
}

/// What `config init` and `config schema` print.
///
/// A type of its own rather than a subset of `ConfigCommand`, so the function below cannot be
/// handed a command that has no document — there is no arm for "this cannot happen".
#[derive(Clone, Copy, Debug)]
enum Document {
    Starter,
    Schema,
}

impl ConfigCommand {
    /// The document this command prints, if it prints one.
    ///
    /// Both describe the *format* rather than a repository, which is why they are answered even
    /// when there is no repository to open.
    fn document(&self) -> Option<Document> {
        match self {
            ConfigCommand::Init => Some(Document::Starter),
            ConfigCommand::Schema => Some(Document::Schema),
            ConfigCommand::Check { .. } | ConfigCommand::Show | ConfigCommand::Path => None,
        }
    }
}

/// Prints it. Meant to be redirected into a file, so it is never wrapped in an envelope — a
/// consumer would have to unwrap it before use.
fn print_document(document: Document) -> u8 {
    match document {
        Document::Starter => print!("{}", canopyd::config::STARTER),
        Document::Schema => println!("{}", canopyd::config::schema::json_schema().trim_end()),
    }
    0
}

/// A `--flag 5s` value, with the grammar named in the error rather than a bare "invalid".
fn duration_arg(text: &str) -> Result<canopyd::config::Duration> {
    canopyd::config::Duration::parse(text)
        .map_err(|error| Error::Module { code: canopyd::ErrorCode::ConfigInvalid, message: error.to_string() })
}

/// git's answer for the hooks directory, which honours `core.hooksPath`. Guessing
/// `.git/hooks` would install into a directory git is not reading.
fn hooks_dir(canopy: &Canopy) -> Result<Utf8PathBuf> {
    let cwd = canopy.repo().root.clone().unwrap_or_else(|| canopy.repo().common_dir.clone());
    let out = canopyd::git::Git::default().run(&cwd, ["rev-parse", "--path-format=absolute", "--git-path", "hooks"])?;
    Ok(Utf8PathBuf::from(out.trim()))
}

fn run_hook(cli: &Cli, canopy: &Canopy, command: &HookCommand) -> Result<u8> {
    match command {
        HookCommand::Install => {
            let dir = hooks_dir(canopy)?;
            let binary = std::env::current_exe()
                .ok()
                .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
                .map(|path| path.to_string())
                .unwrap_or_else(|| "canopyd".to_owned());
            let path = canopyd::hook::install(&dir, &binary)?;
            if cli.json {
                emit("hook install", &serde_json::json!({ "path": path }));
            } else {
                println!("installed {path}");
            }
        }
        HookCommand::Uninstall => {
            let dir = hooks_dir(canopy)?;
            let removed = canopyd::hook::uninstall(&dir)?;
            if cli.json {
                emit("hook uninstall", &serde_json::json!({ "removed": removed }));
            } else {
                println!("{}", if removed { "removed" } else { "nothing of ours was installed" });
            }
        }
        HookCommand::Status => {
            let dir = hooks_dir(canopy)?;
            let installed = canopyd::hook::is_installed(&dir);
            if cli.json {
                emit(
                    "hook status",
                    &serde_json::json!({ "installed": installed, "path": canopyd::hook::hook_path(&dir) }),
                );
            } else {
                println!("{}", if installed { "installed" } else { "not installed" });
            }
        }
        HookCommand::PostCheckout { old, new, flag } => {
            let cwd = canopy.repo().root.clone().unwrap_or_else(|| canopy.repo().common_dir.clone());
            let no_hook = std::env::var_os(canopyd::hook::NO_HOOK_ENV).is_some();
            let trigger = canopyd::hook::classify(old, new, flag, &cwd, no_hook);
            if cli.json {
                emit("hook post-checkout", &trigger);
            } else if let canopyd::hook::Trigger::WorktreeAdded = trigger {
                println!("canopyd: new worktree at {cwd}");
            }
            // Never anything but 0. git cannot abort a checkout, the hooks directory is shared
            // across every worktree, and a hook that fails here breaks all of them at once.
            return Ok(0);
        }
    }
    Ok(0)
}

/// Stops whatever is still running for a worktree that is about to be removed. Best effort:
/// a worktree whose config has gone, or which never started anything, must still be removable.
fn stop_services_for(canopy: &Canopy, target: &str) -> Result<Vec<String>> {
    let Ok((_, worktree, state, env, owner)) = service_context(canopy, Some(target), &[]) else {
        return Ok(Vec::new());
    };
    let default_config = canopyd::config::CanopyConfig::empty();
    let config = canopy.config().and_then(|(_, p)| p.config.as_ref()).unwrap_or(&default_config);
    let facts = owner.facts();
    let ctx = canopyd::ServiceContext { worktree: &worktree, state: &state, env: &env, facts: &facts };
    let statuses = canopyd::service::down(&config.services, None, &ctx)?;
    // The worktree is going away, so what its containers would keep for next time goes too.
    canopyd::service::purge(&config.services, &ctx);
    Ok(statuses.into_iter().map(|status| status.name).collect())
}

/// `--only a --only b` as a set, or `None` for "everything".
fn selection(only: &[String]) -> Option<std::collections::BTreeSet<String>> {
    (!only.is_empty()).then(|| only.iter().cloned().collect())
}

/// A flag that flips when the user interrupts us.
///
/// A loop that only checks between lines never notices Ctrl-C on a service that has gone quiet,
/// which is exactly when someone reaches for it. `signal_hook::flag` sets this from the handler,
/// and the loop reads it on every poll.
fn interrupt_flag() -> Result<std::sync::Arc<std::sync::atomic::AtomicBool>> {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, flag.clone()).map_err(Error::Io)?;
    }
    Ok(flag)
}

fn report_databases(cli: &Cli, command: &str, forks: &[canopyd::db::DbInstance]) {
    if cli.json {
        emit(command, &forks);
        return;
    }
    if forks.is_empty() {
        println!("no forks");
        return;
    }
    for fork in forks {
        let status = match fork.status {
            canopyd::db::ForkStatus::Ready => "ready",
            canopyd::db::ForkStatus::Missing => "missing",
        };
        println!("{:<16} {:<8} {:<8} {}", fork.name, format!("{:?}", fork.adapter).to_lowercase(), status, fork.url);
    }
}

fn report_services(cli: &Cli, command: &str, statuses: &[canopyd::ServiceStatus]) {
    if cli.json {
        emit(command, &statuses);
        return;
    }
    if statuses.is_empty() {
        println!("no services");
        return;
    }
    for status in statuses {
        let pid = status.pid.map(|pid| pid.to_string()).unwrap_or_else(|| "-".to_owned());
        let detail = status.detail.as_deref().map(|d| format!("  {d}")).unwrap_or_default();
        println!("{:<16} {:<10} {:<8}{detail}", status.name, format!("{:?}", status.state).to_lowercase(), pid);
    }
}

/// One line per step, for the human view.
fn describe_step(step: &canopyd::StepOutcome) -> String {
    match &step.result {
        canopyd::StepResult::Ran { millis } => format!("ran      {} ({millis}ms)", step.name),
        canopyd::StepResult::Skipped { reason } => format!("skipped  {} — {reason}", step.name),
        canopyd::StepResult::Failed { status, millis, .. } => {
            format!("FAILED   {} ({status}, {millis}ms)", step.name)
        }
    }
}

/// The branch to act on: the one given, else the branch of the worktree we are standing in.
fn resolve_branch(canopy: &Canopy, given: Option<&str>) -> Result<String> {
    if let Some(branch) = given {
        return Ok(branch.to_owned());
    }
    let root = canopy.repo().root.as_deref().ok_or_else(|| Error::WorktreeNotFound("(bare repository)".to_owned()))?;
    canopy
        .list()?
        .into_iter()
        .find(|entry| entry.path == root)
        .and_then(|entry| entry.branch)
        // A detached worktree has no branch to default to, so the user has to say.
        .ok_or_else(|| Error::WorktreeNotFound("the current worktree has no branch; name one".to_owned()))
}

/// Where a branch's worktree is, preferring the one that exists over the templated guess.
fn worktree_path_for_branch(canopy: &Canopy, branch: &str) -> Result<Utf8PathBuf> {
    if let Some(entry) = canopy.list()?.into_iter().find(|entry| entry.branch.as_deref() == Some(branch)) {
        return Ok(entry.path);
    }
    canopy.path_for(branch, None)
}

fn emit<T: serde::Serialize>(command: &str, data: &T) {
    let envelope = Envelope::ok(command, data, Vec::new());
    println!("{}", serde_json::to_string(&envelope).expect("envelope is serializable"));
}

fn short(head: Option<&str>) -> String {
    head.unwrap_or("unknown").chars().take(8).collect()
}

/// The process cwd as UTF-8. A non-UTF-8 cwd is rejected here rather than corrupted later.
fn current_dir() -> Result<Utf8PathBuf> {
    let dir = std::env::current_dir()?;
    Utf8PathBuf::from_path_buf(dir).map_err(|p| Error::NonUtf8Path(p.to_string_lossy().into_owned()))
}
