//! Redis forks: one small server per worktree.
//!
//! Redis has no cheap way to fork a database inside a server. Its numbered databases share
//! memory, eviction and `FLUSHALL`, so two worktrees on one server are not isolated in any way
//! that matters. A container of its own costs a few megabytes and gives each worktree real
//! isolation, its own port, and an append-only directory that *is* the database — which makes
//! cloning another worktree's state a directory copy made before the server starts.
//!
//! There is no template: a fresh fork is an empty server. The port comes from the same registry
//! as the worktree's other ports, under `db-<name>`, so it is stable for the life of the branch
//! and handed back with the rest when the worktree goes.

use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::{DbContext, DbError, DbInstance, ForkSource, ForkStatus, env_key, postgres::slug};
use crate::config::{DatabaseSpec, DbAdapter, PortSpec};
use crate::container::{self, Finished};

const DEFAULT_VERSION: &str = "7";
const HOST: &str = "127.0.0.1";

/// How many times a new server is pinged, half a second apart.
const READY_ATTEMPTS: u32 = 30;

fn data_dir(state: &Utf8Path, name: &str) -> Utf8PathBuf {
    state.join("redis").join(name)
}

pub(super) fn server_name(state: &Utf8Path, name: &str) -> String {
    format!("canopy-redis-{}-{}", container::short_hash(state.as_str()), slug(name, 40))
}

fn image(spec: &DatabaseSpec) -> String {
    let given = spec.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let clean: String =
        given.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')).collect();
    format!("redis:{clean}")
}

fn call(ctx: &DbContext<'_>, args: &[&str]) -> Finished {
    let owned: Vec<String> = args.iter().map(|text| (*text).to_owned()).collect();
    container::call(&container::docker_bin(), &owned, ctx.project_path)
}

fn engine(name: &str, detail: impl Into<String>) -> DbError {
    DbError::Engine { name: name.to_owned(), detail: detail.into() }
}

pub(super) fn preflight(name: &str, ctx: &DbContext<'_>) -> Result<(), DbError> {
    container::available(&container::docker_bin(), ctx.project_path).map_err(|detail| engine(name, detail))
}

/// This database's port for this branch: allocated once, the same number ever after.
fn port_for(name: &str, ctx: &DbContext<'_>) -> Result<u16, DbError> {
    let key = format!("db-{name}");
    let wanted = BTreeMap::from([(key.clone(), PortSpec::default())]);
    let parent = ctx.ports.parent().unwrap_or(ctx.ports);
    fs::create_dir_all(parent).map_err(|source| DbError::Io { path: parent.to_owned(), source })?;
    let mut registry = crate::ports::Registry::load(ctx.ports).map_err(|error| engine(name, error.to_string()))?;
    let table = registry.allocate(ctx.project, ctx.branch, &wanted).map_err(|error| engine(name, error.to_string()))?;
    Ok(table[&key])
}

/// Copies a directory of plain files, which is what an append-only directory is.
fn copy_dir(from: &Utf8Path, to: &Utf8Path) -> Result<(), DbError> {
    let io = |path: &Utf8Path| {
        let path = path.to_owned();
        move |source| DbError::Io { path, source }
    };
    fs::create_dir_all(to).map_err(io(to))?;
    for entry in fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let Some(file) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let (source, target) = (from.join(&file), to.join(&file));
        if entry.file_type().map_err(io(&source))?.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(io(&target))?;
        }
    }
    Ok(())
}

fn alive(ctx: &DbContext<'_>, server: &str) -> bool {
    let pong = call(ctx, &["exec", server, "redis-cli", "ping"]);
    pong.ok && pong.stdout.trim().eq_ignore_ascii_case("PONG")
}

