//! Postgres forks: one server per engine version, one database per worktree.
//!
//! A server per worktree would cost a gigabyte of memory for five branches and make forking a
//! seeded database a dump and a restore. One shared `canopy-pg-<version>` container instead makes
//! every fork `CREATE DATABASE … TEMPLATE tpl_<project>_<db>`, which Postgres does as a file-level
//! copy: a seeded two-gigabyte database forks in seconds, and dropping a fork is a `DROP`.
//!
//! Everything goes through the `docker` CLI — `run`, `exec psql`, `cp` — so there is no Postgres
//! client to install and no driver to link: the server brings its own tools. The container, its
//! port, its credentials and the template names are the ones the Canopy daemon has always used,
//! so a machine that has both sees one server and one set of templates, not two.
//!
//! - **The port is derived, not allocated.** The container outlives every `canopyd` process, so
//!   its port has to be recomputable from nothing: 16 → 54316. A container that already exists
//!   is asked what it actually published, in case somebody made it differently.
//! - **Names are made here and checked here.** A database name reaches a statement by string
//!   interpolation, because `CREATE DATABASE` takes no parameters. Every name is built from
//!   [`slug`], and [`identifier`] refuses anything else before it gets near `psql`.
//! - **Seeds travel by `docker cp`.** A custom-format dump is binary and would not survive being
//!   piped through a string.

use std::collections::BTreeMap;
use std::io::Read as _;

use camino::Utf8Path;

use super::{DbContext, DbError, DbInstance, ForkSource, ForkStatus, env_key};
use crate::config::{DatabaseSpec, DbAdapter};
use crate::container::{self, Finished};

const DEFAULT_VERSION: &str = "16";
const USER: &str = "canopy";
const PASSWORD: &str = "canopy";
const HOST: &str = "127.0.0.1";

/// How many times a new server is asked whether it is accepting connections, a second apart.
const READY_ATTEMPTS: u32 = 60;

/// `pg_dump -Fc` output starts with this. Anything else is SQL for `psql -f`.
const CUSTOM_DUMP_MAGIC: &[u8] = b"PGDMP";

// ---------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------

/// Lowercase, alphanumerics and underscores, bounded: the intersection of what engines accept
/// unquoted. Something that slugs to nothing is `x`, because an empty identifier is not one.
pub(super) fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    let bounded: String = trimmed.chars().take(max).collect();
    if bounded.is_empty() { "x".to_owned() } else { bounded }
}

/// The seed database every fork of `db` in this project is cloned from.
pub(super) fn template_name(project: &str, db: &str) -> String {
    format!("tpl_{}_{}", slug(project, 40), slug(db, 40))
}

/// One worktree's fork. The state directory is unique to a repository and a branch, which is
/// exactly what a fork belongs to, and both ends of a `--from <branch>` copy know theirs.
pub(super) fn fork_name(state: &Utf8Path, db: &str) -> String {
    format!("wt_{}_{}", container::short_hash(state.as_str()), slug(db, 40))
}

/// The guard in front of every interpolated name: a letter or underscore, then up to 62 more of
/// `[a-z0-9_]`. Our own names always pass. This is what keeps that true when a caller's do not.
fn identifier(name: &str) -> Result<&str, DbError> {
    let mut chars = name.chars();
    let head = chars.next().is_some_and(|ch| ch.is_ascii_lowercase() || ch == '_');
    let tail = chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if head && tail && name.len() <= 63 { Ok(name) } else { Err(DbError::Identifier(name.to_owned())) }
}

