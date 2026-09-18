//! `canopyd` — git worktree dev environments driven by `canopy.yaml`.
//!
//! One crate, two faces: a library other Rust programs embed, and the `canopyd` binary. The
//! binary is a printf over the library — every subcommand calls exactly one method here and
//! serializes the result — so the `--json` contract cannot drift from the API.
//!
//! Two invariants hold everywhere:
//!
//! - **`git worktree list` is the registry.** Persisted state covers only what git cannot know:
//!   which processes we started, and which ports are taken.
//! - **Progress goes to stderr, results to stdout.** A caller can pipe stdout into `jq` while a
//!   human watches the same command work.

#![cfg(unix)]
#![forbid(unsafe_code)]

pub mod config;
pub mod container;
pub mod copy;
pub mod db;
pub mod doctor;
pub mod env;
pub mod error;
pub mod git;
pub mod health;
pub mod hook;
pub mod paths;
pub mod ports;
pub mod proc;
pub mod repo;
pub mod service;
pub mod setup;
pub mod supervise;
pub mod wire;
pub mod worktree;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

pub use config::{CanopyConfig, ConfigSource, Diagnostic, LocatedConfig, Parsed, Severity, WorktreeSpec, parse_str};
pub use db::{DbInstance, ForkStatus};
pub use doctor::{Finding, Report, Severity as FindingSeverity, Swept};
pub use env::{EnvSource, EnvTable, EnvVar, Facts};
pub use error::{Error, ErrorCode, Result};
pub use proc::{ProcessRecord, ProcessState, SpawnRequest, StopOutcome};
pub use repo::{WorktreeEntry, parse_worktree_list};
pub use service::{RunState, ServiceContext, ServiceStatus};
pub use setup::{SetupOptions, SetupOutcome, StepOutcome, StepResult, Stream, run_setup};
pub use supervise::{Control, Event, Exit, Rejection, SuperviseOptions, SuperviseOutcome};
pub use wire::{ENVELOPE_VERSION, Envelope};
pub use worktree::{BranchSpec, CreateOptions, CreateOutcome, DeleteBranch, RemoveOptions, RemoveOutcome};

use git::Git;
use repo::Repo;

/// The entry point for everything. Cheap to construct: discovery is one `git rev-parse`,
/// and the config is read lazily so `list` costs nothing extra in a repo that has none.
#[derive(Debug)]
pub struct Canopy {
    repo: Repo,
    config: std::cell::OnceCell<Option<(LocatedConfig, Parsed)>>,
}

impl Canopy {
    /// Opens the repository containing `cwd`.
    pub fn open(cwd: &Utf8Path) -> Result<Canopy> {
        Canopy::open_with(cwd, Git::default())
    }

    /// Opens with an explicit git binary — the seam tests use to pin behaviour.
    pub fn open_with(cwd: &Utf8Path, git: Git) -> Result<Canopy> {
        Ok(Canopy { repo: Repo::discover(git, cwd)?, config: std::cell::OnceCell::new() })
    }

