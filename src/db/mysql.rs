//! MySQL forks: one server per worktree, filled from a template that is a `.sql` file.
//!
//! MySQL has no `CREATE DATABASE … TEMPLATE`, so the Postgres trick — fork a seeded database in
//! seconds — is not on offer. The next best thing is to pay for seeding once per project: the
//! template is a plain SQL file kept beside the worktrees' state, copied from the repository's
//! seed or produced by running the project's seeder against a throwaway server and dumping the
//! result. Each worktree's own server then replays that one file.
//!
//! Isolation is the container, so every fork uses the same database name inside its own. Files
//! cross the container boundary with `docker cp` in both directions: a dump can hold bytes that
//! are not text, and one that went through a string would not be the dump that was made.

use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::{DbContext, DbError, DbInstance, ForkSource, ForkStatus, env_key, postgres::slug};
use crate::config::{DatabaseSpec, DbAdapter, PortSpec};
use crate::container::{self, Finished};

const DEFAULT_VERSION: &str = "8";
const ROOT_PASSWORD: &str = "canopy";
/// The one database in every fork's server.
const DATABASE: &str = "app";
const HOST: &str = "127.0.0.1";
const REMOTE_SQL: &str = "/tmp/canopy-seed.sql";

/// A MySQL that is initialising its data directory takes a while. A second apart.
const READY_ATTEMPTS: u32 = 90;

pub(super) fn server_name(state: &Utf8Path, name: &str) -> String {
    format!("canopy-mysql-{}-{}", container::short_hash(state.as_str()), slug(name, 40))
}

fn template_server(project_path: &Utf8Path, name: &str) -> String {
    format!("canopy-mysql-tpl-{}-{}", container::short_hash(project_path.as_str()), slug(name, 40))
}

/// The project's seed, as SQL. Beside the worktrees' state rather than inside one of them,
/// because it belongs to all of them.
pub(super) fn template_file(state: &Utf8Path, name: &str) -> Utf8PathBuf {
    state.parent().unwrap_or(state).join("templates").join(format!("mysql-{name}.sql"))
}

fn image(spec: &DatabaseSpec) -> String {
    let given = spec.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let clean: String =
        given.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')).collect();
    format!("mysql:{clean}")
}

fn url(port: u16) -> String {
    format!("mysql://root:{ROOT_PASSWORD}@{HOST}:{port}/{DATABASE}")
}

fn call(ctx: &DbContext<'_>, args: &[&str]) -> Finished {
    let owned: Vec<String> = args.iter().map(|text| (*text).to_owned()).collect();
    container::call(&container::docker_bin(), &owned, ctx.project_path)
}

fn engine(name: &str, detail: impl Into<String>) -> DbError {
    DbError::Engine { name: name.to_owned(), detail: detail.into() }
}

fn said(name: &str, what: &str, finished: &Finished) -> DbError {
    engine(name, format!("{what}: {}", container::last_line(&finished.output)))
}

pub(super) fn preflight(name: &str, ctx: &DbContext<'_>) -> Result<(), DbError> {
    container::available(&container::docker_bin(), ctx.project_path).map_err(|detail| engine(name, detail))
}

fn port_for(name: &str, ctx: &DbContext<'_>) -> Result<u16, DbError> {
    let key = format!("db-{name}");
    let wanted = BTreeMap::from([(key.clone(), PortSpec::default())]);
    let parent = ctx.ports.parent().unwrap_or(ctx.ports);
    fs::create_dir_all(parent).map_err(|source| DbError::Io { path: parent.to_owned(), source })?;
    let mut registry =
        crate::ports::open(ctx.ports, ctx.ports_owner).map_err(|error| engine(name, error.to_string()))?;
    let table = registry.allocate(ctx.project, ctx.branch, &wanted).map_err(|error| engine(name, error.to_string()))?;
    Ok(table[&key])
}

