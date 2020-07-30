use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use git2::{Oid, Repository, Signature};
use log::*;

use crate::args::DeleteFilter;
use crate::branch::{get_fetch_upstream, get_push_upstream, RemoteBranch, RemoteTrackingBranch};
use crate::subprocess::is_merged_by_rev_list;
use crate::util::ForceSendSync;
use crate::{config, Git};

#[derive(Default, Eq, PartialEq, Debug)]
pub struct MergedOrStray {
    // local branches
    pub merged_locals: HashSet<String>,
    pub stray_locals: HashSet<String>,

    /// remote refs
    pub merged_remotes: HashSet<RemoteBranch>,
    pub stray_remotes: HashSet<RemoteBranch>,
}

impl MergedOrStray {
    pub fn accumulate(mut self, mut other: Self) -> Self {
        self.merged_locals.extend(other.merged_locals.drain());
        self.stray_locals.extend(other.stray_locals.drain());
        self.merged_remotes.extend(other.merged_remotes.drain());
        self.stray_remotes.extend(other.stray_remotes.drain());

        self
    }

    pub fn locals(&self) -> Vec<&str> {
        self.merged_locals
            .iter()
            .chain(self.stray_locals.iter())
            .map(String::as_str)
            .collect()
    }

    pub fn remotes(&self) -> Vec<&RemoteBranch> {
        self.merged_remotes
            .iter()
            .chain(self.stray_remotes.iter())
            .collect()
    }
}

#[derive(Default, Eq, PartialEq, Debug)]
pub struct MergedOrStrayAndKeptBacks {
    pub to_delete: MergedOrStray,
    pub kept_backs: HashMap<String, Reason>,
    pub kept_back_remotes: HashMap<RemoteBranch, Reason>,
}

#[derive(Clone, Eq, PartialEq, Debug, Ord, PartialOrd)]
pub struct Reason {
    pub original_classification: OriginalClassification,
    pub message: &'static str,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Ord, PartialOrd, Hash)]
pub enum OriginalClassification {
    MergedLocal,
    StrayLocal,
    MergedRemote,
    StrayRemote,
}

impl std::fmt::Display for OriginalClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OriginalClassification::MergedLocal => write!(f, "merged local"),
            OriginalClassification::StrayLocal => write!(f, "stray local"),
            OriginalClassification::MergedRemote => write!(f, "merged remote"),
            OriginalClassification::StrayRemote => write!(f, "stray remote"),
        }
    }
}

impl MergedOrStrayAndKeptBacks {
    pub fn keep_base(&mut self, repo: &Repository, base_refs: &HashSet<String>) -> Result<()> {
        trace!("base_refs: {:#?}", base_refs);
        self.kept_backs.extend(keep_branches(
            repo,
            &base_refs,
            Reason {
                original_classification: OriginalClassification::MergedLocal,
                message: "a base branch",
            },
            &mut self.to_delete.merged_locals,
        )?);
        self.kept_backs.extend(keep_branches(
            repo,
            &base_refs,
            Reason {
                original_classification: OriginalClassification::StrayLocal,
                message: "a base branch",
            },
            &mut self.to_delete.stray_locals,
        )?);
        self.kept_back_remotes.extend(keep_remote_branches(
            repo,
            &base_refs,
            Reason {
                original_classification: OriginalClassification::MergedRemote,
                message: "a base branch",
            },
            &mut self.to_delete.merged_remotes,
        )?);
        self.kept_back_remotes.extend(keep_remote_branches(
            repo,
            &base_refs,
            Reason {
                original_classification: OriginalClassification::StrayRemote,
                message: "a base branch",
            },
            &mut self.to_delete.stray_remotes,
        )?);
        Ok(())
    }

    pub fn keep_protected(
        &mut self,
        repo: &Repository,
        protected_refs: &HashSet<String>,
    ) -> Result<()> {
        trace!("protected_refs: {:#?}", protected_refs);
        self.kept_backs.extend(keep_branches(
            repo,
            &protected_refs,
            Reason {
                original_classification: OriginalClassification::MergedLocal,
                message: "a protected branch",
            },
            &mut self.to_delete.merged_locals,
        )?);
        self.kept_backs.extend(keep_branches(
            repo,
            &protected_refs,
            Reason {
                original_classification: OriginalClassification::StrayLocal,
                message: "a protected branch",
            },
            &mut self.to_delete.stray_locals,
        )?);
        self.kept_back_remotes.extend(keep_remote_branches(
            repo,
            &protected_refs,
            Reason {
                original_classification: OriginalClassification::MergedRemote,
                message: "a protected branch",
            },
            &mut self.to_delete.merged_remotes,
        )?);
        self.kept_back_remotes.extend(keep_remote_branches(
            repo,
            &protected_refs,
            Reason {
                original_classification: OriginalClassification::StrayRemote,
                message: "a protected branch",
            },
            &mut self.to_delete.stray_remotes,
        )?);
        Ok(())
    }