    /// The `canopy.yaml` in effect, parsed and linted, with the path it came from.
    ///
    /// `None` means no config exists anywhere in the search path — a normal state for a repo
    /// nobody has configured yet, not an error.
    pub fn config(&self) -> Option<&(LocatedConfig, Parsed)> {
        self.config
            .get_or_init(|| {
                let root = self.repo.root.as_deref()?;
                let main = self.repo.worktrees().ok().and_then(|list| list.first().map(|e| e.path.clone()));
                let located = config::locate(root, main.as_deref(), &self.repo.name())?;
                let parsed = config::load::load_file(&located.path).ok()?;
                Some((located, parsed))
            })
            .as_ref()
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// What this repository is and where its pieces live.
    pub fn info(&self) -> Result<RepoInfo> {
        Ok(RepoInfo {
            name: self.repo.name(),
            root: self.repo.root.clone(),
            common_dir: self.repo.common_dir.clone(),
            git_dir: self.repo.git_dir.clone(),
            bare: self.repo.is_bare(),
            worktrees: self.repo.worktrees()?.len(),
        })
    }

    /// Every worktree git knows about.
    pub fn list(&self) -> Result<Vec<WorktreeEntry>> {
        self.repo.worktrees()
    }

    /// The `worktree.path` template in effect. Falls back to the built-in default when the
    /// repository has no config — you can create a worktree in a repo nobody has configured.
    pub fn worktree_template(&self) -> String {
        self.config()
            .and_then(|(_, parsed)| parsed.config.as_ref())
            .map(|config| config.worktree.path.clone())
            .unwrap_or_else(|| config::WorktreeSpec::default().path)
    }

    /// Where this branch's worktree would live. Pure: the branch need not exist.
    pub fn path_for(&self, branch: &str, name: Option<&str>) -> Result<Utf8PathBuf> {
        self.repo.worktree_path_for(&self.worktree_template(), branch, name)
    }

    /// The base branch for a new worktree: the config's `worktree.base`, else the repository's
    /// own default branch.
    pub fn default_base(&self) -> Option<String> {
        self.config()
            .and_then(|(_, parsed)| parsed.config.as_ref())
            .and_then(|config| config.worktree.base.clone())
            .or_else(|| self.repo.default_branch())
    }

    /// Creates a worktree.
    pub fn create(
        &self,
        branch: &worktree::BranchSpec,
        options: &worktree::CreateOptions,
    ) -> Result<worktree::CreateOutcome> {
        self.repo.create_worktree(&self.worktree_template(), branch, options)
    }

    /// The port registry for this repository, kept in the common git dir so every worktree
    /// sees one table.
    pub fn ports_path(&self) -> Utf8PathBuf {
        self.repo.common_dir.join("canopy").join("ports.json")
    }

    /// Allocate-if-absent for every port the config declares, and return the table. Idempotent:
    /// the numbers do not move once a branch has them.
    pub fn ports_for(&self, branch: &str) -> Result<std::collections::BTreeMap<String, u16>> {
        let declared = self
            .config()
            .and_then(|(_, parsed)| parsed.config.as_ref())
            .map(|config| config.ports.clone())
            .unwrap_or_default();
        if declared.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }
        let path = self.ports_path();
        // `ports_path` is always a join, so there is a parent; `map_or` says that without a
        // branch nothing can take.
        path.parent().map_or(Ok(()), std::fs::create_dir_all)?;
        let mut registry = ports::Registry::load(&path).map_err(Error::from)?;
        let table = registry.allocate(&self.repo.name(), branch, &declared).map_err(Error::from)?;
        registry.save().map_err(Error::from)?;
        Ok(table)
    }

    /// Hands a branch's ports back to the pool. Returns how many rows went.
    pub fn release_ports(&self, branch: &str) -> Result<usize> {
        let path = self.ports_path();
        if !path.exists() {
            return Ok(0);
        }
        let mut registry = ports::Registry::load(&path).map_err(Error::from)?;
        let removed = registry.release(branch);
        if removed > 0 {
            registry.save().map_err(Error::from)?;
        }
        Ok(removed)
    }

    /// The resolved environment for a branch's worktree.
    pub fn env_for(&self, branch: &str, worktree: &Utf8Path) -> Result<env::EnvTable> {
        self.env_for_with(branch, worktree, &std::collections::BTreeMap::new())
    }

