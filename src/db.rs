//! Database forks: each worktree gets its own copy of every database `canopy.yaml` declares.
//!
//! A branch that shares a database with `main` is not isolated, whatever else is. A migration
//! run on the branch is a migration run on `main`, and the dev server that was working a minute
//! ago now is not. A fork is the fix: a private copy made when the worktree is, thrown away when
//! it goes.
//!
//! Three decisions carry the module:
//!
//! - **A fork is recorded, not rediscovered.** `git worktree list` cannot know a worktree has a
//!   database, so this is one of the few things persisted: `databases.json` in the worktree's
//!   state directory says what was forked, from what, and the URL it answers on. Everything that
//!   needs the URL — the environment, `${db.main.url}`, a caller — reads the record.
//! - **The record is believed only as far as the fork can be seen.** A recorded fork whose file
//!   is gone reports `missing`, and its URL is kept out of the environment: a service started
//!   against a database that is not there fails in a way that points everywhere but here.
//! - **All of them or none.** Whether each fork *can* be made is asked before any is: a server
//!   database that was passed over because docker is off would leave the worktree pointed at
//!   whatever `DATABASE_URL` the shell had — the shared database, exactly what the fork exists
//!   to prevent — beside a SQLite fork that makes everything look provisioned.
//!
//! Four engines, three shapes. SQLite is a file, so a fork is a copy. Postgres can clone a
//! database inside a server, so there is one server and a template. MySQL and Redis can do
//! neither, so each fork is a small server of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::config::{DatabaseSpec, DbAdapter};
use crate::error::ErrorCode;

mod mysql;
mod postgres;
mod redis;

/// The record of what was forked, inside a worktree's state directory.
const REGISTRY: &str = "databases.json";

/// Where file-backed forks live, inside a worktree's state directory.
const FORKS_DIR: &str = "db";

/// What travels with a SQLite file. A database copied mid-WAL without them loses its most
/// recent commits, and one that arrives with a stale `-shm` from a previous fork is corrupt.
const SIDECARS: [&str; 2] = ["-wal", "-shm"];

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("no database named {0} in canopy.yaml")]
    Unknown(String),
    #[error("database {name}: {branch} has no fork of it to copy")]
    NoSourceFork { name: String, branch: String },
    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a readable fork record: {detail}")]
    Registry { path: Utf8PathBuf, detail: String },
    /// The engine, or docker on its behalf, said no. `detail` is what it said.
    #[error("database {name}: {detail}")]
    Engine { name: String, detail: String },
    #[error("{0:?} is not a name that can be used for a database")]
    Identifier(String),
}

impl DbError {
    pub fn code(&self) -> ErrorCode {
        match self {
            DbError::Unknown(_) => ErrorCode::ConfigInvalid,
            DbError::NoSourceFork { .. }
            | DbError::Registry { .. }
            | DbError::Engine { .. }
            | DbError::Identifier(_) => ErrorCode::DbFailed,
            DbError::Io { .. } => ErrorCode::Io,
        }
    }
}

/// What a fork starts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkSource<'a> {
    /// The project's seed: for SQLite, the file `source:` names in the primary checkout.
    Template,
    Empty,
    /// Another worktree's fork, with the data somebody has been working against.
    Worktree {
        branch: &'a str,
        state: &'a Utf8Path,
    },
}

impl ForkSource<'_> {
    pub(crate) fn label(&self) -> String {
        match self {
            ForkSource::Template => "seed template".to_owned(),
            ForkSource::Empty => "empty".to_owned(),
            ForkSource::Worktree { branch, .. } => format!("worktree {branch}"),
        }
    }
}

/// Where a worktree's forks live and where its seeds come from.
#[derive(Debug, Clone, Copy)]
pub struct DbContext<'a> {
    /// The primary checkout. Seed files are named relative to it, so every worktree forks the
    /// same seed whatever its own checkout has done to the file.
    pub project_path: &'a Utf8Path,
    /// What the project is called. A server's template is shared by every worktree of a project
    /// and named after it.
    pub project: &'a str,
    /// This worktree's state directory.
    pub state: &'a Utf8Path,
    /// The worktree's resolved environment, for a seed `command:`.
    pub env: &'a BTreeMap<String, String>,
    /// The worktree's branch and the port registry: a fork that is its own server takes a port
    /// from the same place the worktree's services do, so it is stable and handed back with them.
    pub branch: &'a str,
    pub ports: &'a Utf8Path,
}