fn quoted(name: &str) -> Result<String, DbError> {
    Ok(format!("\"{}\"", identifier(name)?))
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn version_of(spec: &DatabaseSpec) -> String {
    let given = spec.version.as_deref().unwrap_or(DEFAULT_VERSION);
    given.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')).collect()
}

pub(super) fn server_name(version: &str) -> String {
    format!("canopy-pg-{version}")
}

/// 16 → 54316. A version that does not start with a number still gets a stable port.
pub(super) fn fixed_port(version: &str) -> u16 {
    let digits: String = version.chars().take_while(char::is_ascii_digit).collect();
    let offset = match digits.parse::<u32>() {
        Ok(major) => major % 100,
        Err(_) => version.bytes().fold(0u32, |hash, byte| (hash * 31 + u32::from(byte)) % 100),
    };
    54300 + offset as u16
}

fn url(port: u16, database: &str) -> String {
    // `postgresql://`, not `postgres://`: SQLAlchemy 2 dropped the short alias, and everything
    // that accepts the short one accepts the long one.
    format!("postgresql://{USER}:{PASSWORD}@{HOST}:{port}/{database}")
}

// ---------------------------------------------------------------------------------------
// Talking to docker
// ---------------------------------------------------------------------------------------

struct Server<'a> {
    docker: String,
    ctx: &'a DbContext<'a>,
    /// The database this is being done for, for error messages.
    name: &'a str,
    container: String,
}