    /// [`Canopy::env_for`] with the caller's own variables layered last.
    ///
    /// For an embedder whose environment is richer than this crate can resolve on its own — a
    /// database URL from a fork it made, a per-worktree setting it stores. They go through the
    /// resolver as its override layer rather than being patched onto the finished table, so they
    /// are tagged [`env::EnvSource::Override`], can be interpolated by a service's own `env:`
    /// through `${env.KEY}`, and land in the written env file like everything else.
    pub fn env_for_with(
        &self,
        branch: &str,
        worktree: &Utf8Path,
        overrides: &std::collections::BTreeMap<String, String>,
    ) -> Result<env::EnvTable> {
        let ports = self.ports_for(branch)?;
        // Only forks that are actually there: see `db::ready`.
        let project_root = self.project_path();
        let state = self.state_dir(branch);
        let no_env = std::collections::BTreeMap::new();
        let project_name = self.project_name();
        let databases = db::ready(&db::DbContext {
            project_path: &project_root,
            project: &project_name,
            state: &state,
            env: &no_env,
        })?;
        let default_config = config::CanopyConfig::empty();
        let config = self.config().and_then(|(_, parsed)| parsed.config.as_ref()).unwrap_or(&default_config);
        let name = worktree.file_name().unwrap_or(branch).to_owned();
        let project = self.project_name();
        let project_path = self.repo.root.clone().unwrap_or_else(|| self.repo.common_dir.clone());
        let facts = env::Facts {
            worktree_name: &name,
            worktree_path: worktree,
            branch,
            project: &project,
            project_path: &project_path,
            ports: &ports,
            databases: &databases,
        };
        Ok(env::resolve(config, &facts, overrides))
    }

    /// What the project is called. A config that names itself means it: `${project.name}`,
    /// `CANOPY_PROJECT` and an image tag should say what the project is called, not what
    /// someone happened to call the directory they cloned into. The directory name is the
    /// fallback, not the answer.
    pub fn project_name(&self) -> String {
        let named = self.config().and_then(|(_, parsed)| parsed.config.as_ref()).and_then(|config| config.name.clone());
        named.unwrap_or_else(|| self.repo.name())
    }

    /// The primary checkout, or the git directory of a bare repository. Seeds are named
    /// relative to it, so every worktree forks the same one.
    pub fn project_path(&self) -> Utf8PathBuf {
        self.repo.root.clone().unwrap_or_else(|| self.repo.common_dir.clone())
    }

    /// The databases `canopy.yaml` declares. None when there is no config.
    fn declared_databases(&self) -> std::collections::BTreeMap<String, config::DatabaseSpec> {
        self.config().and_then(|(_, parsed)| parsed.config.as_ref()).map(|c| c.databases.clone()).unwrap_or_default()
    }

    /// Forks the branch's databases that do not have a fork yet, and reports every selected one.
    pub fn db_fork(
        &self,
        branch: &str,
        only: Option<&std::collections::BTreeSet<String>>,
        from: &DbFrom,
    ) -> Result<Vec<db::DbInstance>> {
        let (project, state) = (self.project_path(), self.state_dir(branch));
        let theirs = from.branch().map(|other| self.state_dir(other));
        let source = from.source(theirs.as_deref());
        let ctx = db::DbContext {
            project_path: &project,
            project: &self.project_name(),
            state: &state,
            env: &std::collections::BTreeMap::new(),
        };
        Ok(db::fork(&self.declared_databases(), only, &ctx, &source)?)
    }

    /// The branch's recorded forks, as they stand on disk.
    pub fn db_list(&self, branch: &str) -> Result<Vec<db::DbInstance>> {
        let (project, state) = (self.project_path(), self.state_dir(branch));
        Ok(db::list(&db::DbContext {
            project_path: &project,
            project: &self.project_name(),
            state: &state,
            env: &std::collections::BTreeMap::new(),
        })?)
    }

    /// Throws one fork away and makes it again.
    pub fn db_reset(&self, branch: &str, name: &str, from: &DbFrom) -> Result<db::DbInstance> {
        let (project, state) = (self.project_path(), self.state_dir(branch));
        let theirs = from.branch().map(|other| self.state_dir(other));
        let source = from.source(theirs.as_deref());
        let ctx = db::DbContext {
            project_path: &project,
            project: &self.project_name(),
            state: &state,
            env: &std::collections::BTreeMap::new(),
        };
        Ok(db::reset(&self.declared_databases(), name, &ctx, &source)?)
    }

