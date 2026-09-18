//! Database forks through the CLI: a private copy per worktree, its URL in the environment, and
//! `${db.…}` references that resolve to it.

mod fixture;

use fixture::{Fixture, err_envelope, ok_envelope};

const CONFIG: &str = r#"version: 1
worktree:
  path: "{{ repo_path }}/../wt/{{ name }}"
databases:
  main:
    adapter: sqlite
    source: data/seed.db
    env: DATABASE_URL
  cache:
    adapter: sqlite
env:
  DATABASE_URL: file:/shared/by/everyone.db
  REPORTS_DB: ${db.main.file}
services:
  web:
    run: serve
    env:
      DSN: ${db.main.url}?mode=rwc
      CACHE: ${db.cache}
"#;

fn prepared() -> Fixture {
    let fx = Fixture::new();
    fx.commit(&[("canopy.yaml", CONFIG), ("data/seed.db", "seed-bytes")], "add config and seed");
    let out = fx.cwt().args(["new", "feat/x"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    fx
}

fn run(fx: &Fixture, args: &[&str]) -> serde_json::Value {
    let out = fx.cwt().args(args).arg("--json").output().unwrap();
    ok_envelope(&out.stdout)["data"].clone()
}

fn env_of(fx: &Fixture, branch: &str) -> Vec<serde_json::Value> {
    run(fx, &["env", branch, "--reveal"]).as_array().unwrap().clone()
}

fn var<'a>(vars: &'a [serde_json::Value], key: &str) -> Option<&'a serde_json::Value> {
    vars.iter().find(|v| v["key"] == key)
}

#[test]
fn a_fork_is_a_private_copy_of_the_seed_and_its_url_is_the_environment() {
    let fx = prepared();

    let forks = run(&fx, &["db", "fork", "feat/x"]);
    let forks = forks.as_array().unwrap();
    assert_eq!(forks.len(), 2, "every declared database is forked: {forks:?}");
    let main = forks.iter().find(|f| f["name"] == "main").unwrap();
    assert_eq!(main["status"], "ready");
    assert_eq!(main["adapter"], "sqlite");
    assert_eq!(main["env_key"], "DATABASE_URL");
    assert_eq!(main["forked_from"], "seed template");
    let file = main["detail"]["file"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(file).unwrap(), "seed-bytes");
    assert_eq!(main["url"], format!("file:{file}"));
    // It lives with the worktree's other state, not in the checkout where git would see it.
    assert!(!file.contains("/wt/feat-x/"), "{file}");

    let vars = env_of(&fx, "feat/x");
    // The fork wins over the shared URL the file sets for people running without canopyd.
    assert_eq!(var(&vars, "DATABASE_URL").unwrap()["value"], format!("file:{file}"));
    assert_eq!(var(&vars, "DATABASE_URL").unwrap()["source"], "database");
    assert_eq!(var(&vars, "CANOPY_DB_MAIN_URL").unwrap()["value"], format!("file:{file}"));
    assert_eq!(var(&vars, "CACHE_URL").unwrap()["source"], "database", "no `env:` means <NAME>_URL");
    // A field is a different value from the URL.
    assert_eq!(var(&vars, "REPORTS_DB").unwrap()["value"], file);
}

#[test]
fn a_worktree_with_no_fork_is_not_pointed_at_one() {
    let fx = prepared();

    let vars = env_of(&fx, "feat/x");

    assert_eq!(var(&vars, "DATABASE_URL").unwrap()["value"], "file:/shared/by/everyone.db");
    assert!(var(&vars, "CANOPY_DB_MAIN_URL").is_none());
    // An unresolved reference stays visible rather than becoming an empty string.
    assert_eq!(var(&vars, "REPORTS_DB").unwrap()["value"], "${db.main.file}");
}

#[test]
fn a_service_sees_the_fork_through_its_own_env() {
    let fx = prepared();
    let forks = run(&fx, &["db", "fork", "feat/x"]);
    let main =
        forks.as_array().unwrap().iter().find(|f| f["name"] == "main").unwrap()["url"].as_str().unwrap().to_owned();
    let cache =
        forks.as_array().unwrap().iter().find(|f| f["name"] == "cache").unwrap()["url"].as_str().unwrap().to_owned();

    let out = fx.cwt().args(["config", "show", "--json"]).output().unwrap();
    assert!(out.status.success());
    // `up` would hand the service exactly what `env` resolves plus its own block, and the
    // block's references are resolved against the same forks. The shell proves it.
    fx.commit(
        &[("canopy.yaml", &CONFIG.replace("run: serve", "run: echo \"$DSN $CACHE\" > seen.txt; sleep 30"))],
        "a service that says what it sees",
    );
    let wt = fx.root.parent().unwrap().join("wt/feat-x");
    fx.git_in(&wt, ["merge", "-q", "main"]);
    run(&fx, &["up", "feat/x", "--no-wait"]);
    let seen = wt.join("seen.txt");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !seen.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let _ = fx.cwt().args(["down", "feat/x"]).output();

    assert_eq!(std::fs::read_to_string(&seen).unwrap_or_default().trim(), format!("{main}?mode=rwc {cache}"));
}