    /// `hub-cli` can checkout pull request branch. However they are stored in `refs/pulls/`.
    /// This prevents to remove them.
    pub fn keep_non_heads_remotes(&mut self) {
        let mut merged_remotes = HashSet::new();
        for remote_branch in &self.to_delete.merged_remotes {
            if remote_branch.refname.starts_with("refs/heads/") {
                merged_remotes.insert(remote_branch.clone());
            } else {
                trace!("filter-out: merged remote ref {}", remote_branch);
                self.kept_back_remotes.insert(
                    remote_branch.clone(),
                    Reason {
                        original_classification: OriginalClassification::MergedRemote,
                        message: "a non-heads remote branch",
                    },
                );
            }
        }
        self.to_delete.merged_remotes = merged_remotes;

        let mut stray_remotes = HashSet::new();
        for remote_branch in &self.to_delete.stray_remotes {
            if remote_branch.refname.starts_with("refs/heads/") {
                stray_remotes.insert(remote_branch.clone());
            } else {
                trace!("filter-out: stray_remotes remote ref {}", remote_branch);
                self.kept_back_remotes.insert(
                    remote_branch.clone(),
                    Reason {
                        original_classification: OriginalClassification::StrayRemote,
                        message: "a non-heads remote branch",
                    },
                );
            }
        }
        self.to_delete.stray_remotes = stray_remotes;
    }

    pub fn apply_filter(&mut self, filter: &DeleteFilter) -> Result<()> {
        trace!("Before filter: {:#?}", self);
        trace!("Applying filter: {:?}", filter);
        if !filter.filter_merged_local() {
            trace!(
                "filter-out: merged local branches {:?}",
                self.to_delete.merged_locals
            );
            self.kept_backs
                .extend(self.to_delete.merged_locals.drain().map(|refname| {
                    (
                        refname,
                        Reason {
                            original_classification: OriginalClassification::MergedLocal,
                            message: "out of filter scope",
                        },
                    )
                }));
        }
        if !filter.filter_stray_local() {
            trace!(
                "filter-out: stray local branches {:?}",
                self.to_delete.stray_locals
            );
            self.kept_backs
                .extend(self.to_delete.stray_locals.drain().map(|refname| {
                    (
                        refname,
                        Reason {
                            original_classification: OriginalClassification::StrayLocal,
                            message: "out of filter scope",
                        },
                    )
                }));
        }

        let mut merged_remotes = HashSet::new();
        for remote_branch in &self.to_delete.merged_remotes {
            if filter.filter_merged_remote(&remote_branch.remote) {
                merged_remotes.insert(remote_branch.clone());
            } else {
                trace!("filter-out: merged remote ref {}", remote_branch);
                self.kept_back_remotes.insert(
                    remote_branch.clone(),
                    Reason {
                        original_classification: OriginalClassification::MergedRemote,
                        message: "out of filter scope",
                    },
                );
            }
        }
        self.to_delete.merged_remotes = merged_remotes;

        let mut stray_remotes = HashSet::new();
        for remote_branch in &self.to_delete.stray_remotes {
            if filter.filter_stray_remote(&remote_branch.remote) {
                stray_remotes.insert(remote_branch.clone());
            } else {
                trace!("filter-out: stray_remotes remote ref {}", remote_branch);
                self.kept_back_remotes.insert(
                    remote_branch.clone(),
                    Reason {
                        original_classification: OriginalClassification::StrayRemote,
                        message: "out of filter scope",
                    },
                );
            }
        }
        self.to_delete.stray_remotes = stray_remotes;

        Ok(())
    }

