//! Redis and MySQL forks through the CLI — the engines where a fork is a server of its own —
//! against a stand-in for `docker` that remembers which containers are running.

mod fixture;

use camino::Utf8PathBuf;
use fixture::{Fixture, err_envelope, ok_envelope};

const CONFIG: &str = r#"version: 1
name: shop
worktree:
  path: "{{ repo_path }}/../wt/{{ name }}"
ports:
  web: {}
databases:
  cache:
    adapter: redis
  main:
    adapter: mysql
    env: DATABASE_URL
    seed: { sql: db/seed.sql }
env:
  CACHE_PORT: ${db.cache.port}
"#;

struct Stage {
    fx: Fixture,
    base: Utf8PathBuf,
    docker: Utf8PathBuf,
}

impl Stage {
    fn new(config: &str) -> Stage {
        let fx = Fixture::new();
        fx.commit(&[("canopy.yaml", config), ("db/seed.sql", "CREATE TABLE a (id int);\n")], "config");
        let base = fx.root.parent().unwrap().to_owned();
        let docker = base.join("fake-docker");
        let script = r#"#!/bin/sh
B='__BASE__'
printf '%s\n' "$*" >> "$B/calls"
last() { for a; do :; done; printf '%s' "$a"; }
case "$1" in
  version) echo 27.1.0 ;;
  network) exit 0 ;;
  rm) rm -f "$B/running-$(last "$@")"; exit 0 ;;
  run) n=''; while [ $# -gt 0 ]; do [ "$1" = --name ] && n=$2; shift; done; : > "$B/running-$n"; echo deadbeef ;;
  port) echo '127.0.0.1:33061' ;;
  cp) case "$2" in *:/tmp/*) printf 'dump of %s\n' "${2%%:*}" > "$3" ;; esac; exit 0 ;;
  exec)
    c=$2; [ -f "$B/running-$c" ] || { echo "Error response from daemon: No such container: $c" >&2; exit 1; }
    shift 2
    case "$1" in
      redis-cli) echo PONG ;;
      mysqladmin) echo 'mysqld is alive' ;;
      mysql) echo 4096 ;;
      sh) case "$*" in *'mysql -uroot'*) [ -f "$B/load-fails" ] && { echo 'ERROR 1064 (42000): You have an error in your SQL syntax' >&2; exit 1; } ;; esac; exit 0 ;;
    esac ;;
esac
"#
        .replace("__BASE__", base.as_str());
        std::fs::write(&docker, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let stage = Stage { fx, base, docker };
        assert!(stage.cmd(&["new", "feat/x"]).output().unwrap().status.success());
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

    fn fails(&self, args: &[&str]) -> String {
        let out = self.cmd(args).arg("--json").output().unwrap();
        err_envelope(&out.stdout, "db_failed")["error"]["message"].as_str().unwrap().to_owned()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn forget_calls(&self) {
        let _ = std::fs::remove_file(self.base.join("calls"));
    }

    fn running(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.base)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter_map(|name| name.strip_prefix("running-").map(str::to_owned))
            .collect();
        names.sort();
        names
    }
}

#[test]
fn a_redis_fork_is_a_server_of_its_own_on_a_port_that_never_moves() {
    let stage = Stage::new(CONFIG);

    let forks = stage.run(&["db", "fork", "feat/x", "--only", "cache"]);

    let fork = &forks[0];
    let server = fork["detail"]["container"].as_str().unwrap().to_owned();
    let port = fork["detail"]["port"].as_str().unwrap().to_owned();
    assert!(server.starts_with("canopy-redis-") && server.ends_with("-cache"), "{server}");
    assert_eq!(fork["url"], format!("redis://127.0.0.1:{port}/0"));
    assert_eq!(fork["env_key"], "CACHE_URL");
    assert_eq!(fork["status"], "ready");
    assert!(fork.get("size_bytes").is_none(), "what a redis holds is memory, not a file: {fork}");

    let run = stage.calls().into_iter().find(|call| call.starts_with("run -d ")).expect("docker run");
    let expected_head = format!(
        "run -d --name {server} --label canopy=true --label canopy.database=redis --network canopy -p 127.0.0.1:{port}:6379 -v "
    );
    assert!(run.starts_with(&expected_head), "{run}");
    assert!(run.ends_with("/redis/cache:/data redis:7 redis-server --appendonly yes --dir /data"), "{run}");
    assert_eq!(stage.running(), std::slice::from_ref(&server));

    // The port is the environment, a field, and not one of the worktree's declared ports.
    let vars = stage.run(&["env", "feat/x", "--reveal"]);
    let value = |key: &str| vars.as_array().unwrap().iter().find(|v| v["key"] == key).unwrap()["value"].clone();
    assert_eq!(value("CACHE_URL"), fork["url"]);
    assert_eq!(value("CACHE_PORT"), port);
    assert!(stage.run(&["ports", "feat/x"]).get("db-cache").is_none());

    // A reset is a new server on the same port.
    let again = stage.run(&["db", "reset", "cache", "feat/x"]);
    assert_eq!(again["detail"]["port"], port);
}

#[test]
fn a_redis_fork_can_start_from_another_branchs_data() {
    let stage = Stage::new(CONFIG);
    stage.run(&["db", "fork", "main", "--only", "cache"]);
    let theirs = stage.calls().into_iter().find(|call| call.starts_with("run -d ")).unwrap();
    let their_dir = theirs.split(" -v ").nth(1).unwrap().split(":/data").next().unwrap().to_owned();
    std::fs::create_dir_all(format!("{their_dir}/appendonlydir")).unwrap();
    std::fs::write(format!("{their_dir}/appendonlydir/appendonly.aof"), "SET greeting hello").unwrap();

    let mine = stage.run(&["db", "fork", "feat/x", "--only", "cache", "--from", "main"]);

    assert_eq!(mine[0]["forked_from"], "worktree main");
    assert_eq!(mine[0]["source"], their_dir);
    let run = stage.calls().into_iter().rfind(|call| call.starts_with("run -d ")).unwrap();
    let my_dir = run.split(" -v ").nth(1).unwrap().split(":/data").next().unwrap().to_owned();
    assert_ne!(my_dir, their_dir);
    // Copied before the server started: Redis loads what it finds at boot and nothing later.
    assert_eq!(
        std::fs::read_to_string(format!("{my_dir}/appendonlydir/appendonly.aof")).unwrap(),
        "SET greeting hello"
    );

    stage.run(&["db", "drop", "main", "--only", "cache"]);
    assert_eq!(
        stage.fails(&["db", "reset", "cache", "feat/x", "--from", "main"]),
        "database cache: main has no fork of it to copy"
    );
}

#[test]
fn a_server_that_is_gone_is_missing_and_drop_takes_the_data_with_the_server() {
    let stage = Stage::new(CONFIG);
    let forks = stage.run(&["db", "fork", "feat/x", "--only", "cache"]);
    let server = forks[0]["detail"]["container"].as_str().unwrap().to_owned();
    let run = stage.calls().into_iter().find(|call| call.starts_with("run -d ")).unwrap();
    let dir = run.split(" -v ").nth(1).unwrap().split(":/data").next().unwrap().to_owned();
    assert!(std::path::Path::new(&dir).is_dir());

    std::fs::remove_file(stage.base.join(format!("running-{server}"))).unwrap();
    assert_eq!(stage.run(&["db", "ls", "feat/x"])[0]["status"], "missing");
    assert_eq!(
        stage.run(&["db", "fork", "feat/x", "--only", "cache"])[0]["status"],
        "ready",
        "and fork makes it again"
    );

    assert_eq!(stage.run(&["db", "drop", "feat/x"]), serde_json::json!({ "dropped": ["cache"] }));
    assert!(stage.calls().contains(&format!("rm -f -v {server}")));
    assert!(stage.running().is_empty());
    assert!(!std::path::Path::new(&dir).exists(), "the data is the database");
}

#[test]
fn a_mysql_fork_is_its_own_server_filled_from_the_projects_template_file() {
    let stage = Stage::new(CONFIG);

    let forks = stage.run(&["db", "fork", "feat/x", "--only", "main"]);

    let fork = &forks[0];
    let server = fork["detail"]["container"].as_str().unwrap().to_owned();
    let port = fork["detail"]["port"].as_str().unwrap().to_owned();
    assert!(server.starts_with("canopy-mysql-") && server.ends_with("-main"), "{server}");
    assert_eq!(fork["url"], format!("mysql://root:canopy@127.0.0.1:{port}/app"));
    assert_eq!(fork["size_bytes"], 4096);
    assert_eq!(fork["forked_from"], "seed template");
    let template = fork["source"].as_str().unwrap().to_owned();
    assert!(template.ends_with("/templates/mysql-main.sql"), "{template}");
    assert_eq!(std::fs::read_to_string(&template).unwrap(), "CREATE TABLE a (id int);\n");

    let calls = stage.calls();
    let run = calls.iter().find(|call| call.starts_with("run -d ")).expect("docker run");
    assert_eq!(
        *run,
        format!(
            "run -d --name {server} --label canopy=true --label canopy.database=mysql --network canopy \
             -e MYSQL_ROOT_PASSWORD=canopy -e MYSQL_DATABASE=app -p 127.0.0.1:{port}:3306 mysql:8"
        )
    );
    assert!(calls.contains(&format!("cp {template} {server}:/tmp/canopy-seed.sql")), "{calls:?}");
    assert!(
        calls.contains(&format!("exec {server} sh -c mysql -uroot -pcanopy app < /tmp/canopy-seed.sql")),
        "{calls:?}"
    );

    // The template is the project's, made once: a changed seed is not picked up until asked.
    stage.fx.write("db/seed.sql", "CREATE TABLE b (id int);\n");
    stage.run(&["db", "fork", "main", "--only", "main"]);
    assert_eq!(std::fs::read_to_string(&template).unwrap(), "CREATE TABLE a (id int);\n");
    assert_eq!(
        stage.run(&["db", "template", "main", "--only", "main"]),
        serde_json::json!({ "rebuilt": [template.clone()] })
    );
    assert_eq!(std::fs::read_to_string(&template).unwrap(), "CREATE TABLE b (id int);\n");
}

#[test]
fn a_mysql_seed_command_runs_against_a_throwaway_server_whose_dump_becomes_the_template() {
    let config =
        CONFIG.replace("seed: { sql: db/seed.sql }", "seed: { command: 'echo \"$MYSQL_URL\" > migrated.txt' }");
    let stage = Stage::new(&config);

    let forks = stage.run(&["db", "fork", "feat/x", "--only", "main"]);

    let calls = stage.calls();
    let throwaway = calls.iter().find(|call| call.contains("--name canopy-mysql-tpl-")).expect("a throwaway server");
    assert!(throwaway.ends_with("-p 127.0.0.1::3306 mysql:8"), "docker picks its port: {throwaway}");
    assert_eq!(
        std::fs::read_to_string(stage.fx.root.join("migrated.txt")).unwrap().trim(),
        "mysql://root:canopy@127.0.0.1:33061/app"
    );
    let template = forks[0]["source"].as_str().unwrap();
    assert!(
        std::fs::read_to_string(template).unwrap().starts_with("dump of canopy-mysql-tpl-"),
        "the dump is the template"
    );
    // It was only ever for seeding.
    let left = stage.running();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].starts_with("canopy-mysql-") && !left[0].contains("-tpl-"), "{left:?}");
}

#[test]
fn a_mysql_fork_can_be_dumped_from_another_branchs_server() {
    let stage = Stage::new(CONFIG);
    let theirs =
        stage.run(&["db", "fork", "main", "--only", "main"])[0]["detail"]["container"].as_str().unwrap().to_owned();
    stage.forget_calls();

    let mine = stage.run(&["db", "fork", "feat/x", "--only", "main", "--from", "main"]);

    assert_eq!(mine[0]["source"], format!("{theirs}/app"));
    let calls = stage.calls();
    let dumped = calls.iter().position(|c| {
        *c == format!("exec {theirs} sh -c mysqldump -uroot -pcanopy --no-tablespaces app > /tmp/canopy-seed.sql")
    });
    let started = calls.iter().position(|c| c.starts_with("run -d "));
    assert!(dumped.is_some() && dumped < started, "the source is settled before a server is started: {calls:#?}");

    // No server on that branch means nothing to dump, and no server is started to find that out.
    stage.run(&["db", "drop", "main"]);
    stage.forget_calls();
    assert_eq!(
        stage.fails(&["db", "reset", "main", "feat/x", "--from", "main"]),
        "database main: main has no fork of it to copy"
    );
    assert!(!stage.calls().iter().any(|call| call.starts_with("run -d ")));
}

#[test]
fn a_mysql_fork_that_cannot_be_filled_leaves_no_server_and_no_record() {
    let stage = Stage::new(CONFIG);
    std::fs::write(stage.base.join("load-fails"), "").unwrap();

    let message = stage.fails(&["db", "fork", "feat/x", "--only", "main"]);

    assert_eq!(message, "database main: mysql: ERROR 1064 (42000): You have an error in your SQL syntax");
    assert!(stage.running().is_empty(), "{:?}", stage.running());
    assert_eq!(stage.run(&["db", "ls", "feat/x"]), serde_json::json!([]));
}

#[test]
fn removing_a_worktree_removes_every_server_it_had() {
    let stage = Stage::new(CONFIG);
    stage.run(&["db", "fork", "feat/x"]);
    assert_eq!(stage.running().len(), 2);

    let removed = stage.run(&["rm", "feat/x", "--force", "--delete-branch", "always"]);

    assert_eq!(removed["databases_dropped"], serde_json::json!(["cache", "main"]));
    assert!(stage.running().is_empty());
    // Their ports went back with the worktree's own.
    assert_eq!(removed["ports_released"], 3);
}
