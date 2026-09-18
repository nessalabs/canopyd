//! `runtime: docker` and `runtime: compose`: a container as an ordinary supervised process.
//!
//! A container started with `docker run -d` is a thing apart: its output lives in Docker, its
//! exit is something to poll for, and stopping it is a different verb from stopping a process.
//! Supporting it that way means a second implementation of logs, health, restart policy and
//! shutdown, each subtly unlike the first.
//!
//! So nothing is detached. A docker service is `docker run --rm --init …` **in the foreground**,
//! and a compose service is `docker compose up` the same way. The `docker` CLI is then a host
//! process like any other: what the container prints is what the CLI prints, which is already
//! being redirected into the service's log; the container's exit code is the CLI's; SIGTERM to
//! the process group reaches the CLI, which forwards it. Health checks, `restart:`, the
//! crash-loop budget, `logs` and `down` need no idea a container is involved.
//!
//! Two details make that hold:
//!
//! - **`--init`.** The command runs as `sh -c …`, and a shell that is PID 1 ignores SIGTERM —
//!   the kernel drops signals PID 1 has no handler for. With `--init`, PID 1 is a tiny init that
//!   forwards the signal, and the container stops when asked rather than at `stop_timeout`.
//! - **A backstop.** If the CLI is killed outright the container outlives it. Every stop is
//!   followed by `docker rm -f`, and every start is preceded by one, so a container left by a
//!   crash is never the reason the next start fails with "name already in use".
//!
//! Values reach a container as `-e KEY`, never `-e KEY=VALUE`: the CLI takes them from its own
//! environment, and an argument list is readable by every user on the machine.
//!
//! Everything that builds an argument list here is a pure function, because argument order is
//! easy to get subtly wrong and impossible to see from the outside.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};

use camino::{Utf8Path, Utf8PathBuf};

use crate::config::{ComposeSpec, DockerSpec};

/// Overrides the `docker` binary, the way `CANOPYD_GIT` overrides git. Tests point it at a
/// script; a machine with `podman` aliased to docker points it there.
pub const DOCKER_ENV: &str = "CANOPYD_DOCKER";

/// The network every docker service joins, so they reach each other by container name.
pub const NETWORK: &str = "canopy";

/// Set on every container, so `docker ps --filter label=canopy=true` finds them all.
pub const LABEL: &str = "canopy";
pub const WORKTREE_LABEL: &str = "canopy.worktree";
pub const SERVICE_LABEL: &str = "canopy.service";

pub fn docker_bin() -> String {
    std::env::var(DOCKER_ENV).unwrap_or_else(|_| "docker".to_owned())
}

// ---------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------

/// FNV-1a, 64 bits. Not `DefaultHasher`: a container name has to be the same one after a
/// toolchain upgrade, or the backstop `rm -f` stops finding what the last version started.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Docker object names allow `[a-zA-Z0-9][a-zA-Z0-9_.-]*`. Everything else becomes `-`, and a
/// leading separator is dropped.
fn sanitize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') { ch } else { '-' })
        .collect();
    cleaned.trim_start_matches(|ch: char| !ch.is_ascii_alphanumeric()).to_owned()
}

/// Eight hex characters of the worktree's path: what makes two worktrees' `web` containers two
/// containers. The path, not the branch, because it is what `git worktree list` keys on too.
fn worktree_id(worktree: &Utf8Path) -> String {
    format!("{:016x}", fnv1a(worktree.as_str().as_bytes()))[..8].to_owned()
}

/// The one place container names are built. Start, stop and the backstop all have to agree.
pub fn container_name(worktree: &Utf8Path, service: &str) -> String {
    format!("canopy-{}-{}", worktree_id(worktree), sanitize(service))
}

/// Compose project names are stricter: lowercase alphanumerics, `_` and `-`.
pub fn compose_project(worktree: &Utf8Path, service: &str) -> String {
    let name = format!("canopy-{}-{}", worktree_id(worktree), service).to_lowercase();
    name.chars().map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') { ch } else { '-' }).collect()
}

/// The image built from a service's Dockerfile. The tag is the Dockerfile's content hash, so an
/// unchanged file is a cache hit and a changed one cannot be mistaken for the old image.
pub fn image_tag(project: &str, service: &str, dockerfile: &[u8]) -> String {
    let hash = format!("{:016x}", fnv1a(dockerfile));
    format!("canopy/{}-{}:{}", sanitize(project).to_lowercase(), sanitize(service).to_lowercase(), &hash[..12])
}

