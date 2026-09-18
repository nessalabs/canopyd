//! Postgres forks through the CLI, against a stand-in for `docker`.
//!
//! The stand-in keeps a list of databases in a file, so `CREATE DATABASE`, `DROP DATABASE`,
//! "does it exist" and `pg_database_size` answer the way the real server would, and it writes
//! down every argument list it is given. What is under test is the conversation: which
//! statements are sent, in what order, against which names. The same flow was run by hand
//! against a real `postgres:16` container; this is what keeps it true on a machine without one.

mod fixture;

use camino::Utf8PathBuf;
use fixture::{Fixture, err_envelope, ok_envelope};

const CONFIG: &str = r#"version: 1
name: My App
worktree:
  path: "{{ repo_path }}/../wt/{{ name }}"
databases:
  main:
    adapter: postgres
    env: DATABASE_URL
    seed: { sql: db/seed.sql }
    options: { extensions: "pgcrypto, citext" }
env:
  PG_HOST: ${db.main.host}
  PG_PORT: ${db.main.port}
  PG_NAME: ${db.main.database}
"#;

struct Stage {
    fx: Fixture,
    base: Utf8PathBuf,
    docker: Utf8PathBuf,
}

impl Stage {
    /// `server` is what `docker inspect` finds before anything is done: `none`, `stopped`, `running`.
    fn new(config: &str, server: &str) -> Stage {
        let fx = Fixture::new();
        fx.commit(&[("canopy.yaml", config), ("db/seed.sql", "CREATE TABLE notes (id int);\n")], "config");
        std::fs::write(fx.root.join("db/seed.dump"), b"PGDMP\x01\x0e binary").unwrap();
        let base = fx.root.parent().unwrap().to_owned();
        std::fs::write(base.join("server"), server).unwrap();
        std::fs::write(base.join("dbs"), "postgres\n").unwrap();
        let docker = base.join("fake-docker");
        let script = r#"#!/bin/sh
B='__BASE__'
printf '%s\n' "$*" >> "$B/calls"
has() { grep -qx "$1" "$B/dbs"; }
case "$1" in
  version) [ -f "$B/daemon-down" ] && { echo 'Cannot connect to the Docker daemon' >&2; exit 1; }; echo 27.1.0 ;;
  network|cp|start) [ "$1" = start ] && echo running > "$B/server"; exit 0 ;;
  inspect) case "$(cat "$B/server")" in none) exit 1 ;; stopped) echo false ;; *) echo true ;; esac ;;
  run) echo running > "$B/server"; echo deadbeef ;;
  port) echo '127.0.0.1:54316' ;;
  exec)
    shift 2
    case "$1" in
      pg_isready) exit 0 ;;
      rm) exit 0 ;;
      pg_restore) echo 'pg_restore: warning: errors ignored on restore: 1' >&2; exit 1 ;;
      psql)
        for last; do :; done
        case "$*" in *" -f "*) [ -f "$B/seed-fails" ] && { echo 'psql:/tmp/seed.sql:1: ERROR:  syntax error' >&2; exit 3; }; exit 0 ;; esac
        name=$(printf '%s' "$last" | sed -E "s/^[^\"']*[\"']([^\"']+)[\"'].*/\1/")
        case "$last" in
          'SELECT 1 FROM pg_database'*) has "$name" && echo 1; exit 0 ;;
          'SELECT pg_database_size'*) has "$name" && { echo 8192; exit 0; }; echo "ERROR:  database \"$name\" does not exist" >&2; exit 1 ;;
          'CREATE DATABASE'*) has "$name" && { echo "ERROR:  database \"$name\" already exists" >&2; exit 1; }; echo "$name" >> "$B/dbs" ;;
          'DROP DATABASE'*) grep -vx "$name" "$B/dbs" > "$B/dbs.new"; mv "$B/dbs.new" "$B/dbs" ;;
        esac ;;
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

    fn fails(&self, args: &[&str], code: &str) -> String {
        let out = self.cmd(args).arg("--json").output().unwrap();
        err_envelope(&out.stdout, code)["error"]["message"].as_str().unwrap().to_owned()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// The SQL sent so far: the last argument of every `psql -tAc`.
    fn statements(&self) -> Vec<String> {
        self.calls().iter().filter_map(|call| call.split_once(" -tAc ").map(|(_, sql)| sql.to_owned())).collect()
    }

    fn databases(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("dbs")).unwrap().lines().map(str::to_owned).collect()
    }

    fn forget_calls(&self) {
        let _ = std::fs::remove_file(self.base.join("calls"));
    }
}