/// Starts a server. `publish` is `host:port:3306`, or `host::3306` to let docker pick.
fn start(name: &str, server: &str, spec: &DatabaseSpec, publish: &str, ctx: &DbContext<'_>) -> Result<(), DbError> {
    let _ = call(ctx, &["rm", "-f", "-v", server]);
    container::ensure_network(&container::docker_bin(), ctx.project_path).map_err(|detail| engine(name, detail))?;
    let label = format!("{}=true", container::LABEL);
    let password = format!("MYSQL_ROOT_PASSWORD={ROOT_PASSWORD}");
    let database = format!("MYSQL_DATABASE={DATABASE}");
    #[rustfmt::skip]
    let started = call(ctx, &[
        "run", "-d", "--name", server,
        "--label", &label, "--label", "canopy.database=mysql",
        "--network", container::NETWORK,
        "-e", &password, "-e", &database,
        "-p", publish, &image(spec),
    ]);
    if !started.ok {
        return Err(said(name, "docker run", &started));
    }
    let ping = format!("-p{ROOT_PASSWORD}");
    for attempt in 0..READY_ATTEMPTS {
        let alive = call(ctx, &["exec", server, "mysqladmin", "ping", "-h127.0.0.1", &ping]);
        if alive.ok && alive.stdout.contains("alive") {
            return Ok(());
        }
        if attempt + 1 < READY_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    let _ = call(ctx, &["rm", "-f", "-v", server]);
    Err(engine(name, format!("{server} did not accept connections within {READY_ATTEMPTS}s")))
}

/// Replays a SQL file into a server's database.
fn load(name: &str, server: &str, file: &Utf8Path, ctx: &DbContext<'_>) -> Result<(), DbError> {
    let copied = call(ctx, &["cp", file.as_str(), &format!("{server}:{REMOTE_SQL}")]);
    if !copied.ok {
        return Err(said(name, "docker cp", &copied));
    }
    let replay = format!("mysql -uroot -p{ROOT_PASSWORD} {DATABASE} < {REMOTE_SQL}");
    let loaded = call(ctx, &["exec", server, "sh", "-c", &replay]);
    let _ = call(ctx, &["exec", server, "rm", "-f", REMOTE_SQL]);
    if loaded.ok { Ok(()) } else { Err(said(name, "mysql", &loaded)) }
}

/// Dumps a server's database to `file` on this side of the container boundary.
fn dump(name: &str, server: &str, file: &Utf8Path, ctx: &DbContext<'_>) -> Result<(), DbError> {
    let write = format!("mysqldump -uroot -p{ROOT_PASSWORD} --no-tablespaces {DATABASE} > {REMOTE_SQL}");
    let dumped = call(ctx, &["exec", server, "sh", "-c", &write]);
    if !dumped.ok {
        return Err(said(name, "mysqldump", &dumped));
    }
    let parent = file.parent().unwrap_or(file);
    fs::create_dir_all(parent).map_err(|source| DbError::Io { path: parent.to_owned(), source })?;
    let copied = call(ctx, &["cp", &format!("{server}:{REMOTE_SQL}"), file.as_str()]);
    let _ = call(ctx, &["exec", server, "rm", "-f", REMOTE_SQL]);
    if copied.ok { Ok(()) } else { Err(said(name, "docker cp", &copied)) }
}

/// Makes the project's template unless it is there. `Ok(None)` for a database with no seed.
pub(super) fn ensure_template(
    name: &str,
    spec: &DatabaseSpec,
    ctx: &DbContext<'_>,
    refresh: bool,
) -> Result<Option<Utf8PathBuf>, DbError> {
    let Some(seed) = &spec.seed else { return Ok(None) };
    let target = template_file(ctx.state, name);
    if target.is_file() && !refresh {
        return Ok(Some(target));
    }
    let parent = target.parent().unwrap_or(&target).to_owned();
    fs::create_dir_all(&parent).map_err(|source| DbError::Io { path: parent, source })?;
    if let Some(file) = seed.dump.as_ref().or(seed.sql.as_ref()) {
        let from = ctx.project_path.join(file);
        fs::copy(&from, &target).map_err(|source| DbError::Io { path: from, source })?;
        return Ok(Some(target));
    }
    let Some(command) = &seed.command else { return Ok(None) };

    // No file to copy: run the project's seeder against a throwaway server and keep the dump.
    preflight(name, ctx)?;
    let server = template_server(ctx.project_path, name);
    let built = seed_by_command(name, spec, command, &server, &target, ctx);
    let _ = call(ctx, &["rm", "-f", "-v", &server]);
    built.map(|()| Some(target))
}

fn seed_by_command(
    name: &str,
    spec: &DatabaseSpec,
    command: &str,
    server: &str,
    target: &Utf8Path,
    ctx: &DbContext<'_>,
) -> Result<(), DbError> {
    start(name, server, spec, &format!("{HOST}::3306"), ctx)?;
    let published = call(ctx, &["port", server, "3306/tcp"]);
    let port: u16 = published
        .stdout
        .lines()
        .next()
        .and_then(|line| line.rsplit(':').next())
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| engine(name, format!("{server} published no port")))?;
    let target_url = url(port);
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .current_dir(ctx.project_path)
        .envs(ctx.env)
        .env("DATABASE_URL", &target_url)
        .env("MYSQL_URL", &target_url)
        .env(env_key(name, spec), &target_url)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|source| DbError::Io { path: ctx.project_path.to_owned(), source })?;
    if !output.status.success() {
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        return Err(engine(name, format!("seed command failed: {}", container::last_line(&text))));
    }
    dump(name, server, target, ctx)
}

