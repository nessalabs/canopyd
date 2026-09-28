//! `runtime: docker` and `runtime: compose` through the CLI, against a stand-in for `docker`.
//!
//! The stand-in is a shell script named by `CANOPYD_DOCKER` that writes down every argument list
//! it is given, and for `run` and `compose up` stays alive the way the real CLI does. That is
//! the whole contract between canopyd and docker — what it is asked, and that it keeps running —
//! so these tests run on a machine with no docker at all, and assert on exactly the thing that
//! is easy to get subtly wrong: the arguments.

mod fixture;

use camino::Utf8PathBuf;
use fixture::{Fixture, ok_envelope};

const HEAD: &str = r#"version: 1
name: demo
worktree:
  path: "{{ repo_path }}/../wt/{{ name }}"
ports:
  web: {}
env:
  API_TOKEN: hunter2
services:
"#;

/// How the stand-in behaves where a test needs it to differ.
#[derive(Clone, Copy)]
struct Behaviour {
    daemon_up: bool,
    image_exists: bool,
    build_ok: bool,
}

const WORKING: Behaviour = Behaviour { daemon_up: true, image_exists: false, build_ok: true };

struct Stage {
    fx: Fixture,
    worktree: Utf8PathBuf,
    docker: Utf8PathBuf,
    calls: Utf8PathBuf,
}

impl Stage {
    fn new(services: &str, behaviour: Behaviour) -> Stage {
        let fx = Fixture::new();
        fx.commit(&[("canopy.yaml", &format!("{HEAD}{services}")), ("Dockerfile.dev", "FROM node:22\n")], "config");
        let base = fx.root.parent().unwrap().to_owned();
        let calls = base.join("docker-calls.txt");
        let docker = base.join("fake-docker");
        let version =
            if behaviour.daemon_up { "echo 27.1.0" } else { "echo 'Cannot connect to the Docker daemon' >&2; exit 1" };
        let build = if behaviour.build_ok { "exit 0" } else { "echo 'failed to solve: boom' >&2; exit 1" };
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\ncase \"$1\" in\n  version) {version} ;;\n  network|rm) exit 0 ;;\n  image) exit {image} ;;\n  build) echo 'Step 1/1 : FROM node:22'; {build} ;;\n  run) echo 'container says hi'; echo \"TOKEN=$API_TOKEN\"; exec sleep 600 ;;\n  compose) case \" $* \" in *' up '*) echo 'stack up'; exec sleep 600 ;; *) exit 0 ;; esac ;;\nesac\n",
            image = if behaviour.image_exists { 0 } else { 1 },
        );
        std::fs::write(&docker, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();

        let stage = Stage { worktree: base.join("wt/feat-x"), fx, docker, calls };
        let out = stage.cmd(&["new", "feat/x"]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        stage
    }

    fn cmd(&self, args: &[&str]) -> std::process::Command {
        let mut command = self.fx.cwt();
        command.env("CANOPYD_DOCKER", self.docker.as_str()).args(args);
        command
    }

    fn run(&self, args: &[&str]) -> serde_json::Value {
        let out = self.cmd(args).arg("--json").output().unwrap();
        ok_envelope(&out.stdout)["data"].clone()
    }

    /// Every argument list docker has been given so far, one per line, oldest first.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn port(&self) -> String {
        self.run(&["ports", "feat/x"])["web"].to_string()
    }