impl Server<'_> {
    fn call(&self, args: &[&str]) -> Finished {
        let owned: Vec<String> = args.iter().map(|text| (*text).to_owned()).collect();
        container::call(&self.docker, &owned, self.ctx.project_path)
    }

    fn failed(&self, what: &str, finished: &Finished) -> DbError {
        DbError::Engine {
            name: self.name.to_owned(),
            detail: format!("{what}: {}", container::last_line(&finished.output)),
        }
    }

    /// One statement, and what it printed on stdout.
    fn sql(&self, database: &str, statement: &str) -> Result<String, DbError> {
        let finished = self.call(&[
            "exec",
            &self.container,
            "psql",
            "-U",
            USER,
            "-d",
            database,
            "-v",
            "ON_ERROR_STOP=1",
            "-tAc",
            statement,
        ]);
        if finished.ok { Ok(finished.stdout.trim().to_owned()) } else { Err(self.failed("psql", &finished)) }
    }

    fn exists(&self, database: &str) -> Result<bool, DbError> {
        Ok(self.sql("postgres", &format!("SELECT 1 FROM pg_database WHERE datname = {}", literal(database)))? == "1")
    }

    /// A database with a connection open can be neither dropped nor used as a template.
    fn evict(&self, database: &str) {
        let statement = format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = {} AND pid <> pg_backend_pid()",
            literal(database)
        );
        let _ = self.sql("postgres", &statement);
    }

    fn drop_database(&self, database: &str) -> Result<(), DbError> {
        self.evict(database);
        self.sql("postgres", &format!("DROP DATABASE IF EXISTS {}", quoted(database)?)).map(|_| ())
    }

    fn size(&self, database: &str) -> Option<u64> {
        self.sql("postgres", &format!("SELECT pg_database_size({})", literal(database))).ok()?.parse().ok()
    }

    fn running(&self) -> Option<bool> {
        let finished = self.call(&["inspect", "--format", "{{.State.Running}}", &self.container]);
        finished.ok.then(|| finished.stdout.trim() == "true")
    }

    /// What the container actually published 5432 as, for one that was not made by us.
    fn published_port(&self) -> Option<u16> {
        let finished = self.call(&["port", &self.container, "5432/tcp"]);
        let first = finished.stdout.lines().next()?;
        first.rsplit(':').next()?.trim().parse().ok()
    }

    /// Makes sure the server exists, is running and answers, and says which port it is on.
    fn ensure(&self, version: &str) -> Result<u16, DbError> {
        container::available(&self.docker, self.ctx.project_path)
            .map_err(|detail| DbError::Engine { name: self.name.to_owned(), detail })?;
        match self.running() {
            Some(true) => {}
            Some(false) => {
                let started = self.call(&["start", &self.container]);
                if !started.ok {
                    return Err(self.failed("docker start", &started));
                }
            }
            None => self.create(version)?,
        }
        let port = self.published_port().unwrap_or_else(|| fixed_port(version));
        self.wait_ready()?;
        Ok(port)
    }

    fn create(&self, version: &str) -> Result<(), DbError> {
        container::ensure_network(&self.docker, self.ctx.project_path)
            .map_err(|detail| DbError::Engine { name: self.name.to_owned(), detail })?;
        let publish = format!("{HOST}:{}:5432", fixed_port(version));
        // A named volume keeps templates, which are expensive to build, when the container goes.
        let volume = format!("{}-data:/var/lib/postgresql/data", self.container);
        let image = format!("postgres:{version}");
        let user = format!("POSTGRES_USER={USER}");
        let password = format!("POSTGRES_PASSWORD={PASSWORD}");
        let label = format!("{}=true", container::LABEL);
        #[rustfmt::skip]
        let created = self.call(&[
            "run", "-d", "--name", &self.container,
            "--label", &label, "--label", "canopy.database=postgres",
            "--network", container::NETWORK,
            "-e", &user, "-e", &password, "-e", "POSTGRES_DB=postgres",
            "-p", &publish, "-v", &volume, &image,
        ]);
        // Two worktrees provisioning at once both get here. The loser's container is the winner's.
        if created.ok || created.output.contains("already in use") {
            Ok(())
        } else {
            Err(self.failed("docker run", &created))
        }
    }

    fn wait_ready(&self) -> Result<(), DbError> {
        for attempt in 0..READY_ATTEMPTS {
            if self.call(&["exec", &self.container, "pg_isready", "-U", USER]).ok {
                return Ok(());
            }
            if attempt + 1 < READY_ATTEMPTS {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        Err(DbError::Engine {
            name: self.name.to_owned(),
            detail: format!("{} did not accept connections within {READY_ATTEMPTS}s", self.container),
        })
    }

    fn extensions(&self, database: &str, spec: &DatabaseSpec) -> Result<(), DbError> {
        let wanted = spec.options.get("extensions").map(String::as_str).unwrap_or_default();
        for extension in wanted.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            self.sql(database, &format!("CREATE EXTENSION IF NOT EXISTS {}", quoted(extension)?))?;
        }
        Ok(())
    }

    /// Copies a seed file into the container and replays it into `database`.
    fn load(&self, database: &str, file: &Utf8Path) -> Result<(), DbError> {
        let mut head = [0u8; 5];
        let custom = std::fs::File::open(file)
            .and_then(|mut opened| opened.read(&mut head))
            .map(|read| &head[..read] == CUSTOM_DUMP_MAGIC)
            .map_err(|source| DbError::Io { path: file.to_owned(), source })?;
        let base: String = file
            .file_name()
            .unwrap_or("seed")
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') { ch } else { '_' })
            .collect();
        let remote = format!("/tmp/canopy-seed-{base}");
        let copied = self.call(&["cp", file.as_str(), &format!("{}:{remote}", self.container)]);
        if !copied.ok {
            return Err(self.failed("docker cp", &copied));
        }
        let loaded = if custom {
            // pg_restore exits 1 for warnings — an extension comment it may not set, an owner
            // that does not exist here — having restored everything.
            let finished = self.call(&[
                "exec",
                &self.container,
                "pg_restore",
                "-U",
                USER,
                "-d",
                database,
                "--no-owner",
                "--no-privileges",
                &remote,
            ]);
            if finished.ok || finished.code == Some(1) { Ok(()) } else { Err(self.failed("pg_restore", &finished)) }
        } else {
            let finished = self.call(&[
                "exec",
                &self.container,
                "psql",
                "-U",
                USER,
                "-d",
                database,
                "-v",
                "ON_ERROR_STOP=1",
                "-f",
                &remote,
            ]);
            if finished.ok { Ok(()) } else { Err(self.failed("psql", &finished)) }
        };
        let _ = self.call(&["exec", &self.container, "rm", "-f", &remote]);
        loaded
    }

    /// Builds the project's template for `name` unless it is there already.
    fn ensure_template(&self, spec: &DatabaseSpec, port: u16, refresh: bool) -> Result<String, DbError> {
        let template = template_name(self.ctx.project, self.name);
        let exists = self.exists(&template)?;
        if exists && !refresh {
            return Ok(template);
        }
        if exists {
            self.drop_database(&template)?;
        }
        self.sql("postgres", &format!("CREATE DATABASE {}", quoted(&template)?))?;
        self.extensions(&template, spec)?;
        let built = self.seed(spec, &template, port);
        if built.is_err() {
            // Half a template would be cloned into every fork from now on. None is better.
            let _ = self.drop_database(&template);
        }
        built.map(|()| template)
    }

    fn seed(&self, spec: &DatabaseSpec, template: &str, port: u16) -> Result<(), DbError> {
        let Some(seed) = &spec.seed else { return Ok(()) };
        if let Some(file) = seed.dump.as_ref().or(seed.sql.as_ref()) {
            return self.load(template, &self.ctx.project_path.join(file));
        }
        let Some(command) = &seed.command else { return Ok(()) };
        // Migrations and seeders: run against the template with the URL where they expect it.
        let target = url(port, template);
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", command])
            .current_dir(self.ctx.project_path)
            .envs(self.ctx.env)
            .env("DATABASE_URL", &target)
            .env(env_key(self.name, spec), &target)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|source| DbError::Io { path: self.ctx.project_path.to_owned(), source })?;
        if output.status.success() {
            return Ok(());
        }
        let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
        said.push_str(&String::from_utf8_lossy(&output.stderr));
        Err(DbError::Engine {
            name: self.name.to_owned(),
            detail: format!("seed command failed: {}", container::last_line(&said)),
        })
    }
}