pub(super) fn fork(
    name: &str,
    spec: &DatabaseSpec,
    ctx: &DbContext<'_>,
    source: &ForkSource<'_>,
) -> Result<DbInstance, DbError> {
    preflight(name, ctx)?;
    let port = port_for(name, ctx)?;
    let server = server_name(ctx.state, name);
    let dir = data_dir(ctx.state, name);

    // Whatever served this database before is stopped first: it has the directory open.
    let _ = call(ctx, &["rm", "-f", "-v", &server]);
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(DbError::Io { path: dir, source }),
    }
    let copied = match source {
        ForkSource::Worktree { branch, state } => {
            let theirs = data_dir(state, name);
            if !theirs.is_dir() {
                return Err(DbError::NoSourceFork { name: name.to_owned(), branch: (*branch).to_owned() });
            }
            // Before the server starts, because Redis loads what it finds at boot and nothing later.
            copy_dir(&theirs, &dir)?;
            Some(theirs.into_string())
        }
        ForkSource::Template | ForkSource::Empty => {
            fs::create_dir_all(&dir).map_err(|source| DbError::Io { path: dir.clone(), source })?;
            None
        }
    };

    container::ensure_network(&container::docker_bin(), ctx.project_path).map_err(|detail| engine(name, detail))?;
    let publish = format!("{HOST}:{port}:6379");
    let mount = format!("{dir}:/data");
    let label = format!("{}=true", container::LABEL);
    #[rustfmt::skip]
    let started = call(ctx, &[
        "run", "-d", "--name", &server,
        "--label", &label, "--label", "canopy.database=redis",
        "--network", container::NETWORK,
        "-p", &publish, "-v", &mount, &image(spec),
        "redis-server", "--appendonly", "yes", "--dir", "/data",
    ]);
    if !started.ok {
        return Err(engine(name, format!("docker run: {}", container::last_line(&started.output))));
    }
    let mut ready = false;
    for attempt in 0..READY_ATTEMPTS {
        ready = alive(ctx, &server);
        if ready || attempt + 1 == READY_ATTEMPTS {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if !ready {
        let _ = call(ctx, &["rm", "-f", "-v", &server]);
        return Err(engine(name, format!("{server} did not answer a ping within {}s", READY_ATTEMPTS / 2)));
    }

    let detail = BTreeMap::from(
        [("container", server), ("host", HOST.to_owned()), ("port", port.to_string())].map(|(k, v)| (k.to_owned(), v)),
    );
    Ok(DbInstance {
        name: name.to_owned(),
        adapter: DbAdapter::Redis,
        status: ForkStatus::Ready,
        url: format!("redis://{HOST}:{port}/0"),
        env_key: env_key(name, spec),
        forked_from: source.label(),
        source: copied,
        detail,
        // What a Redis holds is memory, not a file worth measuring.
        size_bytes: None,
    })
}

/// Stops the server and removes its data. Removal always finishes: with docker off there is no
/// server to stop, and the directory still goes.
pub(super) fn remove(instance: &DbInstance, ctx: &DbContext<'_>) -> Result<(), DbError> {
    if let Some(server) = instance.detail.get("container") {
        let _ = call(ctx, &["rm", "-f", "-v", server]);
    }
    let dir = data_dir(ctx.state, &instance.name);
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DbError::Io { path: dir, source }),
    }
}

pub(super) fn answers(instance: &DbInstance, ctx: &DbContext<'_>) -> bool {
    instance.detail.get("container").is_some_and(|server| alive(ctx, server))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_server_is_named_after_the_worktree_and_the_database() {
        let a = server_name(Utf8Path::new("/repo/.git/canopy/worktrees/feat-a"), "My Cache");
        assert!(a.starts_with("canopy-redis-") && a.ends_with("-my_cache"), "{a}");
        assert_ne!(a, server_name(Utf8Path::new("/repo/.git/canopy/worktrees/feat-b"), "My Cache"));
    }

    #[test]
    fn the_image_follows_the_version_and_cannot_be_talked_into_anything_else() {
        let spec = |yaml: &str| -> DatabaseSpec { serde_saphyr::from_str(yaml).expect("spec") };
        assert_eq!(image(&spec("adapter: redis\n")), "redis:7");
        assert_eq!(image(&spec("adapter: redis\nversion: \"6.2-alpine\"\n")), "redis:6.2-alpine");
        assert_eq!(image(&spec("adapter: redis\nversion: \"7 --privileged\"\n")), "redis:7--privileged");
    }

    #[test]
    fn a_data_directory_is_copied_whole_nested_files_included() {
        let dir = TempDir::new().expect("temp dir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf-8");
        let from = root.join("from");
        fs::create_dir_all(from.join("appendonlydir")).expect("dirs");
        fs::write(from.join("dump.rdb"), b"rdb").expect("rdb");
        fs::write(from.join("appendonlydir/appendonly.aof.1.incr.aof"), b"aof").expect("aof");

        copy_dir(&from, &root.join("to")).expect("copy");

        assert_eq!(fs::read(root.join("to/dump.rdb")).expect("rdb"), b"rdb");
        assert_eq!(fs::read(root.join("to/appendonlydir/appendonly.aof.1.incr.aof")).expect("aof"), b"aof");
        let error = copy_dir(&root.join("nowhere"), &root.join("to2")).expect_err("no source");
        assert!(matches!(error, DbError::Io { .. }), "{error}");
    }
}