/// Whether a recorded fork is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkStatus {
    Ready,
    /// Recorded, and gone: somebody deleted the file, or the state directory was restored from
    /// somewhere it was not.
    Missing,
}

/// One fork, as recorded and as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbInstance {
    pub name: String,
    pub adapter: DbAdapter,
    pub status: ForkStatus,
    /// What a client connects to. For SQLite, `file:<absolute path>`.
    pub url: String,
    /// The environment variable that carries `url`: `env:` from the spec, else `<NAME>_URL`.
    pub env_key: String,
    /// `seed template`, `empty` or `worktree <branch>` — what was asked for.
    pub forked_from: String,
    /// What it was actually copied from, or `None` when it started empty — which a `template`
    /// fork does when the project has no seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Adapter-specific fields, reachable as `${db.<name>.<field>}`. SQLite has `file`.
    pub detail: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

impl DbInstance {
    /// Everything `${db.<name>…}` can reach: the bare name and `.url` are the URL, and every
    /// detail is a field.
    pub fn fields(&self) -> BTreeMap<String, String> {
        let mut out = self.detail.clone();
        out.insert("url".to_owned(), self.url.clone());
        out
    }
}

/// `main-db` → `MAIN_DB_URL`, unless the spec names its own variable.
pub fn env_key(name: &str, spec: &DatabaseSpec) -> String {
    match &spec.env {
        Some(key) => key.clone(),
        None => format!("{}_URL", crate::env::upper_snake(name)),
    }
}

// ---------------------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------------------

fn registry_path(state: &Utf8Path) -> Utf8PathBuf {
    state.join(REGISTRY)
}

/// The forks recorded for a worktree, as written. No record means no forks.
fn read_registry(state: &Utf8Path) -> Result<Vec<DbInstance>, DbError> {
    let path = registry_path(state);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(DbError::Io { path, source }),
    };
    serde_json::from_str(&text).map_err(|error| DbError::Registry { path, detail: error.to_string() })
}

/// Replaces the record. Written beside itself and renamed, so a reader never sees half a file
/// and a crash leaves the old record rather than none.
fn write_registry(state: &Utf8Path, instances: &[DbInstance]) -> Result<(), DbError> {
    let path = registry_path(state);
    let staged = state.join(format!("{REGISTRY}.tmp"));
    fs::create_dir_all(state).map_err(|source| DbError::Io { path: state.to_owned(), source })?;
    let text = serde_json::to_string_pretty(instances).expect("instances are serializable");
    fs::write(&staged, text).map_err(|source| DbError::Io { path: staged.clone(), source })?;
    fs::rename(&staged, &path).map_err(|source| DbError::Io { path, source })
}

// ---------------------------------------------------------------------------------------
// What a caller does
// ---------------------------------------------------------------------------------------

/// The declared databases this call is about: all of them, or `only`, each of which must exist.
fn selection<'a>(
    databases: &'a BTreeMap<String, DatabaseSpec>,
    only: Option<&BTreeSet<String>>,
) -> Result<Vec<(&'a String, &'a DatabaseSpec)>, DbError> {
    if let Some(names) = only {
        for name in names {
            if !databases.contains_key(name) {
                return Err(DbError::Unknown(name.clone()));
            }
        }
    }
    Ok(databases.iter().filter(|(name, _)| only.is_none_or(|names| names.contains(*name))).collect())
}

/// Whether a fork of `spec` could be made at all right now. Asked for every selected database
/// before any of them is touched, so a machine with docker off forks nothing rather than half,
/// and a `reset` does not drop a working fork it then cannot replace.
fn preflight(name: &str, spec: &DatabaseSpec, ctx: &DbContext<'_>) -> Result<(), DbError> {
    match spec.adapter {
        DbAdapter::Sqlite => Ok(()),
        DbAdapter::Postgres => postgres::preflight(name, ctx),
        DbAdapter::Mysql => mysql::preflight(name, ctx),
        DbAdapter::Redis => redis::preflight(name, ctx),
    }
}