fn server<'a>(name: &'a str, container: String, ctx: &'a DbContext<'a>) -> Server<'a> {
    Server { docker: container::docker_bin(), ctx, name, container }
}

// ---------------------------------------------------------------------------------------
// What `db` calls
// ---------------------------------------------------------------------------------------

pub(super) fn fork(
    name: &str,
    spec: &DatabaseSpec,
    ctx: &DbContext<'_>,
    source: &ForkSource<'_>,
) -> Result<DbInstance, DbError> {
    let version = version_of(spec);
    let server = server(name, server_name(&version), ctx);
    let port = server.ensure(&version)?;
    let database = fork_name(ctx.state, name);
    server.drop_database(&database)?;

    let template = match source {
        ForkSource::Empty => None,
        ForkSource::Template => Some(server.ensure_template(spec, port, false)?),
        ForkSource::Worktree { branch, state } => {
            let theirs = fork_name(state, name);
            if !server.exists(&theirs)? {
                return Err(DbError::NoSourceFork { name: name.to_owned(), branch: (*branch).to_owned() });
            }
            Some(theirs)
        }
    };
    match &template {
        Some(template) => {
            // TEMPLATE needs its source idle, so whatever is still connected to it is evicted.
            server.evict(template);
            server
                .sql("postgres", &format!("CREATE DATABASE {} TEMPLATE {}", quoted(&database)?, quoted(template)?))?;
        }
        None => {
            server.sql("postgres", &format!("CREATE DATABASE {}", quoted(&database)?))?;
            server.extensions(&database, spec)?;
        }
    }

    let detail = BTreeMap::from(
        [
            ("container", server.container.clone()),
            ("database", database.clone()),
            ("host", HOST.to_owned()),
            ("port", port.to_string()),
            ("user", USER.to_owned()),
            ("password", PASSWORD.to_owned()),
        ]
        .map(|(key, value)| (key.to_owned(), value)),
    );
    Ok(DbInstance {
        name: name.to_owned(),
        adapter: DbAdapter::Postgres,
        status: ForkStatus::Ready,
        url: url(port, &database),
        env_key: env_key(name, spec),
        forked_from: source.label(),
        source: template,
        detail,
        size_bytes: server.size(&database),
    })
}

/// Whether a fork could be made at all right now: is there a docker to ask.
pub(super) fn preflight(name: &str, ctx: &DbContext<'_>) -> Result<(), DbError> {
    container::available(&container::docker_bin(), ctx.project_path)
        .map_err(|detail| DbError::Engine { name: name.to_owned(), detail })
}

/// Rebuilds the project's template from its seed. Existing forks are untouched; the next one
/// made from the template gets the new data.
pub(super) fn refresh_template(name: &str, spec: &DatabaseSpec, ctx: &DbContext<'_>) -> Result<String, DbError> {
    let version = version_of(spec);
    let server = server(name, server_name(&version), ctx);
    let port = server.ensure(&version)?;
    server.ensure_template(spec, port, true)
}

/// The container and database a recorded fork lives in, as the record has them. The record and
/// not the spec, so a fork whose database has left `canopy.yaml` can still be dropped.
fn recorded(instance: &DbInstance) -> Option<(String, String)> {
    Some((instance.detail.get("container")?.clone(), instance.detail.get("database")?.clone()))
}