#[test]
fn the_first_fork_makes_the_server_builds_the_template_and_clones_it() {
    let stage = Stage::new(CONFIG, "none");

    let forks = stage.run(&["db", "fork", "feat/x"]);

    let fork = &forks[0];
    let database = fork["detail"]["database"].as_str().unwrap().to_owned();
    assert!(database.starts_with("wt_") && database.ends_with("_main"), "{database}");
    assert_eq!(fork["status"], "ready");
    assert_eq!(fork["adapter"], "postgres");
    assert_eq!(fork["url"], format!("postgresql://canopy:canopy@127.0.0.1:54316/{database}"));
    assert_eq!(fork["source"], "tpl_my_app_main");
    assert_eq!(fork["forked_from"], "seed template");
    assert_eq!(fork["size_bytes"], 8192);
    assert_eq!(fork["detail"]["container"], "canopy-pg-16");

    let calls = stage.calls();
    let created = calls.iter().find(|call| call.starts_with("run -d ")).expect("the server is created");
    assert_eq!(
        created,
        "run -d --name canopy-pg-16 --label canopy=true --label canopy.database=postgres --network canopy \
         -e POSTGRES_USER=canopy -e POSTGRES_PASSWORD=canopy -e POSTGRES_DB=postgres \
         -p 127.0.0.1:54316:5432 -v canopy-pg-16-data:/var/lib/postgresql/data postgres:16"
    );
    assert!(
        calls.contains(&format!("cp {}/db/seed.sql canopy-pg-16:/tmp/canopy-seed-seed.sql", stage.fx.root)),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|call| call.ends_with("-d tpl_my_app_main -v ON_ERROR_STOP=1 -f /tmp/canopy-seed-seed.sql"))
    );

    let sql = stage.statements();
    let at =
        |needle: &str| sql.iter().position(|s| s == needle).unwrap_or_else(|| panic!("never sent {needle}: {sql:#?}"));
    // The template is made, given its extensions and seeded before anything is cloned from it.
    assert!(at("CREATE DATABASE \"tpl_my_app_main\"") < at("CREATE EXTENSION IF NOT EXISTS \"pgcrypto\""));
    assert!(
        at("CREATE EXTENSION IF NOT EXISTS \"citext\"")
            < at(&format!("CREATE DATABASE \"{database}\" TEMPLATE \"tpl_my_app_main\""))
    );
    assert_eq!(stage.databases(), ["postgres", "tpl_my_app_main", &database]);

    // The fork is the environment, and its parts are fields.
    let vars = stage.run(&["env", "feat/x", "--reveal"]);
    let value = |key: &str| vars.as_array().unwrap().iter().find(|v| v["key"] == key).unwrap()["value"].clone();
    assert_eq!(value("DATABASE_URL"), fork["url"]);
    assert_eq!(value("PG_HOST"), "127.0.0.1");
    assert_eq!(value("PG_PORT"), "54316");
    assert_eq!(value("PG_NAME"), database);
}

#[test]
fn a_second_worktree_reuses_the_server_and_the_template() {
    let stage = Stage::new(CONFIG, "none");
    stage.run(&["db", "fork", "feat/x"]);
    stage.forget_calls();

    let forks = stage.run(&["db", "fork", "main"]);

    assert_ne!(forks[0]["detail"]["database"], stage.run(&["db", "ls", "feat/x"])[0]["detail"]["database"]);
    let calls = stage.calls();
    assert!(!calls.iter().any(|call| call.starts_with("run -d") || call.starts_with("cp ")), "{calls:?}");
    assert!(!stage.statements().contains(&"CREATE DATABASE \"tpl_my_app_main\"".to_owned()));
}