/// Forks every selected database that does not already have a fork, and reports all of them.
///
/// Idempotent: a database that is already forked and still there is left exactly as it is — the
/// data in it is somebody's work. [`reset`] is how to ask for a fresh one.
pub fn fork(
    databases: &BTreeMap<String, DatabaseSpec>,
    only: Option<&BTreeSet<String>>,
    ctx: &DbContext<'_>,
    source: &ForkSource<'_>,
) -> Result<Vec<DbInstance>, DbError> {
    let chosen = selection(databases, only)?;
    let mut recorded = read_registry(ctx.state)?;
    for (name, spec) in &chosen {
        // Only for what is about to be made: a fork that is there needs nothing from docker.
        let there = recorded
            .iter()
            .any(|instance| &instance.name == *name && observe(instance, ctx, true).status == ForkStatus::Ready);
        if !there {
            preflight(name, spec, ctx)?;
        }
    }
    let mut out = Vec::with_capacity(chosen.len());
    for (name, spec) in chosen {
        let existing =
            recorded.iter().find(|instance| &instance.name == name).map(|instance| observe(instance, ctx, true));
        let instance = match existing {
            Some(instance) if instance.status == ForkStatus::Ready => instance,
            _ => match spec.adapter {
                DbAdapter::Sqlite => sqlite_fork(name, spec, ctx, source)?,
                DbAdapter::Postgres => postgres::fork(name, spec, ctx, source)?,
                DbAdapter::Mysql => mysql::fork(name, spec, ctx, source)?,
                DbAdapter::Redis => redis::fork(name, spec, ctx, source)?,
            },
        };
        recorded.retain(|other| other.name != instance.name);
        recorded.push(instance.clone());
        out.push(instance);
    }
    recorded.sort_by(|a, b| a.name.cmp(&b.name));
    write_registry(ctx.state, &recorded)?;
    Ok(out)
}

/// Every recorded fork, re-checked against the disk rather than trusted from the record.
pub fn list(ctx: &DbContext<'_>) -> Result<Vec<DbInstance>, DbError> {
    Ok(read_registry(ctx.state)?.iter().map(|instance| observe(instance, ctx, true)).collect())
}

/// Rebuilds the seed template of every selected database that has one, and returns what was
/// rebuilt. Forks that exist are untouched; the next one made from the template is new.
///
/// SQLite has nothing to rebuild: its template *is* the seed file, read at fork time.
pub fn refresh_templates(
    databases: &BTreeMap<String, DatabaseSpec>,
    only: Option<&BTreeSet<String>>,
    ctx: &DbContext<'_>,
) -> Result<Vec<String>, DbError> {
    let mut rebuilt = Vec::new();
    for (name, spec) in selection(databases, only)? {
        match spec.adapter {
            DbAdapter::Postgres => rebuilt.push(postgres::refresh_template(name, spec, ctx)?),
            DbAdapter::Mysql => {
                rebuilt.extend(mysql::ensure_template(name, spec, ctx, true)?.map(Utf8PathBuf::into_string));
            }
            // SQLite's template is its seed file, and a Redis has none.
            DbAdapter::Sqlite | DbAdapter::Redis => {}
        }
    }
    Ok(rebuilt)
}

/// Throws a fork away and makes it again from `source`.
pub fn reset(
    databases: &BTreeMap<String, DatabaseSpec>,
    name: &str,
    ctx: &DbContext<'_>,
    source: &ForkSource<'_>,
) -> Result<DbInstance, DbError> {
    let only = BTreeSet::from([name.to_owned()]);
    // Everything that can be known to fail is asked first. Dropping the fork and *then* finding
    // docker is off leaves a worktree with no database where it had a working one.
    for (name, spec) in selection(databases, Some(&only))? {
        preflight(name, spec, ctx)?;
    }
    drop_forks(databases, Some(&only), ctx)?;
    let mut forked = fork(databases, Some(&only), ctx, source)?;
    // Asked for one declared database by name, so one instance comes back.
    Ok(forked.remove(0))
}