/// Drops a fork. A server that is not running has nothing to drop, and removal must always be
/// able to finish: a worktree cannot be un-removable because docker is off.
pub(super) fn remove(instance: &DbInstance, ctx: &DbContext<'_>) -> Result<(), DbError> {
    let Some((container, database)) = recorded(instance) else { return Ok(()) };
    let server = server(&instance.name, container, ctx);
    if server.running() != Some(true) {
        return Ok(());
    }
    server.drop_database(&database)
}

/// The fork's size, or `None` when it cannot be seen: no server, or no such database.
pub(super) fn size(instance: &DbInstance, ctx: &DbContext<'_>) -> Option<u64> {
    let (container, database) = recorded(instance)?;
    let server = server(&instance.name, container, ctx);
    if server.running() != Some(true) {
        return None;
    }
    server.size(&database)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("My Project", 40, "my_project")]
    #[case("--weird--name--", 40, "weird_name")]
    #[case("", 40, "x")]
    #[case("!!!", 40, "x")]
    #[case("abcdefghij", 4, "abcd")]
    fn slugs_are_safe_unquoted_identifiers(#[case] text: &str, #[case] max: usize, #[case] expected: &str) {
        assert_eq!(slug(text, max), expected);
    }

    #[test]
    fn names_are_the_ones_the_daemon_has_always_used() {
        assert_eq!(template_name("My App", "main-db"), "tpl_my_app_main_db");
        assert_eq!(server_name("16"), "canopy-pg-16");
        let fork = fork_name(Utf8Path::new("/repo/.git/canopyd/state/feat-x"), "main-db");
        assert!(fork.starts_with("wt_") && fork.ends_with("_main_db"), "{fork}");
        assert_eq!(fork.len(), "wt_".len() + 8 + "_main_db".len());
        assert_ne!(fork, fork_name(Utf8Path::new("/repo/.git/canopyd/state/feat-y"), "main-db"));
    }

    #[rstest]
    #[case("16", 54316)]
    #[case("9.6", 54309)]
    #[case("117", 54317)]
    #[case("latest", 54300 + ("latest".bytes().fold(0u32, |h, b| (h * 31 + u32::from(b)) % 100)) as u16)]
    fn the_port_is_recomputable_from_the_version_alone(#[case] version: &str, #[case] expected: u16) {
        assert_eq!(fixed_port(version), expected);
    }

    #[rstest]
    #[case("tpl_app_main", true)]
    #[case("_x9", true)]
    #[case("Upper", false)]
    #[case("9starts_with_digit", false)]
    #[case("has-dash", false)]
    #[case("semi;colon", false)]
    #[case("quote\"d", false)]
    #[case("", false)]
    fn only_a_plain_identifier_reaches_a_statement(#[case] name: &str, #[case] ok: bool) {
        assert_eq!(identifier(name).is_ok(), ok, "{name}");
        assert_eq!(quoted(name).ok(), ok.then(|| format!("\"{name}\"")));
        let long = "a".repeat(64);
        assert!(identifier(&long).is_err(), "64 characters is one more than Postgres keeps");
        assert!(identifier(&long[..63]).is_ok());
    }

    #[test]
    fn a_literal_cannot_be_closed_from_inside() {
        assert_eq!(literal("it's"), "'it''s'");
    }

    #[test]
    fn a_version_is_cleaned_before_it_names_an_image() {
        let spec = |version: &str| -> DatabaseSpec {
            serde_saphyr::from_str(&format!("adapter: postgres\nversion: \"{version}\"\n")).expect("spec")
        };
        assert_eq!(version_of(&spec("16; rm -rf /")), "16rm-rf");
        assert_eq!(version_of(&spec("15.4-alpine")), "15.4-alpine");
        let unset: DatabaseSpec = serde_saphyr::from_str("adapter: postgres\n").expect("spec");
        assert_eq!(version_of(&unset), "16");
    }

    #[test]
    fn the_url_is_the_long_scheme_on_loopback() {
        assert_eq!(url(54316, "wt_ab_main"), "postgresql://canopy:canopy@127.0.0.1:54316/wt_ab_main");
    }
}
