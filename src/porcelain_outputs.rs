use anyhow::Result;
use git2::Repository;

use crate::{get_remotes, TrimPlan};

/// Prints all locally to-be-deleted branches.
pub fn print_local(
    plan: &TrimPlan,
    _repo: &Repository,
    mut writer: impl std::io::Write,
) -> Result<()> {
    let mut merged_locals = Vec::new();
    for branch in &plan.to_delete {
        if let Some(local) = branch.local() {
            merged_locals.push(local.short_name().to_owned());
        }
    }

    merged_locals.sort();
    for branch in merged_locals {
        writeln!(writer, "{}", branch)?;
    }

    Ok(())
}

/// Print all remotely to-be-deleted branches in the form "<remote>/<branch_name>"
pub fn print_remote(
    plan: &TrimPlan,
    repo: &Repository,
    mut writer: impl std::io::Write,
) -> Result<()> {
    let mut merged_remotes = Vec::new();
    let remotes = get_remotes(&repo)?;
    for branch in &plan.to_delete {
        if let Some(remote) = branch.remote(&remotes)? {
            merged_remotes.push(remote);
        }
    }

    merged_remotes.sort();
    for branch in merged_remotes {
        let branch_name = &branch.refname["/refs/heads".len()..];
        writeln!(writer, "{}/{}", branch.remote, branch_name)?;
    }

    Ok(())
}

pub fn print_json(plan: &TrimPlan, _repo: &Repository, writer: impl std::io::Write) -> Result<()> {
    serde_json::to_writer(writer, &plan)?;

    Ok(())
}