#[test]
fn forking_again_keeps_the_data_and_reset_replaces_it() {
    let fx = prepared();
    let forks = run(&fx, &["db", "fork", "feat/x", "--only", "main"]);
    assert_eq!(forks.as_array().unwrap().len(), 1, "--only forks just that one");
    let file = forks[0]["detail"]["file"].as_str().unwrap().to_owned();
    std::fs::write(&file, "an afternoon of work").unwrap();

    run(&fx, &["db", "fork", "feat/x"]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "an afternoon of work");

    let fresh = run(&fx, &["db", "reset", "main", "feat/x"]);
    assert_eq!(fresh["name"], "main");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "seed-bytes");

    let empty = run(&fx, &["db", "reset", "main", "feat/x", "--from", "empty"]);
    assert_eq!(empty["forked_from"], "empty");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
}

#[test]
fn a_fork_can_start_from_another_branchs_fork() {
    let fx = prepared();
    let theirs = run(&fx, &["db", "fork", "main", "--only", "main"]);
    std::fs::write(theirs[0]["detail"]["file"].as_str().unwrap(), "what main has been working against").unwrap();

    let mine = run(&fx, &["db", "fork", "feat/x", "--only", "main", "--from", "main"]);

    assert_eq!(mine[0]["forked_from"], "worktree main");
    assert_eq!(
        std::fs::read_to_string(mine[0]["detail"]["file"].as_str().unwrap()).unwrap(),
        "what main has been working against"
    );

    // A branch with nothing to copy is an error, not an empty database that looks like success.
    let out = fx.cwt().args(["db", "reset", "cache", "feat/x", "--from", "main", "--json"]).output().unwrap();
    let envelope = err_envelope(&out.stdout, "db_failed");
    assert!(envelope["error"]["message"].as_str().unwrap().contains("main has no fork of it"), "{envelope}");
}

#[test]
fn ls_checks_the_disk_and_drop_removes_forks() {
    let fx = prepared();
    assert_eq!(run(&fx, &["db", "ls", "feat/x"]), serde_json::json!([]));
    let forks = run(&fx, &["db", "fork", "feat/x"]);
    let file = forks.as_array().unwrap().iter().find(|f| f["name"] == "main").unwrap()["detail"]["file"]
        .as_str()
        .unwrap()
        .to_owned();

    std::fs::remove_file(&file).unwrap();
    let listed = run(&fx, &["db", "ls", "feat/x"]);
    let main = listed.as_array().unwrap().iter().find(|f| f["name"] == "main").unwrap();
    assert_eq!(main["status"], "missing");
    // And a missing fork is kept out of the environment.
    assert_eq!(var(&env_of(&fx, "feat/x"), "DATABASE_URL").unwrap()["value"], "file:/shared/by/everyone.db");

    assert_eq!(run(&fx, &["db", "drop", "feat/x", "--only", "cache"]), serde_json::json!({ "dropped": ["cache"] }));
    assert_eq!(run(&fx, &["db", "drop", "feat/x"]), serde_json::json!({ "dropped": ["main"] }));
    assert_eq!(run(&fx, &["db", "drop", "feat/x"]), serde_json::json!({ "dropped": [] }));
}

#[test]
fn removing_a_worktree_takes_its_forks_with_it() {
    let fx = prepared();
    let forks = run(&fx, &["db", "fork", "feat/x"]);
    let file = forks[0]["detail"]["file"].as_str().unwrap().to_owned();
    assert!(std::path::Path::new(&file).exists());

    run(&fx, &["rm", "feat/x", "--force", "--delete-branch", "always"]);

    assert!(!std::path::Path::new(&file).exists(), "a fork must not outlive its worktree");
}

#[test]
fn an_adapter_this_version_cannot_drive_is_refused_and_warned_about() {
    let fx = Fixture::new();
    fx.commit(
        &[("canopy.yaml", "version: 1\ndatabases:\n  main:\n    adapter: redis\n  local:\n    adapter: sqlite\n")],
        "config",
    );

    let out = fx.cwt().args(["db", "fork", "main", "--json"]).output().unwrap();
    let envelope = err_envelope(&out.stdout, "db_unsupported");
    assert!(envelope["error"]["message"].as_str().unwrap().contains("adapter redis"), "{envelope}");
    assert_eq!(
        run(&fx, &["db", "ls", "main"]),
        serde_json::json!([]),
        "and nothing was forked, not even the sqlite one"
    );

    let check = fx.cwt().args(["config", "check", "--json"]).output().unwrap();
    let diagnostics: Vec<serde_json::Value> = ok_envelope(&check.stdout)["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["path"].as_str().unwrap().starts_with("databases"))
        .cloned()
        .collect();
    let paths: Vec<&str> = diagnostics.iter().map(|d| d["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["databases.main"], "sqlite is supported and says nothing: {diagnostics:?}");
    assert!(diagnostics[0]["message"].as_str().unwrap().contains("adapter redis is not supported"));
}

#[test]
fn the_human_output_is_a_table_and_says_when_there_is_nothing() {
    let fx = prepared();
    let text = |args: &[&str]| String::from_utf8(fx.cwt().args(args).output().unwrap().stdout).unwrap();

    assert_eq!(text(&["db", "ls", "feat/x"]), "no forks\n");
    assert_eq!(text(&["db", "drop", "feat/x"]), "no forks\n");
    let forked = text(&["db", "fork", "feat/x"]);
    assert!(
        forked.contains("main") && forked.contains("sqlite") && forked.contains("ready") && forked.contains("file:"),
        "{forked}"
    );
    assert!(text(&["db", "reset", "main", "feat/x"]).starts_with("main"));
    assert_eq!(text(&["db", "drop", "feat/x"]), "dropped cache\ndropped main\n");
}