    /// Rebuilds the seed templates of the selected databases and returns their names. Only
    /// server databases have one: SQLite's template is its seed file, read at fork time.
    pub fn db_refresh_templates(
        &self,
        branch: &str,
        only: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<Vec<String>> {
        let (project, state) = (self.project_path(), self.state_dir(branch));
        let ctx = db::DbContext {
            project_path: &project,
            project: &self.project_name(),
            state: &state,
            env: &std::collections::BTreeMap::new(),
        };
        Ok(db::refresh_templates(&self.declared_databases(), only, &ctx)?)
    }

    /// Removes the branch's forks, all of them or `only`, and returns the names that had one.
    pub fn db_drop(&self, branch: &str, only: Option<&std::collections::BTreeSet<String>>) -> Result<Vec<String>> {
        let (project, state) = (self.project_path(), self.state_dir(branch));
        let ctx = db::DbContext {
            project_path: &project,
            project: &self.project_name(),
            state: &state,
            env: &std::collections::BTreeMap::new(),
        };
        Ok(db::drop_forks(&self.declared_databases(), only, &ctx)?)
    }

    /// Where every worktree's state lives. `doctor` and `gc` walk this to find debris.
    pub fn state_root(&self) -> Utf8PathBuf {
        self.repo.common_dir.join("canopy").join("worktrees")
    }

    /// Per-worktree state: process records and logs, under the common git dir so it survives
    /// `git clean` and is never mistaken for part of the checkout.
    pub fn state_dir(&self, branch: &str) -> Utf8PathBuf {
        self.repo.common_dir.join("canopy").join("worktrees").join(paths::sanitize(branch))
    }

    /// Removes the worktree for a branch name or a path.
    pub fn remove(&self, target: &str, options: &worktree::RemoveOptions) -> Result<worktree::RemoveOutcome> {
        self.repo.remove_worktree(target, options)
    }
}

/// `canopyd info`.
#[derive(Debug, Clone, Serialize)]
pub struct RepoInfo {
    /// Stable repository name — the common dir's parent.
    pub name: String,
    /// The checkout we were invoked from; absent for a bare repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<Utf8PathBuf>,
    /// Identifies the repository from any worktree.
    pub common_dir: Utf8PathBuf,
    pub git_dir: Utf8PathBuf,
    pub bare: bool,
    pub worktrees: usize,
}

/// What a fork starts as, in the terms a caller has: a word, or another branch's name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DbFrom {
    /// The project's seed.
    #[default]
    Template,
    Empty,
    /// Another worktree's fork, named by its branch.
    Branch(String),
}

impl DbFrom {
    /// `template`, `empty`, or anything else as a branch name.
    pub fn parse(text: &str) -> DbFrom {
        match text {
            "template" => DbFrom::Template,
            "empty" => DbFrom::Empty,
            branch => DbFrom::Branch(branch.to_owned()),
        }
    }

    fn branch(&self) -> Option<&str> {
        match self {
            DbFrom::Branch(branch) => Some(branch),
            DbFrom::Template | DbFrom::Empty => None,
        }
    }

    /// `state` is the other branch's state directory, and is only consulted for [`DbFrom::Branch`].
    fn source<'a>(&'a self, state: Option<&'a Utf8Path>) -> db::ForkSource<'a> {
        match (self, state) {
            (DbFrom::Branch(branch), Some(state)) => db::ForkSource::Worktree { branch, state },
            (DbFrom::Empty, _) => db::ForkSource::Empty,
            (DbFrom::Template | DbFrom::Branch(_), _) => db::ForkSource::Template,
        }
    }
}

/// The documentation's code examples, compiled as doctests.
///
/// Documentation that does not compile is worse than none: it is confidently wrong. Attaching
/// the files here means a rename or a signature change breaks the build rather than quietly
/// making the docs a lie.
#[cfg(doctest)]
mod doc_examples {
    #[doc = include_str!("../README.md")]
    mod readme {}

    #[doc = include_str!("../docs/json-api.md")]
    mod json_api {}
}