/// Removes the selected forks and their records, and returns the names that had one.
///
/// A recorded fork whose database is no longer declared is removed too when nothing is
/// selected: the config moved on, and the file it left behind is nobody's.
pub fn drop_forks(
    databases: &BTreeMap<String, DatabaseSpec>,
    only: Option<&BTreeSet<String>>,
    ctx: &DbContext<'_>,
) -> Result<Vec<String>, DbError> {
    selection(databases, only)?;
    let recorded = read_registry(ctx.state)?;
    let (going, staying): (Vec<DbInstance>, Vec<DbInstance>) =
        recorded.into_iter().partition(|instance| only.is_none_or(|names| names.contains(&instance.name)));
    for instance in &going {
        match instance.adapter {
            DbAdapter::Sqlite => sqlite_remove(&fork_file(ctx.state, &instance.name))?,
            DbAdapter::Postgres => postgres::remove(instance, ctx)?,
            DbAdapter::Mysql => mysql::remove(instance, ctx),
            DbAdapter::Redis => redis::remove(instance, ctx)?,
        }
    }
    write_registry(ctx.state, &staying)?;
    Ok(going.into_iter().map(|instance| instance.name).collect())
}

/// The forks a worktree's environment may point at: recorded *and* present.
///
/// "Present" is checked where checking is a `stat`. A server fork is taken at its record's word
/// here: this runs for every command that resolves an environment, and asking docker each time
/// would put a process spawn in front of `canopyd env`. `db ls` is the one that asks.
pub fn ready(ctx: &DbContext<'_>) -> Result<Vec<DbInstance>, DbError> {
    let recorded = read_registry(ctx.state)?;
    Ok(recorded
        .iter()
        .map(|instance| observe(instance, ctx, false))
        .filter(|instance| instance.status == ForkStatus::Ready)
        .collect())
}

/// A record, corrected for what is actually there. `probe` allows the expensive question.
fn observe(instance: &DbInstance, ctx: &DbContext<'_>, probe: bool) -> DbInstance {
    if instance.adapter != DbAdapter::Sqlite {
        if !probe {
            return DbInstance { status: ForkStatus::Ready, ..instance.clone() };
        }
        // `Some(size)` when the fork answers; a Redis answers without having a size worth the name.
        let seen = match instance.adapter {
            DbAdapter::Postgres => postgres::size(instance, ctx).map(Some),
            DbAdapter::Mysql => mysql::answers(instance, ctx).map(Some),
            DbAdapter::Redis | DbAdapter::Sqlite => redis::answers(instance, ctx).then_some(None),
        };
        return DbInstance {
            status: if seen.is_some() { ForkStatus::Ready } else { ForkStatus::Missing },
            size_bytes: seen.flatten(),
            ..instance.clone()
        };
    }
    let size = file_size(&fork_file(ctx.state, &instance.name));
    DbInstance {
        status: if size.is_some() { ForkStatus::Ready } else { ForkStatus::Missing },
        size_bytes: size,
        ..instance.clone()
    }
}

// ---------------------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------------------

fn fork_file(state: &Utf8Path, name: &str) -> Utf8PathBuf {
    state.join(FORKS_DIR).join(format!("{name}.db"))
}

fn file_size(path: &Utf8Path) -> Option<u64> {
    fs::metadata(path).ok().filter(|meta| meta.is_file()).map(|meta| meta.len())
}

/// Removes a database file and whatever travels with it. Already gone is success.
fn sqlite_remove(file: &Utf8Path) -> Result<(), DbError> {
    let mut paths = vec![file.to_owned()];
    paths.extend(SIDECARS.iter().map(|suffix| Utf8PathBuf::from(format!("{file}{suffix}"))));
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(DbError::Io { path, source }),
        }
    }
    Ok(())
}