fn size(ctx: &DbContext<'_>, server: &str) -> Option<u64> {
    let query = format!(
        "SELECT IFNULL(SUM(data_length + index_length), 0) FROM information_schema.tables WHERE table_schema = '{DATABASE}'"
    );
    let password = format!("-p{ROOT_PASSWORD}");
    let asked = call(ctx, &["exec", server, "mysql", "-uroot", &password, "-N", "-B", "-e", &query]);
    asked.ok.then(|| asked.stdout.trim().parse::<f64>().ok()).flatten().map(|bytes| bytes as u64)
}

pub(super) fn fork(
    name: &str,
    spec: &DatabaseSpec,
    ctx: &DbContext<'_>,
    source: &ForkSource<'_>,
) -> Result<DbInstance, DbError> {
    preflight(name, ctx)?;
    // What the fork will be filled from is settled before a server is started for it, so a
    // source that is not there costs nothing.
    let filling = match source {
        ForkSource::Empty => None,
        ForkSource::Template => ensure_template(name, spec, ctx, false)?.map(|file| (file.clone().into_string(), file)),
        ForkSource::Worktree { branch, state } => {
            let theirs = server_name(state, name);
            let alive =
                call(ctx, &["exec", &theirs, "mysqladmin", "ping", "-h127.0.0.1", &format!("-p{ROOT_PASSWORD}")]);
            if !alive.ok {
                return Err(DbError::NoSourceFork { name: name.to_owned(), branch: (*branch).to_owned() });
            }
            let file = ctx.state.join(format!("mysql-{name}-from-branch.sql"));
            dump(name, &theirs, &file, ctx)?;
            Some((format!("{theirs}/{DATABASE}"), file))
        }
    };

    let port = port_for(name, ctx)?;
    let server = server_name(ctx.state, name);
    start(name, &server, spec, &format!("{HOST}:{port}:3306"), ctx)?;
    let loaded = filling.as_ref().map_or(Ok(()), |(_, file)| load(name, &server, file, ctx));
    if matches!(source, ForkSource::Worktree { .. })
        && let Some((_, file)) = &filling
    {
        let _ = fs::remove_file(file);
    }
    if let Err(error) = loaded {
        let _ = call(ctx, &["rm", "-f", "-v", &server]);
        return Err(error);
    }

    let detail = BTreeMap::from(
        [
            ("container", server.clone()),
            ("database", DATABASE.to_owned()),
            ("host", HOST.to_owned()),
            ("port", port.to_string()),
            ("user", "root".to_owned()),
            ("password", ROOT_PASSWORD.to_owned()),
        ]
        .map(|(key, value)| (key.to_owned(), value)),
    );
    Ok(DbInstance {
        name: name.to_owned(),
        adapter: DbAdapter::Mysql,
        status: ForkStatus::Ready,
        url: url(port),
        env_key: env_key(name, spec),
        forked_from: source.label(),
        source: filling.map(|(label, _)| label),
        detail,
        size_bytes: size(ctx, &server),
    })
}

/// The server *is* the fork, so removing one is removing the other, volumes included.
pub(super) fn remove(instance: &DbInstance, ctx: &DbContext<'_>) {
    if let Some(server) = instance.detail.get("container") {
        let _ = call(ctx, &["rm", "-f", "-v", server]);
    }
}

/// The fork's size when its server answers, `None` when it does not.
pub(super) fn answers(instance: &DbInstance, ctx: &DbContext<'_>) -> Option<u64> {
    let server = instance.detail.get("container")?;
    let alive = call(ctx, &["exec", server, "mysqladmin", "ping", "-h127.0.0.1", &format!("-p{ROOT_PASSWORD}")]);
    (alive.ok && alive.stdout.contains("alive")).then(|| size(ctx, server).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_say_which_worktree_which_database_and_which_project() {
        let state = Utf8Path::new("/repo/.git/canopy/worktrees/feat-a");
        let server = server_name(state, "Main DB");
        assert!(server.starts_with("canopy-mysql-") && server.ends_with("-main_db"), "{server}");
        assert_ne!(server, server_name(Utf8Path::new("/repo/.git/canopy/worktrees/feat-b"), "Main DB"));
        assert!(template_server(Utf8Path::new("/repo"), "main").starts_with("canopy-mysql-tpl-"));
        // Shared by every worktree: beside their state directories, not inside one.
        assert_eq!(template_file(state, "main"), "/repo/.git/canopy/worktrees/templates/mysql-main.sql");
    }

    #[test]
    fn the_image_and_the_url_are_what_a_client_expects() {
        let spec = |yaml: &str| -> DatabaseSpec { serde_saphyr::from_str(yaml).expect("spec") };
        assert_eq!(image(&spec("adapter: mysql\n")), "mysql:8");
        assert_eq!(image(&spec("adapter: mysql\nversion: \"8.4; echo\"\n")), "mysql:8.4echo");
        assert_eq!(url(19021), "mysql://root:canopy@127.0.0.1:19021/app");
    }
}
