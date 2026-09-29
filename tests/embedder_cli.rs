//! The settings an embedder runs `canopyd` with: one port registry shared by every project
//! (`CANOPYD_PORTS_FILE`), its own default range (`CANOPYD_PORT_RANGE`), ports it allocates
//! itself (`ports --reserve`), and its own out-of-repo config (`CANOPYD_CONFIG`).

mod fixture;

use camino::Utf8PathBuf;
use fixture::{Fixture, err_envelope, ok_envelope};

/// Enough ports that two projects on `main` would collide in a tiny range if they did not
/// share a table.
const PORTS: &str = "version: 1\nports:\n  web: {}\n  api: {}\n";

/// A repository with ports declared.
fn project() -> Fixture {
    let fx = Fixture::new();
    fx.commit(&[("canopy.yaml", PORTS)], "add config");
    fx
}

fn cwt(fx: &Fixture, registry: &Utf8PathBuf, range: &str) -> std::process::Command {
    let mut command = fx.cwt();
    command.env("CANOPYD_PORTS_FILE", registry.as_str()).env("CANOPYD_PORT_RANGE", range);
    command
}

fn data(out: &std::process::Output) -> serde_json::Value {
    ok_envelope(&out.stdout)["data"].clone()
}

fn shared() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().join("ports.json")).unwrap();
    (dir, path)
}

#[test]
fn two_projects_sharing_a_registry_never_share_a_port() {
    // Four ports in a range of four: without one table, both projects' `main` would start from
    // the same hash and at least one number would be handed out twice.
    let (_dir, registry) = shared();
    let (a, b) = (project(), project());
    let range = "41000-41003";
    let first = data(&cwt(&a, &registry, range).args(["ports", "main", "--json"]).output().unwrap());
    let second = data(&cwt(&b, &registry, range).args(["ports", "main", "--json"]).output().unwrap());

    let mut all: Vec<u64> = [&first, &second]
        .iter()
        .flat_map(|table| table.as_object().unwrap().values().map(|port| port.as_u64().unwrap()))
        .collect();
    assert!(all.iter().all(|port| (41000..=41003).contains(port)), "{all:?}");
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 4, "{first} {second}");

    // Each project sees its own rows, and hands back only its own.
    let own = data(&cwt(&a, &registry, range).args(["ports", "--all", "--json"]).output().unwrap());
    assert_eq!(own.as_array().unwrap().len(), 2, "{own}");
    let released = data(&cwt(&a, &registry, range).args(["ports", "main", "--release", "--json"]).output().unwrap());
    assert_eq!(released["released"], 2);
    let still = data(&cwt(&b, &registry, range).args(["ports", "main", "--json"]).output().unwrap());
    assert_eq!(still, second, "releasing one project's main took the other's ports");
}

#[test]
fn gc_leaves_another_projects_rows_alone() {
    // To project A, project B's `feat/y` is a branch with no worktree — stale, by A's lights.
    let (_dir, registry) = shared();
    let (a, b) = (project(), project());
    b.branch("feat/y");
    let held = data(&cwt(&b, &registry, "41100-41199").args(["ports", "feat/y", "--json"]).output().unwrap());

    let swept = data(&cwt(&a, &registry, "41100-41199").args(["gc", "--json"]).output().unwrap());
    assert_eq!(swept["ports_released"], 0, "{swept}");
    let after = data(&cwt(&b, &registry, "41100-41199").args(["ports", "feat/y", "--json"]).output().unwrap());
    assert_eq!(after, held);
}

#[test]
fn a_reserved_port_is_held_against_everyone() {
    let (_dir, registry) = shared();
    let (a, b) = (project(), project());
    let table = data(
        &cwt(&a, &registry, "41200-41201")
            .args(["ports", "main", "--reserve", "db-main=41200", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(table["db-main"], 41200);

    // The other project cannot pin it, and allocation walks past it.
    let refused =
        cwt(&b, &registry, "41200-41201").args(["ports", "main", "--reserve", "db=41200", "--json"]).output().unwrap();
    err_envelope(&refused.stdout, "port_in_use");
    let exhausted = cwt(&b, &registry, "41200-41201").args(["ports", "main", "--json"]).output().unwrap();
    // Two ports wanted, one left in the range: the range is exhausted rather than shared.
    err_envelope(&exhausted.stdout, "port_in_use");
}

#[test]
fn a_reservation_that_is_not_name_equals_port_is_refused() {
    let (_dir, registry) = shared();
    let a = project();
    for bad in ["db-main", "=41300", "db=http", "db=0"] {
        let out =
            cwt(&a, &registry, "41300-41399").args(["ports", "main", "--reserve", bad, "--json"]).output().unwrap();
        err_envelope(&out.stdout, "config_invalid");
    }
}

#[test]
fn a_range_that_is_not_from_to_is_refused() {
    let (_dir, registry) = shared();
    let a = project();
    let out = cwt(&a, &registry, "forty-thousand").args(["ports", "main", "--json"]).output().unwrap();
    err_envelope(&out.stdout, "config_invalid");
}

#[test]
fn canopyd_config_names_the_out_of_repo_file() {
    // A repository with no canopy.yaml of its own, and an embedder that keeps one for it.
    let fx = Fixture::new();
    let elsewhere = fx.home.join("embedder/project/canopy.yaml");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, PORTS).unwrap();

    let path =
        data(&fx.cwt().env("CANOPYD_CONFIG", elsewhere.as_str()).args(["config", "path", "--json"]).output().unwrap());
    assert_eq!(path["path"], elsewhere.as_str(), "{path}");
    let ports =
        data(&fx.cwt().env("CANOPYD_CONFIG", elsewhere.as_str()).args(["ports", "main", "--json"]).output().unwrap());
    assert_eq!(ports.as_object().unwrap().len(), 2, "{ports}");

    // A committed file still wins.
    fx.commit(&[("canopy.yaml", "version: 1\n")], "commit one");
    let path =
        data(&fx.cwt().env("CANOPYD_CONFIG", elsewhere.as_str()).args(["config", "path", "--json"]).output().unwrap());
    assert_eq!(path["path"], fx.root.join("canopy.yaml").as_str());
}
