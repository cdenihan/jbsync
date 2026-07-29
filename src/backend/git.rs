//! Git backend: a repository the CLI owns, so the user never manages one.
//!
//! `jbsync` shells out to the `git` binary rather than linking libgit2. That
//! keeps the dependency budget at zero for this feature, and — more usefully —
//! means authentication is whatever the user already has working: SSH agents,
//! Keychain, Windows Credential Manager, `gh auth`, hardware keys. A linked
//! library would have to reimplement all of that.
//!
//! Merging is deliberately *not* delegated to Git. Git would merge settings
//! files line by line and can produce XML that no longer parses. Instead the
//! engine performs a setting-level three-way merge and this backend records the
//! result with `-s ours`, which keeps the merged tree while still recording the
//! remote commit as a parent so history stays honest and pushes fast-forward.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use super::{Backend, Incoming, Published, Tree};
use crate::error::{JbsyncError, Result};

/// Settings files are compared byte for byte across platforms, so Git must
/// never translate line endings or apply filters to them.
const GITATTRIBUTES: &str = "* -text -diff=auto\n";

pub struct GitBackend {
    workdir: PathBuf,
    remote: Option<String>,
    branch: String,
}

impl GitBackend {
    pub fn new(workdir: PathBuf, remote: Option<String>, branch: String) -> Self {
        Self {
            workdir,
            remote,
            branch,
        }
    }