#[test]
fn a_server_that_is_stopped_is_started_not_recreated() {
    let stage = Stage::new(CONFIG, "stopped");
    stage.run(&["db", "fork", "feat/x"]);
    let calls = stage.calls();
    assert!(calls.contains(&"start canopy-pg-16".to_owned()), "{calls:?}");
    assert!(!calls.iter().any(|call| call.starts_with("run -d")));
}

#[test]
fn a_custom_dump_goes_through_pg_restore_whose_warnings_are_not_failures() {
    let stage = Stage::new(&CONFIG.replace("sql: db/seed.sql", "dump: db/seed.dump"), "running");

    stage.run(&["db", "fork", "feat/x"]);

    let calls = stage.calls();
    let restore = calls.iter().find(|call| call.contains(" pg_restore ")).expect("pg_restore");
    assert!(restore.ends_with("-d tpl_my_app_main --no-owner --no-privileges /tmp/canopy-seed-seed.dump"), "{restore}");
    assert!(calls.contains(&"exec canopy-pg-16 rm -f /tmp/canopy-seed-seed.dump".to_owned()), "the copy is cleaned up");
}

#[test]
fn a_seed_command_runs_against_the_template_with_the_url_where_it_expects_it() {
    let config =
        CONFIG.replace("seed: { sql: db/seed.sql }", "seed: { command: 'echo \"$DATABASE_URL\" > migrated.txt' }");
    let stage = Stage::new(&config, "running");

    stage.run(&["db", "fork", "feat/x"]);

    let seen = std::fs::read_to_string(stage.fx.root.join("migrated.txt")).unwrap();
    assert_eq!(seen.trim(), "postgresql://canopy:canopy@127.0.0.1:54316/tpl_my_app_main");
}

#[test]
fn a_seed_that_fails_leaves_no_half_built_template_behind() {
    let stage = Stage::new(CONFIG, "running");
    std::fs::write(stage.base.join("seed-fails"), "").unwrap();

    let message = stage.fails(&["db", "fork", "feat/x"], "db_failed");

    assert_eq!(message, "database main: psql: psql:/tmp/seed.sql:1: ERROR:  syntax error");
    assert_eq!(stage.databases(), ["postgres"], "every later fork would have cloned it");
    assert_eq!(stage.run(&["db", "ls", "feat/x"]), serde_json::json!([]));
}

#[test]
fn a_fork_can_be_cloned_from_another_branchs_fork() {
    let stage = Stage::new(CONFIG, "running");
    let theirs = stage.run(&["db", "fork", "main"])[0]["detail"]["database"].as_str().unwrap().to_owned();

    let mine = stage.run(&["db", "fork", "feat/x", "--from", "main"]);

    assert_eq!(mine[0]["forked_from"], "worktree main");
    assert_eq!(mine[0]["source"], theirs);
    let database = mine[0]["detail"]["database"].as_str().unwrap();
    assert!(stage.statements().contains(&format!("CREATE DATABASE \"{database}\" TEMPLATE \"{theirs}\"")));

    // A branch with no fork is an error, not an empty database that looks like success.
    stage.run(&["db", "drop", "main"]);
    let message = stage.fails(&["db", "reset", "main", "feat/x", "--from", "main"], "db_failed");
    assert_eq!(message, "database main: main has no fork of it to copy");
}

#[test]
fn an_empty_fork_skips_the_template_and_gets_its_extensions_itself() {
    let stage = Stage::new(CONFIG, "running");

    let forks = stage.run(&["db", "fork", "feat/x", "--from", "empty"]);

    let database = forks[0]["detail"]["database"].as_str().unwrap();
    assert!(forks[0].get("source").is_none(), "nothing was cloned: {forks}");
    let sql = stage.statements();
    assert!(sql.contains(&format!("CREATE DATABASE \"{database}\"")));
    assert!(!sql.iter().any(|s| s.contains("tpl_my_app_main")), "{sql:#?}");
    assert_eq!(stage.calls().iter().filter(|call| call.contains("CREATE EXTENSION")).count(), 2);
}