    pub fn adjust_not_to_detach(&mut self, repo: &Repository) -> Result<()> {
        if repo.head_detached()? {
            return Ok(());
        }
        let head = repo.head()?;
        let head_name = head.name().context("non-utf8 head ref name")?;

        if self.to_delete.merged_locals.contains(head_name) {
            self.to_delete.merged_locals.remove(head_name);
            self.kept_backs.insert(
                head_name.to_string(),
                Reason {
                    original_classification: OriginalClassification::MergedLocal,
                    message: "not to make detached HEAD",
                },
            );
        }
        if self.to_delete.stray_locals.contains(head_name) {
            self.to_delete.stray_locals.remove(head_name);
            self.kept_backs.insert(
                head_name.to_string(),
                Reason {
                    original_classification: OriginalClassification::StrayLocal,
                    message: "not to make detached HEAD",
                },
            );
        }
        Ok(())
    }
}

fn keep_branches(
    repo: &Repository,
    protected_refs: &HashSet<String>,
    reason: Reason,
    references: &mut HashSet<String>,
) -> Result<HashMap<String, Reason>> {
    let mut kept_back = HashMap::new();
    let mut bag = HashSet::new();
    for refname in references.iter() {
        let reference = repo.find_reference(refname)?;
        let refname = reference.name().context("non utf-8 branch ref")?;
        if protected_refs.contains(refname) {
            bag.insert(refname.to_string());
            kept_back.insert(refname.to_string(), reason.clone());
        }
    }
    for refname in bag.into_iter() {
        references.remove(&refname);
    }
    Ok(kept_back)
}

fn keep_remote_branches(
    repo: &Repository,
    protected_refs: &HashSet<String>,
    reason: Reason,
    remote_branches: &mut HashSet<RemoteBranch>,
) -> Result<HashMap<RemoteBranch, Reason>> {
    let mut kept_back = HashMap::new();
    for remote_branch in remote_branches.iter() {
        if let Some(remote_tracking) =
            RemoteTrackingBranch::from_remote_branch(repo, remote_branch)?
        {
            if protected_refs.contains(&remote_tracking.refname) {
                kept_back.insert(remote_branch.clone(), reason.clone());
            }
        }
    }
    for remote_branch in kept_back.keys() {
        remote_branches.remove(remote_branch);
    }
    Ok(kept_back)
}

#[derive(Debug, Clone)]
pub struct Ref {
    name: String,
    commit: String,
}