    fn git(&self, arguments: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&self.workdir)
            .output()
            .map_err(|error| {
                JbsyncError::Git(format!(
                    "could not run git (is it installed and on PATH?): {error}"
                ))
            })?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        Err(JbsyncError::Git(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    fn git_text(&self, arguments: &[&str]) -> Result<String> {
        let raw = self.git(arguments)?;
        Ok(String::from_utf8_lossy(&raw).trim().to_string())
    }

    /// Runs a command whose failure is a legitimate answer rather than an error
    /// (`rev-parse` on an empty repository, `merge-base` on unrelated trees).
    fn git_optional(&self, arguments: &[&str]) -> Option<String> {
        self.git_text(arguments).ok().filter(|out| !out.is_empty())
    }

    fn has_commit(&self) -> bool {
        self.git_optional(&["rev-parse", "--verify", "HEAD"])
            .is_some()
    }

    fn remote_ref(&self) -> String {
        format!("origin/{}", self.branch)
    }

    /// Every file at `reference`, keyed by store-relative path.
    fn tree_at(&self, reference: &str) -> Result<Tree> {
        let listing = self.git_text(&["ls-tree", "-r", "-z", "--name-only", reference])?;
        let mut tree = Tree::new();
        for path in listing.split('\0').filter(|entry| !entry.is_empty()) {
            let blob = self.git(&["show", &format!("{reference}:{path}")])?;
            tree.insert(path.to_string(), blob);
        }
        Ok(tree)
    }

    fn stage_all(&self) -> Result<()> {
        self.git(&["add", "-A"])?;
        Ok(())
    }

    /// True when staged content differs from `HEAD` (or when there is no
    /// `HEAD` yet and something is staged).
    fn has_staged_changes(&self) -> Result<bool> {
        if !self.has_commit() {
            let staged = self.git_text(&["diff", "--cached", "--name-only"])?;
            return Ok(!staged.is_empty());
        }
        Ok(self.git(&["diff", "--cached", "--quiet"]).err().is_some())
    }

    fn commit(&self, message: &str) -> Result<String> {
        self.git(&["commit", "--no-verify", "-m", message])?;
        self.git_text(&["rev-parse", "HEAD"])
    }

    fn push(&self) -> Result<()> {
        if self.remote.is_none() {
            return Ok(());
        }
        self.git(&["push", "-u", "origin", &self.branch])?;
        Ok(())
    }

    /// Git refuses to commit without an identity. This store is jbsync's own
    /// bookkeeping rather than something the user authors, so a placeholder is
    /// set only when the user has configured nothing — never overriding a real
    /// identity, and never touching global config.
    fn ensure_identity(&self) -> Result<()> {
        if self.git_optional(&["config", "user.email"]).is_none() {
            self.git(&["config", "user.email", "jbsync@localhost"])?;
        }
        if self.git_optional(&["config", "user.name"]).is_none() {
            self.git(&["config", "user.name", "jbsync"])?;
        }
        Ok(())
    }

    fn configure_remote(&self) -> Result<()> {
        let Some(remote) = &self.remote else {
            // A remote that was removed from config should stop being used.
            if self
                .git_optional(&["remote", "get-url", "origin"])
                .is_some()
            {
                self.git(&["remote", "remove", "origin"])?;
            }
            return Ok(());
        };
        match self.git_optional(&["remote", "get-url", "origin"]) {
            Some(current) if current == *remote => Ok(()),
            Some(_) => {
                self.git(&["remote", "set-url", "origin", remote])?;
                Ok(())
            }
            None => {
                self.git(&["remote", "add", "origin", remote])?;
                Ok(())
            }
        }
    }
}

impl Backend for GitBackend {
    fn describe(&self) -> String {
        // The store's path is deliberately left out: it belongs to
        // `jbsync repo show`, and repeating it in every report header buried
        // the part that actually changes.
        self.remote.as_ref().map_or_else(
            || "local store, no remote".to_string(),
            |remote| format!("{remote} on {}", self.branch),
        )
    }

    fn workdir(&self) -> &Path {
        &self.workdir
    }

    fn initialize(&self) -> Result<()> {
        std::fs::create_dir_all(&self.workdir)?;
        if !self.workdir.join(".git").exists() {
            self.git(&["init"])?;
            // `init -b` needs Git 2.28; this works everywhere.
            self.git(&[
                "symbolic-ref",
                "HEAD",
                &format!("refs/heads/{}", self.branch),
            ])?;
        }
        let attributes = self.workdir.join(".gitattributes");
        if !attributes.exists() {
            std::fs::write(&attributes, GITATTRIBUTES)?;
        }
        self.ensure_identity()?;
        self.configure_remote()?;

        // Adopt an existing remote history when this machine is starting empty,
        // so a second machine joins the store instead of forking it.
        if self.remote.is_some() && !self.has_commit() {
            let remote_ref = self.remote_ref();
            if self.git(&["fetch", "origin", &self.branch]).is_ok()
                && self
                    .git_optional(&["rev-parse", "--verify", &remote_ref])
                    .is_some()
            {
                self.git(&["reset", "--hard", &remote_ref])?;
            }
        }
        Ok(())
    }

    fn incoming(&self) -> Result<Option<Incoming>> {
        if self.remote.is_none() {
            return Ok(None);
        }
        // A fresh clone or an empty remote is not an error, just nothing to take.
        if self.git(&["fetch", "origin", &self.branch]).is_err() {
            return Ok(None);
        }
        let remote_ref = self.remote_ref();
        let Some(cursor) = self.git_optional(&["rev-parse", "--verify", &remote_ref]) else {
            return Ok(None);
        };
        if !self.has_commit() {
            return Ok(Some(Incoming {
                remote: self.tree_at(&cursor)?,
                base: Tree::new(),
                cursor,
            }));
        }
        // Already contains the remote tip: nothing to merge.
        if self
            .git(&["merge-base", "--is-ancestor", &cursor, "HEAD"])
            .is_ok()
        {
            return Ok(None);
        }
        let base = match self.git_optional(&["merge-base", "HEAD", &cursor]) {
            Some(reference) => self.tree_at(&reference)?,
            None => Tree::new(),
        };
        Ok(Some(Incoming {
            remote: self.tree_at(&cursor)?,
            base,
            cursor,
        }))
    }

    fn publish(&self, message: &str) -> Result<Published> {
        self.stage_all()?;
        if !self.has_staged_changes()? {
            // Still push: a previous run may have committed without reaching
            // the remote.
            self.push()?;
            return Ok(Published::Unchanged);
        }
        let files = self
            .git_text(&["diff", "--cached", "--name-only"])?
            .lines()
            .filter(|line| !line.is_empty())
            .count();
        let cursor = self.commit(message)?;
        self.push()?;
        Ok(Published::Committed { cursor, files })
    }

    fn reconcile(&self, cursor: &str, message: &str) -> Result<()> {
        // The working copy already holds the merged result, so commit it before
        // recording the remote as a parent.
        self.stage_all()?;
        if self.has_staged_changes()? {
            self.commit(message)?;
        }
        if !self.has_commit() {
            self.git(&["reset", "--hard", cursor])?;
            return Ok(());
        }
        if self
            .git(&["merge-base", "--is-ancestor", cursor, "HEAD"])
            .is_ok()
        {
            return Ok(());
        }
        // `-s ours` keeps the tree we just merged by hand while still recording
        // the remote commit as a parent, so the next push fast-forwards.
        let merge_message = format!("{message} (merge)");
        self.git(&[
            "merge",
            "-s",
            "ours",
            "--allow-unrelated-histories",
            "--no-verify",
            "-m",
            &merge_message,
            cursor,
        ])?;
        Ok(())
    }
}

/// Store paths that are jbsync's own bookkeeping rather than settings.
pub fn is_internal_path(relative: &str) -> bool {
    relative == ".gitattributes" || relative.starts_with(".git/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(root: &Path, remote: Option<&str>) -> GitBackend {
        GitBackend::new(
            root.to_path_buf(),
            remote.map(str::to_string),
            "main".to_string(),
        )
    }

    /// Git refuses to commit without an identity, and CI machines have none.
    fn set_identity(backend: &GitBackend) {
        backend
            .git(&["config", "user.email", "jbsync@test"])
            .unwrap();
        backend.git(&["config", "user.name", "jbsync"]).unwrap();
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn initialize_is_idempotent_and_pins_line_endings() {
        let directory = tempfile::tempdir().unwrap();
        let git = backend(directory.path(), None);
        git.initialize().unwrap();
        git.initialize().unwrap();
        assert!(directory.path().join(".git").is_dir());
        assert_eq!(
            std::fs::read_to_string(directory.path().join(".gitattributes")).unwrap(),
            GITATTRIBUTES
        );
    }

    #[test]
    fn publish_commits_once_then_reports_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let git = backend(directory.path(), None);
        git.initialize().unwrap();
        set_identity(&git);

        write(
            directory.path(),
            "shared/options/laf.xml",
            "<application />",
        );
        match git.publish("first").unwrap() {
            Published::Committed { files, .. } => assert!(files >= 1),
            Published::Unchanged => panic!("expected a commit"),
        }
        assert_eq!(git.publish("second").unwrap(), Published::Unchanged);
    }

    #[test]
    fn no_remote_means_nothing_incoming() {
        let directory = tempfile::tempdir().unwrap();
        let git = backend(directory.path(), None);
        git.initialize().unwrap();
        set_identity(&git);
        write(directory.path(), "a.xml", "<a />");
        git.publish("first").unwrap();
        assert!(git.incoming().unwrap().is_none());
    }

    #[test]
    fn incoming_reports_remote_state_and_common_ancestor() {
        let origin = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(origin.path())
            .output()
            .unwrap();
        let remote = origin.path().to_string_lossy().into_owned();

        // First machine publishes a baseline.
        let one = tempfile::tempdir().unwrap();
        let first = backend(one.path(), Some(&remote));
        first.initialize().unwrap();
        set_identity(&first);
        write(one.path(), "shared/laf.xml", "<application v=\"1\" />");
        first.publish("baseline").unwrap();

        // Second machine adopts it, then both diverge.
        let two = tempfile::tempdir().unwrap();
        let second = backend(two.path(), Some(&remote));
        second.initialize().unwrap();
        set_identity(&second);
        assert!(
            two.path().join("shared/laf.xml").exists(),
            "adopted history"
        );

        write(one.path(), "shared/laf.xml", "<application v=\"2\" />");
        first.publish("from one").unwrap();
        write(two.path(), "shared/laf.xml", "<application v=\"3\" />");
        second.publish("from two").unwrap_err();

        let incoming = second.incoming().unwrap().expect("remote moved ahead");
        assert_eq!(
            incoming.remote.get("shared/laf.xml").map(Vec::as_slice),
            Some(b"<application v=\"2\" />".as_slice())
        );
        assert_eq!(
            incoming.base.get("shared/laf.xml").map(Vec::as_slice),
            Some(b"<application v=\"1\" />".as_slice()),
            "base is the last state both machines agreed on"
        );
    }

    #[test]
    fn reconcile_keeps_the_merged_tree_and_lets_the_push_succeed() {
        let origin = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(origin.path())
            .output()
            .unwrap();
        let remote = origin.path().to_string_lossy().into_owned();

        let one = tempfile::tempdir().unwrap();
        let first = backend(one.path(), Some(&remote));
        first.initialize().unwrap();
        set_identity(&first);
        write(one.path(), "shared/laf.xml", "<application v=\"1\" />");
        first.publish("baseline").unwrap();

        let two = tempfile::tempdir().unwrap();
        let second = backend(two.path(), Some(&remote));
        second.initialize().unwrap();
        set_identity(&second);

        write(one.path(), "shared/laf.xml", "<application v=\"2\" />");
        first.publish("from one").unwrap();

        // Second machine merges by hand, then records the reconciliation.
        write(two.path(), "shared/laf.xml", "<application v=\"merged\" />");
        let incoming = second.incoming().unwrap().unwrap();
        second.reconcile(&incoming.cursor, "merge").unwrap();
        second.publish("publish merged").unwrap();

        assert_eq!(
            std::fs::read_to_string(two.path().join("shared/laf.xml")).unwrap(),
            "<application v=\"merged\" />"
        );
        assert!(
            second.incoming().unwrap().is_none(),
            "reconciled and pushed, so nothing is outstanding"
        );
    }
}