#[test]
fn ls_asks_the_server_and_a_fork_that_is_gone_is_missing() {
    let stage = Stage::new(CONFIG, "running");
    let database = stage.run(&["db", "fork", "feat/x"])[0]["detail"]["database"].as_str().unwrap().to_owned();
    assert_eq!(stage.run(&["db", "ls", "feat/x"])[0]["status"], "ready");

    // Dropped behind our back.
    let kept: Vec<String> = stage.databases().into_iter().filter(|name| *name != database).collect();
    std::fs::write(stage.base.join("dbs"), format!("{}\n", kept.join("\n"))).unwrap();

    let listed = stage.run(&["db", "ls", "feat/x"]);
    assert_eq!(listed[0]["status"], "missing");
    // And `fork` makes it again rather than trusting the record.
    assert_eq!(stage.run(&["db", "fork", "feat/x"])[0]["status"], "ready");
    assert!(stage.databases().contains(&database));
}

#[test]
fn removing_a_worktree_drops_its_fork_on_the_server() {
    let stage = Stage::new(CONFIG, "running");
    let database = stage.run(&["db", "fork", "feat/x"])[0]["detail"]["database"].as_str().unwrap().to_owned();

    let removed = stage.run(&["rm", "feat/x", "--force", "--delete-branch", "always"]);

    assert_eq!(removed["databases_dropped"], serde_json::json!(["main"]));
    assert!(stage.statements().contains(&format!("DROP DATABASE IF EXISTS \"{database}\"")));
    assert_eq!(stage.databases(), ["postgres", "tpl_my_app_main"], "the fork goes, the template stays");
}

#[test]
fn template_rebuilds_from_the_seed_and_leaves_forks_alone() {
    let stage = Stage::new(CONFIG, "running");
    let database = stage.run(&["db", "fork", "feat/x"])[0]["detail"]["database"].as_str().unwrap().to_owned();
    stage.forget_calls();

    assert_eq!(stage.run(&["db", "template", "feat/x"]), serde_json::json!({ "rebuilt": ["tpl_my_app_main"] }));

    let sql = stage.statements();
    let at =
        |needle: &str| sql.iter().position(|s| s == needle).unwrap_or_else(|| panic!("never sent {needle}: {sql:#?}"));
    assert!(at("DROP DATABASE IF EXISTS \"tpl_my_app_main\"") < at("CREATE DATABASE \"tpl_my_app_main\""));
    assert!(stage.calls().iter().any(|call| call.starts_with("cp ")), "seeded again");
    assert!(stage.databases().contains(&database));
    // SQLite has no template to rebuild: its seed file is read at fork time.
    let sqlite = Stage::new("version: 1\ndatabases:\n  main:\n    adapter: sqlite\n", "none");
    assert_eq!(sqlite.run(&["db", "template", "main"]), serde_json::json!({ "rebuilt": [] }));
}

#[test]
fn no_docker_is_an_error_that_says_so_and_never_blocks_a_removal() {
    let stage = Stage::new(CONFIG, "running");
    stage.run(&["db", "fork", "feat/x"]);
    std::fs::write(stage.base.join("daemon-down"), "").unwrap();

    let message = stage.fails(&["db", "reset", "main", "feat/x"], "db_failed");
    assert_eq!(message, "database main: docker is not available: Cannot connect to the Docker daemon");

    // The environment still resolves: a server fork is taken at its record's word there.
    let vars = stage.run(&["env", "feat/x", "--reveal"]);
    assert!(vars.as_array().unwrap().iter().any(|v| v["key"] == "DATABASE_URL" && v["source"] == "database"));

    // And a worktree can always be removed, whatever docker is doing.
    let removed = stage.run(&["rm", "feat/x", "--force", "--delete-branch", "always"]);
    assert_eq!(removed["branch_deleted"], true);
}