fn sqlite_fork(
    name: &str,
    spec: &DatabaseSpec,
    ctx: &DbContext<'_>,
    source: &ForkSource<'_>,
) -> Result<DbInstance, DbError> {
    let target = fork_file(ctx.state, name);
    let dir = ctx.state.join(FORKS_DIR);
    fs::create_dir_all(&dir).map_err(|source| DbError::Io { path: dir.clone(), source })?;
    // A leftover `-wal` beside a fresh copy would be replayed into it.
    sqlite_remove(&target)?;

    let from = match source {
        ForkSource::Empty => None,
        ForkSource::Template => spec.source.as_ref().map(|relative| ctx.project_path.join(relative)),
        ForkSource::Worktree { branch, state } => {
            let theirs = fork_file(state, name);
            if file_size(&theirs).is_none() {
                return Err(DbError::NoSourceFork { name: name.to_owned(), branch: (*branch).to_owned() });
            }
            Some(theirs)
        }
    };
    // A seed the config names and the checkout does not have is a project with no seed yet, not
    // a failure: the fork starts empty, and `source` on the instance says so.
    let copied = match from.filter(|path| file_size(path).is_some()) {
        Some(path) => {
            fs::copy(&path, &target).map_err(|source| DbError::Io { path: target.clone(), source })?;
            for suffix in SIDECARS {
                let sidecar = Utf8PathBuf::from(format!("{path}{suffix}"));
                if file_size(&sidecar).is_some() {
                    let to = Utf8PathBuf::from(format!("{target}{suffix}"));
                    fs::copy(&sidecar, &to).map_err(|source| DbError::Io { path: to.clone(), source })?;
                }
            }
            Some(path.into_string())
        }
        None => {
            fs::write(&target, b"").map_err(|source| DbError::Io { path: target.clone(), source })?;
            None
        }
    };

    Ok(DbInstance {
        name: name.to_owned(),
        adapter: DbAdapter::Sqlite,
        status: ForkStatus::Ready,
        url: format!("file:{target}"),
        env_key: env_key(name, spec),
        forked_from: source.label(),
        source: copied,
        detail: BTreeMap::from([("file".to_owned(), target.to_string())]),
        size_bytes: file_size(&target),
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    static NO_ENV: BTreeMap<String, String> = BTreeMap::new();

    struct Harness {
        _dir: TempDir,
        project: Utf8PathBuf,
        state: Utf8PathBuf,
        other_state: Utf8PathBuf,
        ports: Utf8PathBuf,
    }

    impl Harness {
        fn new() -> Harness {
            let dir = TempDir::new().expect("temp dir");
            let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf-8 temp dir");
            let project = root.join("project");
            fs::create_dir_all(project.join("data")).expect("project");
            let ports = root.join("state/ports.json");
            Harness {
                _dir: dir,
                project,
                state: root.join("state/feat-x"),
                other_state: root.join("state/main"),
                ports,
            }
        }

        fn ctx(&self) -> DbContext<'_> {
            DbContext {
                project_path: &self.project,
                project: "demo",
                state: &self.state,
                env: &NO_ENV,
                branch: "feat/x",
                ports: &self.ports,
            }
        }

        fn other(&self) -> DbContext<'_> {
            DbContext {
                project_path: &self.project,
                project: "demo",
                state: &self.other_state,
                env: &NO_ENV,
                branch: "main",
                ports: &self.ports,
            }
        }

        fn seed(&self, relative: &str, bytes: &[u8]) {
            fs::write(self.project.join(relative), bytes).expect("seed");
        }
    }

    fn databases(yaml: &str) -> BTreeMap<String, DatabaseSpec> {
        serde_saphyr::from_str(yaml).expect("test databases parse")
    }

    fn one_sqlite() -> BTreeMap<String, DatabaseSpec> {
        databases("main:\n  adapter: sqlite\n  source: data/seed.db\n")
    }

    fn only(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_template_fork_is_a_copy_of_the_seed_with_its_sidecars() {
        let harness = Harness::new();
        harness.seed("data/seed.db", b"seed-bytes");
        harness.seed("data/seed.db-wal", b"wal-bytes");

        let forked = fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Template).expect("fork");

        let file = harness.state.join("db/main.db");
        assert_eq!(fs::read(&file).expect("fork file"), b"seed-bytes");
        assert_eq!(fs::read(format!("{file}-wal")).expect("wal travels"), b"wal-bytes");
        assert!(!Utf8PathBuf::from(format!("{file}-shm")).exists(), "a sidecar the seed lacks is not invented");
        assert_eq!(
            forked,
            [DbInstance {
                name: "main".to_owned(),
                adapter: DbAdapter::Sqlite,
                status: ForkStatus::Ready,
                url: format!("file:{file}"),
                env_key: "MAIN_URL".to_owned(),
                forked_from: "seed template".to_owned(),
                source: Some(harness.project.join("data/seed.db").into_string()),
                detail: BTreeMap::from([("file".to_owned(), file.to_string())]),
                size_bytes: Some(10),
            }]
        );
    }

    #[rstest]
    #[case::asked_for_empty(ForkSource::Empty, true)]
    #[case::no_seed_in_the_checkout(ForkSource::Template, false)]
    fn a_fork_with_nothing_to_copy_starts_empty_and_says_so(#[case] source: ForkSource<'static>, #[case] seeded: bool) {
        let harness = Harness::new();
        if seeded {
            harness.seed("data/seed.db", b"ignored when empty is asked for");
        }

        let forked = fork(&one_sqlite(), None, &harness.ctx(), &source).expect("fork");

        assert_eq!(forked[0].source, None, "nothing was copied, and the instance says so");
        assert_eq!(forked[0].size_bytes, Some(0));
        assert_eq!(forked[0].status, ForkStatus::Ready);
        assert_eq!(fs::read(harness.state.join("db/main.db")).expect("fork file"), b"");
    }

    #[test]
    fn a_database_without_a_source_forks_empty_from_the_template() {
        let harness = Harness::new();
        let forked =
            fork(&databases("cache:\n  adapter: sqlite\n"), None, &harness.ctx(), &ForkSource::Template).expect("fork");
        assert_eq!(forked[0].source, None);
        assert_eq!(forked[0].forked_from, "seed template");
    }

    #[test]
    fn a_fork_can_be_copied_from_another_worktrees_fork() {
        let harness = Harness::new();
        harness.seed("data/seed.db", b"seed");
        fork(&one_sqlite(), None, &harness.other(), &ForkSource::Template).expect("main's fork");
        // Somebody worked against main's fork. That is the data the new worktree wants.
        fs::write(harness.other_state.join("db/main.db"), b"worked-on").expect("work");

        let source = ForkSource::Worktree { branch: "main", state: &harness.other_state };
        let forked = fork(&one_sqlite(), None, &harness.ctx(), &source).expect("fork");

        assert_eq!(fs::read(harness.state.join("db/main.db")).expect("fork file"), b"worked-on");
        assert_eq!(forked[0].forked_from, "worktree main");
        assert_eq!(forked[0].source, Some(harness.other_state.join("db/main.db").into_string()));
    }

    #[test]
    fn copying_from_a_worktree_with_no_fork_is_an_error_not_an_empty_database() {
        let harness = Harness::new();
        let source = ForkSource::Worktree { branch: "main", state: &harness.other_state };

        let error = fork(&one_sqlite(), None, &harness.ctx(), &source).expect_err("no source fork");

        assert_eq!(error.to_string(), "database main: main has no fork of it to copy");
        assert_eq!(error.code(), ErrorCode::DbFailed);
        assert!(list(&harness.ctx()).expect("list").is_empty(), "nothing was recorded");
    }

    #[test]
    fn forking_again_leaves_the_data_somebody_is_working_against_alone() {
        let harness = Harness::new();
        harness.seed("data/seed.db", b"seed");
        fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Template).expect("fork");
        fs::write(harness.state.join("db/main.db"), b"an afternoon of work").expect("work");

        let again = fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Template).expect("fork again");

        assert_eq!(fs::read(harness.state.join("db/main.db")).expect("fork file"), b"an afternoon of work");
        assert_eq!(again[0].size_bytes, Some(20), "and the report is of what is there now");
    }

    #[test]
    fn a_fork_that_has_gone_missing_is_reported_and_made_again() {
        let harness = Harness::new();
        harness.seed("data/seed.db", b"seed");
        fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Template).expect("fork");
        fs::remove_file(harness.state.join("db/main.db")).expect("lose it");

        let listed = list(&harness.ctx()).expect("list");
        assert_eq!(listed[0].status, ForkStatus::Missing);
        assert_eq!(listed[0].size_bytes, None);
        assert!(ready(&harness.ctx()).expect("ready").is_empty(), "a missing fork must not reach the environment");

        let again = fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Template).expect("fork again");
        assert_eq!(again[0].status, ForkStatus::Ready);
        assert_eq!(fs::read(harness.state.join("db/main.db")).expect("fork file"), b"seed");
    }

    #[test]
    fn reset_throws_the_fork_away_and_forks_again() {
        let harness = Harness::new();
        harness.seed("data/seed.db", b"seed");
        fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Template).expect("fork");
        fs::write(harness.state.join("db/main.db"), b"a mess").expect("work");
        fs::write(harness.state.join("db/main.db-wal"), b"stale").expect("stale wal");

        let fresh = reset(&one_sqlite(), "main", &harness.ctx(), &ForkSource::Template).expect("reset");

        assert_eq!(fresh.name, "main");
        assert_eq!(fs::read(harness.state.join("db/main.db")).expect("fork file"), b"seed");
        assert!(!harness.state.join("db/main.db-wal").exists(), "a stale wal would be replayed into the copy");
    }

    #[test]
    fn drop_removes_the_files_and_the_record_and_only_what_was_selected() {
        let harness = Harness::new();
        let both = databases("cache:\n  adapter: sqlite\nmain:\n  adapter: sqlite\n");
        fork(&both, None, &harness.ctx(), &ForkSource::Empty).expect("fork");

        let dropped = drop_forks(&both, Some(&only(&["cache"])), &harness.ctx()).expect("drop");

        assert_eq!(dropped, ["cache"]);
        assert!(!harness.state.join("db/cache.db").exists());
        let left: Vec<String> = list(&harness.ctx()).expect("list").into_iter().map(|i| i.name).collect();
        assert_eq!(left, ["main"]);

        // Dropping everything takes a fork whose database the config no longer declares, too.
        let dropped = drop_forks(&BTreeMap::new(), None, &harness.ctx()).expect("drop all");
        assert_eq!(dropped, ["main"]);
        assert!(list(&harness.ctx()).expect("list").is_empty());
        // And doing it again is not an error.
        assert!(drop_forks(&BTreeMap::new(), None, &harness.ctx()).expect("drop nothing").is_empty());
    }

    #[test]
    fn only_restricts_a_fork_and_an_unknown_name_is_an_error() {
        let harness = Harness::new();
        let both = databases("cache:\n  adapter: sqlite\nmain:\n  adapter: sqlite\n");

        let forked = fork(&both, Some(&only(&["main"])), &harness.ctx(), &ForkSource::Empty).expect("fork");
        assert_eq!(forked.len(), 1);
        assert!(!harness.state.join("db/cache.db").exists());

        let error = fork(&both, Some(&only(&["nope"])), &harness.ctx(), &ForkSource::Empty).expect_err("unknown");
        assert_eq!(error.to_string(), "no database named nope in canopy.yaml");
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
        let error = drop_forks(&both, Some(&only(&["nope"])), &harness.ctx()).expect_err("unknown");
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    }

    #[test]
    fn the_env_key_is_the_specs_own_or_derived_from_the_name() {
        let named = databases("main-db:\n  adapter: sqlite\n  env: DATABASE_URL\nmain-db-2:\n  adapter: sqlite\n");
        assert_eq!(env_key("main-db", &named["main-db"]), "DATABASE_URL");
        assert_eq!(env_key("main-db-2", &named["main-db-2"]), "MAIN_DB_2_URL");
    }

    #[test]
    fn fields_are_the_url_and_every_detail() {
        let harness = Harness::new();
        let forked = fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Empty).expect("fork");
        let fields = forked[0].fields();
        assert_eq!(fields["url"], forked[0].url);
        assert_eq!(fields["file"], harness.state.join("db/main.db").to_string());
    }

    #[test]
    fn a_record_that_will_not_parse_is_an_error_not_an_empty_list() {
        let harness = Harness::new();
        fs::create_dir_all(&harness.state).expect("state");
        fs::write(harness.state.join(REGISTRY), "{ not json").expect("corrupt");

        let error = list(&harness.ctx()).expect_err("corrupt");

        assert!(error.to_string().contains("is not a readable fork record"), "{error}");
        assert_eq!(error.code(), ErrorCode::DbFailed);
    }

    #[test]
    fn a_record_that_cannot_be_read_or_written_is_an_io_error() {
        let harness = Harness::new();
        // A directory where the record should be: reading it fails, and so does replacing it.
        fs::create_dir_all(harness.state.join(REGISTRY)).expect("squat");
        let error = list(&harness.ctx()).expect_err("unreadable");
        assert_eq!(error.code(), ErrorCode::Io);

        let harness = Harness::new();
        // A file where the state directory should be: nothing can be written under it.
        fs::create_dir_all(harness.state.parent().expect("parent")).expect("parent");
        fs::write(&harness.state, "in the way").expect("squat");
        let error = fork(&one_sqlite(), None, &harness.ctx(), &ForkSource::Empty).expect_err("unwritable");
        assert_eq!(error.code(), ErrorCode::Io);
    }
}