    fn logged(&self, service: &str, needle: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let out = self.cmd(&["logs", service, "feat/x"]).output().unwrap();
            if String::from_utf8_lossy(&out.stdout).contains(needle) {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = self.cmd(&["down", "feat/x"]).output();
    }
}

const DOCKER_WEB: &str = "  web:\n    runtime: docker\n    run: npm start -- --port ${ports.web}\n    cwd: apps/web\n    docker:\n      image: node:22\n      volumes: [\"./tmp:/tmp\"]\n";

#[test]
fn a_docker_service_is_the_docker_cli_run_attached_with_values_kept_out_of_its_arguments() {
    let stage = Stage::new(DOCKER_WEB, WORKING);
    let port = stage.port();

    let started = stage.run(&["up", "feat/x", "--no-wait"]);
    assert_eq!(started[0]["state"], "running", "{started}");
    assert!(started[0]["pid"].as_i64().is_some_and(|pid| pid > 0));

    let calls = stage.calls();
    let name =
        calls.iter().find_map(|call| call.strip_prefix("rm -f ")).expect("a leftover is cleared first").to_owned();
    assert!(name.starts_with("canopy-") && name.ends_with("-web"), "{name}");
    assert_eq!(calls[0], "version --format {{.Server.Version}}");
    assert_eq!(calls[1], "network inspect canopy");
    assert_eq!(calls[2], format!("rm -f {name}"));
    let wt = &stage.worktree;
    let expected = format!(
        "run --rm --init --name {name} --label canopy=true --label canopy.worktree={wt} --label canopy.service=web \
         --network canopy -v {wt}:/workspace -w /workspace/apps/web"
    );
    assert!(calls[3].starts_with(&expected), "{}", calls[3]);
    assert!(calls[3].contains(" -e API_TOKEN ") && calls[3].contains(" -e CANOPY_BRANCH "), "{}", calls[3]);
    assert!(calls[3].contains(&format!(" -p {port}:{port} ")), "{}", calls[3]);
    assert!(
        calls[3].ends_with(&format!(" -v {wt}/./tmp:/tmp node:22 sh -c npm start -- --port {port}")),
        "{}",
        calls[3]
    );
    assert!(!calls.iter().any(|call| call.contains("hunter2")), "a value reached an argument list: {calls:?}");

    // The value got there anyway, through the CLI's environment, and the container's output is
    // the service's log without anybody pumping it.
    assert!(stage.logged("web", "TOKEN=hunter2"));
    assert!(stage.logged("web", "container says hi"));
    assert_eq!(stage.run(&["ps", "feat/x"])[0]["state"], "running");

    let stopped = stage.run(&["down", "feat/x"]);
    assert_eq!(stopped[0]["state"], "stopped");
    assert_eq!(
        stage.calls().last().map(String::as_str),
        Some(format!("rm -f {name}").as_str()),
        "the backstop runs after the stop"
    );
}

#[test]
fn a_docker_that_is_not_running_fails_the_service_with_dockers_own_words_and_starts_nothing() {
    let stage = Stage::new(DOCKER_WEB, Behaviour { daemon_up: false, ..WORKING });

    let started = stage.run(&["up", "feat/x", "--no-wait"]);

    assert_eq!(started[0]["state"], "failed", "{started}");
    assert_eq!(started[0]["detail"], "docker is not available: Cannot connect to the Docker daemon");
    assert_eq!(stage.calls(), ["version --format {{.Server.Version}}"]);
}

#[test]
fn a_dockerfile_is_built_once_under_a_tag_that_follows_its_contents() {
    let yaml = "  web:\n    runtime: docker\n    run: npm start\n    docker:\n      dockerfile: Dockerfile.dev\n";
    let stage = Stage::new(yaml, WORKING);

    stage.run(&["up", "feat/x", "--no-wait"]);

    let calls = stage.calls();
    let build = calls.iter().find(|call| call.starts_with("build ")).expect("docker build");
    let tag = build.split(' ').nth(4).expect("the tag").to_owned();
    assert!(tag.starts_with("canopy/demo-web:"), "{tag}");
    assert_eq!(*build, format!("build -f {wt}/Dockerfile.dev -t {tag} {wt}/.", wt = stage.worktree));
    assert!(calls.contains(&format!("image inspect {tag}")));
    let run = calls.iter().find(|call| call.starts_with("run ")).expect("docker run");
    assert!(run.ends_with(&format!(" {tag} sh -c npm start")), "{run}");
    // The build's output is the first thing anyone wants when such a service will not start.
    assert!(stage.logged("web", "Step 1/1 : FROM node:22"));
}

#[test]
fn an_image_that_is_already_built_is_not_built_again() {
    let yaml = "  web:\n    runtime: docker\n    run: npm start\n    docker:\n      dockerfile: Dockerfile.dev\n";
    let stage = Stage::new(yaml, Behaviour { image_exists: true, ..WORKING });
    stage.run(&["up", "feat/x", "--no-wait"]);
    assert!(!stage.calls().iter().any(|call| call.starts_with("build ")), "{:?}", stage.calls());
}

#[test]
fn a_build_that_fails_is_a_failed_service_with_the_reason_and_the_output_in_its_log() {
    let yaml = "  web:\n    runtime: docker\n    run: npm start\n    docker:\n      dockerfile: Dockerfile.dev\n";
    let stage = Stage::new(yaml, Behaviour { build_ok: false, ..WORKING });

    let started = stage.run(&["up", "feat/x", "--no-wait"]);

    assert_eq!(started[0]["state"], "failed");
    assert_eq!(started[0]["detail"], "docker build failed: failed to solve: boom");
    assert!(!stage.calls().iter().any(|call| call.starts_with("run ")));
    assert!(stage.logged("web", "failed to solve: boom"));

    // A Dockerfile that is not there is the same kind of failure, found sooner.
    let missing = Stage::new(
        "  web:\n    runtime: docker\n    run: npm start\n    docker:\n      dockerfile: nope/Dockerfile\n",
        WORKING,
    );
    let started = missing.run(&["up", "feat/x", "--no-wait"]);
    assert_eq!(started[0]["state"], "failed");
    assert!(started[0]["detail"].as_str().unwrap().starts_with("nope/Dockerfile: "), "{started}");
}

#[test]
fn a_compose_service_is_compose_up_attached_and_every_call_names_the_same_stack() {
    let yaml = "  stack:\n    compose:\n      file: ops/stack.yml\n      services: [api]\n      profiles: [dev]\n    stop_timeout: 7s\n";
    let stage = Stage::new(yaml, WORKING);
    let port = stage.port();

    let started = stage.run(&["up", "feat/x", "--no-wait"]);
    assert_eq!(started[0]["state"], "running", "{started}");

    let calls = stage.calls();
    let up = calls.iter().find(|call| call.contains(" up ")).expect("compose up");
    let pinned = up.split(" up ").next().unwrap().to_owned();
    assert!(pinned.starts_with("compose -p canopy-"), "{pinned}");
    assert!(
        pinned.contains(&format!("-stack -f {}/ops/stack.yml --profile dev --env-file ", stage.worktree)),
        "{pinned}"
    );
    assert!(up.ends_with(" up --no-color --abort-on-container-exit=false api"), "{up}");
    assert!(stage.logged("stack", "stack up"));

    // `${VAR}` in the compose file reads this worktree's values, not the developer's shell.
    let env_file = pinned.rsplit(' ').next().unwrap();
    let env = std::fs::read_to_string(env_file).unwrap();
    assert!(env.contains(&format!("CANOPY_PORT_WEB={port}\n")) && env.contains("API_TOKEN=hunter2\n"), "{env}");

    stage.run(&["down", "feat/x"]);
    assert_eq!(stage.calls().last().unwrap(), &format!("{pinned} stop -t 7"), "a stop keeps the stack for next time");

    // Removing the worktree does not: its volumes would be inherited by the next one on the path.
    stage.run(&["rm", "feat/x", "--force", "--delete-branch", "always"]);
    assert!(stage.calls().contains(&format!("{pinned} down -v --remove-orphans")), "{:?}", stage.calls());
}