// ---------------------------------------------------------------------------------------
// Argument lists
// ---------------------------------------------------------------------------------------

/// One `docker.volumes` entry. A bare name (`cache:/app/cache`) is a named volume; anything that
/// looks like a path is resolved against the worktree, so `./tmp:/tmp` means *this* worktree's.
pub fn resolve_volume(entry: &str, worktree: &Utf8Path) -> String {
    let Some((host, rest)) = entry.split_once(':') else { return entry.to_owned() };
    let is_path = host.starts_with('.') || host.starts_with('/') || host.starts_with('~') || host.contains('/');
    if host.is_empty() || !is_path {
        return entry.to_owned();
    }
    let resolved =
        if host.starts_with('/') || host.starts_with('~') { Utf8PathBuf::from(host) } else { worktree.join(host) };
    format!("{resolved}:{rest}")
}

/// Everything `docker run` needs to know about one service.
#[derive(Debug)]
pub struct RunPlan<'a> {
    pub name: &'a str,
    pub image: &'a str,
    pub service: &'a str,
    pub worktree: &'a Utf8Path,
    /// The service's `cwd:`, relative to the worktree.
    pub cwd: Option<&'a str>,
    pub docker: &'a DockerSpec,
    /// Names only — see the module docs.
    pub env: &'a BTreeMap<String, String>,
    pub ports: &'a [u16],
    /// `run:`, handed to `sh -c` in the container.
    pub command: &'a str,
}

/// The whole `docker run` argument list.
///
/// A port is published under the same number on both sides, so `${ports.api}` means one thing
/// in the container, in the env file and in the browser. Nothing here makes the app listen on
/// `0.0.0.0`: a service that binds `127.0.0.1` inside its container is unreachable from the host
/// even with `-p`, and that is the image's configuration to fix, not ours to second-guess.
pub fn run_args(plan: &RunPlan<'_>) -> Vec<String> {
    let workdir = &plan.docker.workdir;
    let inside = match plan.cwd {
        Some(cwd) => format!("{}/{}", workdir.trim_end_matches('/'), cwd.trim_start_matches("./")),
        None => workdir.clone(),
    };
    let mut args: Vec<String> = ["run", "--rm", "--init", "--name", plan.name].map(str::to_owned).into();
    for (label, value) in [(LABEL, "true"), (WORKTREE_LABEL, plan.worktree.as_str()), (SERVICE_LABEL, plan.service)] {
        args.extend(["--label".to_owned(), format!("{label}={value}")]);
    }
    args.extend(["--network".to_owned(), NETWORK.to_owned()]);
    args.extend(["-v".to_owned(), format!("{}:{workdir}", plan.worktree), "-w".to_owned(), inside]);
    for key in plan.env.keys() {
        args.extend(["-e".to_owned(), key.clone()]);
    }
    for port in plan.ports {
        args.extend(["-p".to_owned(), format!("{port}:{port}")]);
    }
    if let Some(user) = &plan.docker.user {
        args.extend(["--user".to_owned(), user.clone()]);
    }
    for volume in &plan.docker.volumes {
        args.extend(["-v".to_owned(), resolve_volume(volume, plan.worktree)]);
    }
    args.extend(plan.docker.args.iter().cloned());
    args.extend([plan.image.to_owned(), "sh".to_owned(), "-c".to_owned(), plan.command.to_owned()]);
    args
}

pub fn build_args(dockerfile: &Utf8Path, tag: &str, context: &Utf8Path) -> Vec<String> {
    ["build", "-f", dockerfile.as_str(), "-t", tag, context.as_str()].map(str::to_owned).into()
}

/// `docker compose …` with everything that pins a call to one project, then `rest`. Shared by
/// `up`, `stop` and `down` so they cannot disagree about which stack they mean.
pub fn compose_args(
    project: &str,
    file: &Utf8Path,
    spec: &ComposeSpec,
    env_file: &Utf8Path,
    rest: &[&str],
) -> Vec<String> {
    let mut args: Vec<String> = ["compose", "-p", project, "-f", file.as_str()].map(str::to_owned).into();
    for profile in &spec.profiles {
        args.extend(["--profile".to_owned(), profile.clone()]);
    }
    args.extend(["--env-file".to_owned(), env_file.to_string()]);
    args.extend(rest.iter().map(|text| (*text).to_owned()));
    args
}