impl Ref {
    fn from_name(repo: &Repository, refname: &str) -> Result<Ref> {
        Ok(Ref {
            name: refname.to_string(),
            commit: repo
                .find_reference(refname)?
                .peel_to_commit()?
                .id()
                .to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamMergeState {
    upstream: Ref,
    merged: bool,
}

pub struct Classification {
    pub branch: Ref,
    pub branch_is_merged: bool,
    pub fetch: Option<UpstreamMergeState>,
    pub push: Option<UpstreamMergeState>,
    pub messages: Vec<&'static str>,
    pub result: MergedOrStray,
}

impl Classification {
    fn merged_or_stray_remote(
        &mut self,
        repo: &Repository,
        merge_state: &UpstreamMergeState,
    ) -> Result<()> {
        if merge_state.merged {
            self.messages
                .push("fetch upstream is merged, but forget to delete");
            self.merged_remote(repo, &merge_state.upstream)
        } else {
            self.messages.push("fetch upstream is not merged");
            self.stray_remote(repo, &merge_state.upstream)
        }
    }

    fn merged_remote(&mut self, repo: &Repository, upstream: &Ref) -> Result<()> {
        self.result
            .merged_remotes
            .insert(RemoteTrackingBranch::new(&upstream.name).remote_branch(&repo)?);
        Ok(())
    }

    fn stray_remote(&mut self, repo: &Repository, upstream: &Ref) -> Result<()> {
        self.result
            .stray_remotes
            .insert(RemoteTrackingBranch::new(&upstream.name).remote_branch(&repo)?);
        Ok(())
    }
}

/// Make sure repo and config are semantically Send + Sync.
pub fn classify(
    git: ForceSendSync<&Git>,
    merged_locals: &HashSet<String>,
    remote_heads_per_url: &HashMap<String, HashSet<String>>,
    base: &RemoteTrackingBranch,
    refname: &str,
) -> Result<Classification> {
    let branch = Ref::from_name(&git.repo, refname)?;
    let branch_is_merged =
        merged_locals.contains(refname) || is_merged(&git.repo, &base.refname, refname)?;
    let fetch = if let Some(fetch) = get_fetch_upstream(&git.repo, &git.config, refname)? {
        let upstream = Ref::from_name(&git.repo, &fetch.refname)?;
        let merged = (branch_is_merged && upstream.commit == branch.commit)
            || is_merged(&git.repo, &base.refname, &upstream.name)?;
        Some(UpstreamMergeState { upstream, merged })
    } else {
        None
    };
    let push = if let Some(push) = get_push_upstream(&git.repo, &git.config, refname)? {
        let upstream = Ref::from_name(&git.repo, &push.refname)?;
        let merged = (branch_is_merged && upstream.commit == branch.commit)
            || fetch
                .as_ref()
                .map(|x| x.merged && upstream.commit == x.upstream.commit)
                == Some(true)
            || is_merged(&git.repo, &base.refname, &upstream.name)?;
        Some(UpstreamMergeState { upstream, merged })
    } else {
        None
    };

    let mut c = Classification {
        branch,
        branch_is_merged,
        fetch: fetch.clone(),
        push: push.clone(),
        messages: vec![],
        result: MergedOrStray::default(),
    };

    match (fetch, push) {
        (Some(fetch), Some(push)) => {
            if branch_is_merged {
                c.messages.push("local is merged");
                c.result.merged_locals.insert(refname.to_string());
                c.merged_or_stray_remote(&git.repo, &fetch)?;
                c.merged_or_stray_remote(&git.repo, &push)?;
            } else if fetch.merged || push.merged {
                c.messages
                    .push("some upstreams are merged, but the local strays");
                c.result.stray_locals.insert(refname.to_string());
                c.merged_or_stray_remote(&git.repo, &push)?;
                c.merged_or_stray_remote(&git.repo, &fetch)?;
            }
        }

        (Some(upstream), None) | (None, Some(upstream)) => {
            if branch_is_merged {
                c.messages.push("local is merged");
                c.result.merged_locals.insert(refname.to_string());
                c.merged_or_stray_remote(&git.repo, &upstream)?;
            } else if upstream.merged {
                c.messages.push("upstream is merged, but the local strays");
                c.result.stray_locals.insert(refname.to_string());
                c.merged_remote(&git.repo, &upstream.upstream)?;
            }
        }

        // `hub-cli` sets config `branch.{branch_name}.remote` as URL without `remote.{remote}` entry.
        // so `get_push_upstream` and `get_fetch_upstream` returns None.
        // However we can try manual classification without `remote.{remote}` entry.
        (None, None) => {
            let remote = config::get_remote_raw(&git.config, refname)?
                .expect("should have it if it has an upstream");
            let merge = config::get_merge(&git.config, refname)?
                .expect("should have it if it has an upstream");
            let upstream_is_exists = remote_heads_per_url.contains_key(&remote)
                && remote_heads_per_url[&remote].contains(&merge);

            if upstream_is_exists && branch_is_merged {
                c.messages.push(
                    "merged local, merged remote: the branch is merged, but forgot to delete",
                );
                c.result.merged_locals.insert(refname.to_string());
                c.result.merged_remotes.insert(RemoteBranch {
                    remote,
                    refname: merge,
                });
            } else if branch_is_merged {
                c.messages
                    .push("merged local: the branch is merged, and deleted");
                c.result.merged_locals.insert(refname.to_string());
            } else if !upstream_is_exists {
                c.messages
                    .push("the branch is not merged but the remote is gone somehow");
                c.result.stray_locals.insert(refname.to_string());
            } else {
                c.messages.push("skip: the branch is alive");
            }
        }
    }

    Ok(c)
}

fn is_merged(repo: &Repository, base: &str, refname: &str) -> Result<bool> {
    let base_oid = repo.find_reference(base)?.peel_to_commit()?.id();
    let other_oid = repo.find_reference(refname)?.peel_to_commit()?.id();
    // git merge-base {base} {refname}
    let merge_base = repo.merge_base(base_oid, other_oid)?.to_string();
    Ok(is_merged_by_rev_list(repo, base, refname)?
        || is_squash_merged(repo, &merge_base, base, refname)?)
}

/// Source: https://stackoverflow.com/a/56026209
fn is_squash_merged(
    repo: &Repository,
    merge_base: &str,
    base: &str,
    refname: &str,
) -> Result<bool> {
    let tree = repo
        .revparse_single(&format!("{}^{{tree}}", refname))?
        .peel_to_tree()?;
    let tmp_sig = Signature::now("git-trim", "git-trim@squash.merge.test.local")?;
    let dangling_commit = repo.commit(
        None,
        &tmp_sig,
        &tmp_sig,
        "git-trim: squash merge test",
        &tree,
        &[&repo.find_commit(Oid::from_str(merge_base)?)?],
    )?;

    is_merged_by_rev_list(repo, base, &dangling_commit.to_string())
}