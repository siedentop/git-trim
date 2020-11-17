use std::ops::Deref;

use anyhow::Context;

/// Use with caution.
/// It makes wrapping type T to be Send + Sync.
/// Make sure T is semantically Send + Sync
#[derive(Copy, Clone)]
pub struct ForceSendSync<T>(T);

unsafe impl<T> Sync for ForceSendSync<T> {}
unsafe impl<T> Send for ForceSendSync<T> {}

impl<T> ForceSendSync<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
    pub fn unwrap(self) -> T {
        self.0
    }
}

impl<T> Deref for ForceSendSync<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Gets all remotes for a Repository. This combines two slow calls in the git2
/// API and returns the full Remote objects, not just some strings.
pub fn get_remotes<'a>(repo: &'a git2::Repository) -> anyhow::Result<Vec<git2::Remote<'a>>> {
    let mut remotes = vec![];
    for remote_name in repo.remotes()?.iter() {
        let remote_name = remote_name.context("non-utf8 remote name")?;
        let remote = repo.find_remote(&remote_name)?;
        remotes.push(remote);
    }
    Ok(remotes)
}