/// `up`, attached. `--abort-on-container-exit=false` is explicit: one container of the stack
/// finishing — a migration sidecar — must not take the rest down behind the supervisor's back.
pub fn compose_up_args(project: &str, file: &Utf8Path, spec: &ComposeSpec, env_file: &Utf8Path) -> Vec<String> {
    let mut args =
        compose_args(project, file, spec, env_file, &["up", "--no-color", "--abort-on-container-exit=false"]);
    args.extend(spec.services.iter().cloned());
    args
}

/// The `--env-file` compose reads `${VAR}` from. Compose has no escaping, so a newline in a
/// value is flattened rather than allowed to start a new, wrong, variable.
pub fn compose_env_file(env: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (key, value) in env {
        out.push_str(key);
        out.push('=');
        out.push_str(&value.replace("\r\n", " ").replace('\n', " "));
        out.push('\n');
    }
    out
}

/// One argument, safe inside `sh -c`. Single quotes, because nothing is special inside them —
/// not `$`, not a backtick — and the one character that ends them is closed, escaped, reopened.
pub fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty() && arg.chars().all(|ch| ch.is_ascii_alphanumeric() || "_-./:=@%+,".contains(ch));
    if plain { arg.to_owned() } else { format!("'{}'", arg.replace('\'', "'\\''")) }
}

/// `exec <docker> <args…>` as one shell line. `exec`, so the shell that launches a service is
/// replaced by the CLI: signals go to the thing that forwards them, and the pid on record is
/// the CLI's rather than a wrapper's.
pub fn exec_line(docker: &str, args: &[String]) -> String {
    let mut line = format!("exec {}", shell_quote(docker));
    for arg in args {
        line.push(' ');
        line.push_str(&shell_quote(arg));
    }
    line
}

// ---------------------------------------------------------------------------------------
// Calls that finish
// ---------------------------------------------------------------------------------------

/// What a short-lived `docker` call came to.
#[derive(Debug)]
pub struct Finished {
    pub ok: bool,
    pub output: String,
}

/// Runs `docker <args>` to completion, stdout and stderr together. A docker that cannot be
/// spawned at all is `ok: false` with the reason as its output: every caller treats "docker said
/// no" and "there is no docker" the same way.
pub fn call(docker: &str, args: &[String], cwd: &Utf8Path) -> Finished {
    let result = Command::new(docker).args(args).current_dir(cwd).stdin(Stdio::null()).output();
    match result {
        Ok(out) => {
            let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
            output.push_str(&String::from_utf8_lossy(&out.stderr));
            Finished { ok: out.status.success(), output }
        }
        Err(error) => Finished { ok: false, output: format!("could not run {docker}: {error}") },
    }
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|text| (*text).to_owned()).collect()
}

/// `Err(why)` when there is no usable docker: not installed, or the daemon is not running.
pub fn available(docker: &str, cwd: &Utf8Path) -> Result<(), String> {
    let probe = call(docker, &owned(&["version", "--format", "{{.Server.Version}}"]), cwd);
    if probe.ok { Ok(()) } else { Err(format!("docker is not available: {}", last_line(&probe.output))) }
}

/// Creates the shared network unless it is there. Two worktrees starting at once both try, and
/// the loser's "already exists" is success.
pub fn ensure_network(docker: &str, cwd: &Utf8Path) -> Result<(), String> {
    if call(docker, &owned(&["network", "inspect", NETWORK]), cwd).ok {
        return Ok(());
    }
    let created = call(docker, &owned(&["network", "create", NETWORK]), cwd);
    if created.ok || created.output.contains("already exists") {
        Ok(())
    } else {
        Err(format!("could not create the {NETWORK} network: {}", last_line(&created.output)))
    }
}

/// The backstop. Removing a container that is not there is what usually happens, and is fine.
pub fn remove_container(docker: &str, name: &str, cwd: &Utf8Path) {
    let _ = call(docker, &owned(&["rm", "-f", name]), cwd);
}

pub fn image_exists(docker: &str, tag: &str, cwd: &Utf8Path) -> bool {
    call(docker, &owned(&["image", "inspect", tag]), cwd).ok
}

/// The last thing docker said, which is where it puts the reason.
pub fn last_line(output: &str) -> String {
    output.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("no output").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn docker_spec(yaml: &str) -> DockerSpec {
        serde_saphyr::from_str(yaml).expect("docker spec parses")
    }

    fn compose_spec(yaml: &str) -> ComposeSpec {
        serde_saphyr::from_str(yaml).expect("compose spec parses")
    }

    #[test]
    fn names_are_stable_distinct_per_worktree_and_legal() {
        let a = Utf8Path::new("/code/wt/feat-a");
        let b = Utf8Path::new("/code/wt/feat-b");
        // Pinned on purpose: a name that changed between versions would orphan every container
        // the last version started.
        assert_eq!(container_name(a, "web"), "canopy-e083c518-web");
        assert_ne!(container_name(a, "web"), container_name(b, "web"));
        assert_eq!(container_name(a, "my svc/2"), "canopy-e083c518-my-svc-2");
        assert_eq!(compose_project(a, "My.Stack"), "canopy-e083c518-my-stack");
    }

    #[test]
    fn an_image_tag_follows_the_dockerfile_not_the_clock() {
        let tag = image_tag("My Project", "Web", b"FROM node:22\n");
        assert_eq!(tag, image_tag("My Project", "Web", b"FROM node:22\n"));
        assert_ne!(tag, image_tag("My Project", "Web", b"FROM node:24\n"));
        assert!(tag.starts_with("canopy/my-project-web:"), "{tag}");
        assert_eq!(tag.rsplit(':').next().map(str::len), Some(12));
    }

    #[rstest]
    #[case("cache:/app/cache", "cache:/app/cache")]
    #[case("./tmp:/tmp", "/wt/./tmp:/tmp")]
    #[case("data/db:/var/lib/db:ro", "/wt/data/db:/var/lib/db:ro")]
    #[case("/abs:/x", "/abs:/x")]
    #[case("~/.npm:/root/.npm", "~/.npm:/root/.npm")]
    #[case("no-colon", "no-colon")]
    #[case(":/odd", ":/odd")]
    fn a_volume_that_looks_like_a_path_is_resolved_against_the_worktree(#[case] entry: &str, #[case] expected: &str) {
        assert_eq!(resolve_volume(entry, Utf8Path::new("/wt")), expected);
    }

    #[test]
    fn the_run_argument_list_is_exactly_this() {
        let docker = docker_spec(
            "image: node:22\nuser: \"1000:1000\"\nvolumes: [\"cache:/cache\", \"./tmp:/tmp\"]\nargs: [\"--cpus\", \"2\"]\n",
        );
        let env =
            BTreeMap::from([("API_TOKEN".to_owned(), "hunter2".to_owned()), ("PORT".to_owned(), "5173".to_owned())]);
        let plan = RunPlan {
            name: "canopy-abc-web",
            image: "node:22",
            service: "web",
            worktree: Utf8Path::new("/wt"),
            cwd: Some("./apps/web"),
            docker: &docker,
            env: &env,
            ports: &[5173, 9229],
            command: "npm run dev -- --port $PORT",
        };

        let args = run_args(&plan);

        let expected = [
            "run",
            "--rm",
            "--init",
            "--name",
            "canopy-abc-web",
            "--label",
            "canopy=true",
            "--label",
            "canopy.worktree=/wt",
            "--label",
            "canopy.service=web",
            "--network",
            "canopy",
            "-v",
            "/wt:/workspace",
            "-w",
            "/workspace/apps/web",
            "-e",
            "API_TOKEN",
            "-e",
            "PORT",
            "-p",
            "5173:5173",
            "-p",
            "9229:9229",
            "--user",
            "1000:1000",
            "-v",
            "cache:/cache",
            "-v",
            "/wt/./tmp:/tmp",
            "--cpus",
            "2",
            "node:22",
            "sh",
            "-c",
            "npm run dev -- --port $PORT",
        ];
        assert_eq!(args, expected);
        assert!(!args.iter().any(|arg| arg.contains("hunter2")), "a value must never be an argument");
    }

    #[test]
    fn without_a_cwd_the_container_starts_in_the_workdir() {
        let docker = docker_spec("image: x\nworkdir: /app\n");
        let env = BTreeMap::new();
        let plan = RunPlan {
            name: "n",
            image: "x",
            service: "s",
            worktree: Utf8Path::new("/wt"),
            cwd: None,
            docker: &docker,
            env: &env,
            ports: &[],
            command: "true",
        };
        let args = run_args(&plan);
        let at = args.iter().position(|arg| arg == "-w").expect("-w");
        assert_eq!(args[at + 1], "/app");
        assert!(args.contains(&"/wt:/app".to_owned()));
    }

    #[test]
    fn compose_calls_all_name_the_same_project_file_profiles_and_env_file() {
        let spec = compose_spec("file: ops/stack.yml\nservices: [api, worker]\nprofiles: [dev]\n");
        let file = Utf8Path::new("/wt/ops/stack.yml");
        let env_file = Utf8Path::new("/state/compose-stack.env");

        let pinned = [
            "compose",
            "-p",
            "proj",
            "-f",
            "/wt/ops/stack.yml",
            "--profile",
            "dev",
            "--env-file",
            "/state/compose-stack.env",
        ];
        let up = compose_up_args("proj", file, &spec, env_file);
        assert_eq!(up[..pinned.len()], pinned);
        assert_eq!(up[pinned.len()..], ["up", "--no-color", "--abort-on-container-exit=false", "api", "worker"]);

        let down = compose_args("proj", file, &spec, env_file, &["down", "-v", "--remove-orphans"]);
        assert_eq!(down[..pinned.len()], pinned);
        assert_eq!(down[pinned.len()..], ["down", "-v", "--remove-orphans"]);
    }

    #[test]
    fn the_compose_env_file_is_sorted_and_cannot_be_split_by_a_newline() {
        let env = BTreeMap::from([
            ("B".to_owned(), "two\nINJECTED=1".to_owned()),
            ("A".to_owned(), "one\r\nmore".to_owned()),
        ]);
        assert_eq!(compose_env_file(&env), "A=one more\nB=two INJECTED=1\n");
    }

    #[rstest]
    #[case("plain-arg_1.2/x:y=z@%+,", "plain-arg_1.2/x:y=z@%+,")]
    #[case("", "''")]
    #[case("two words", "'two words'")]
    #[case("$HOME `id`", "'$HOME `id`'")]
    #[case("it's", "'it'\\''s'")]
    fn an_argument_survives_the_shell_exactly(#[case] arg: &str, #[case] expected: &str) {
        assert_eq!(shell_quote(arg), expected);
        // And the shell agrees: what comes out is what went in.
        let out =
            Command::new("/bin/sh").args(["-c", &format!("printf %s {}", shell_quote(arg))]).output().expect("sh");
        assert_eq!(String::from_utf8_lossy(&out.stdout), arg);
    }

    #[test]
    fn the_exec_line_replaces_the_shell_with_docker() {
        let line =
            exec_line("/opt/my docker", &["run".to_owned(), "sh".to_owned(), "-c".to_owned(), "echo $X".to_owned()]);
        assert_eq!(line, "exec '/opt/my docker' run sh -c 'echo $X'");
    }

    #[test]
    fn build_args_name_the_dockerfile_the_tag_and_the_context() {
        let args = build_args(Utf8Path::new("/wt/Dockerfile.dev"), "canopy/p-s:abc", Utf8Path::new("/wt"));
        assert_eq!(args, ["build", "-f", "/wt/Dockerfile.dev", "-t", "canopy/p-s:abc", "/wt"]);
    }

    #[test]
    fn a_docker_that_is_not_there_is_not_available_and_says_why() {
        let cwd = Utf8Path::new("/");
        let error = available("/definitely/not/docker", cwd).expect_err("no docker");
        assert!(error.starts_with("docker is not available: could not run /definitely/not/docker"), "{error}");
        assert!(!image_exists("/definitely/not/docker", "x", cwd));
        assert!(ensure_network("/definitely/not/docker", cwd).expect_err("no docker").contains("could not create"));
        // The backstop never fails, because there is nothing a caller could do about it.
        remove_container("/definitely/not/docker", "x", cwd);
    }

    #[rstest]
    #[case("one\ntwo\n\n", "two")]
    #[case("", "no output")]
    #[case("  padded  ", "padded")]
    fn the_last_line_is_where_docker_puts_the_reason(#[case] output: &str, #[case] expected: &str) {
        assert_eq!(last_line(output), expected);
    }
}
