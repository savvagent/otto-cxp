//! Keeping the migration owner's credential out of the serving process (#27).
//!
//! Fly secrets are app-wide: `CXP_MIGRATION_DATABASE_URL`, which only the
//! release command (`cxp-server migrate`) needs, is also in the environment of
//! every serving machine. The serving process must not hold it: code
//! execution in it, or anything that dumps its environment, would otherwise
//! hand over the owner role, which is what the split exists to keep from it.
//!
//! Removing the variable with `unsetenv` is not enough on Linux: the original
//! environment block stays in the process's memory and in
//! `/proc/<pid>/environ`. So a serving process that finds the variable
//! **re-executes itself** with the same arguments and the environment minus
//! that variable ([`reexec_without_migration_secret`]), before it reads any
//! configuration, starts a thread, or builds the async runtime. The new image
//! never had it. A value that a local `.env` supplies afterwards is removed
//! with `remove_var` ([`forget_migration_secret`]), still before any thread
//! exists; on a developer's machine that is enough.
//!
//! What this does not cover: whoever can read the machine's other processes or
//! its init's environment (root on the machine, `fly ssh console`) still sees
//! the secret. docs/deploy/fly.md says so, and names the stronger arrangement
//! (migrations from a separate app) as the follow-up.

use std::ffi::{OsStr, OsString};

/// The release command's owner credential.
pub const MIGRATION_SECRET: &str = "CXP_MIGRATION_DATABASE_URL";

/// Set in the re-executed process's environment, so its boot can log that the
/// secret was removed (the new process cannot otherwise tell).
pub const SCRUBBED_MARKER: &str = "CXP_MIGRATION_SECRET_REMOVED";

/// The environment to re-execute with, or `None` when `vars` holds no
/// [`MIGRATION_SECRET`] and there is nothing to remove. Order is kept; the
/// marker is added.
pub fn without_migration_secret<I>(vars: I) -> Option<Vec<(OsString, OsString)>>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let vars: Vec<(OsString, OsString)> = vars.into_iter().collect();
    if !vars.iter().any(|(k, _)| k == OsStr::new(MIGRATION_SECRET)) {
        return None;
    }
    let mut kept: Vec<(OsString, OsString)> = vars
        .into_iter()
        .filter(|(k, _)| k != OsStr::new(MIGRATION_SECRET) && k != OsStr::new(SCRUBBED_MARKER))
        .collect();
    kept.push((SCRUBBED_MARKER.into(), "1".into()));
    Some(kept)
}

/// For a serving process: when the environment holds [`MIGRATION_SECRET`],
/// replace this process with a fresh copy of the same executable and
/// arguments whose environment lacks it. Returns `Ok(())` only when there was
/// nothing to remove; on success it does not return at all. Must run before
/// any thread starts (it is the first thing `main` does when serving).
#[cfg(unix)]
pub fn reexec_without_migration_secret() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::unix::process::CommandExt;

    let Some(env) = without_migration_secret(std::env::vars_os()) else {
        return Ok(());
    };
    let exe = std::env::current_exe().context("could not find this executable to re-exec")?;
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env_clear()
        .envs(env)
        .exec();
    // exec returns only on failure. Refuse to serve rather than carry on with
    // the owner's credential in memory.
    Err(anyhow::Error::new(err).context(format!(
        "could not re-exec without {MIGRATION_SECRET}; refusing to serve with the migration \
         owner's credential in this process"
    )))
}

#[cfg(not(unix))]
pub fn reexec_without_migration_secret() -> anyhow::Result<()> {
    forget_migration_secret();
    Ok(())
}

/// Remove [`MIGRATION_SECRET`] from this process's environment (a value a
/// local `.env` loaded after the re-exec check). Call only while the process
/// has one thread.
pub fn forget_migration_secret() -> bool {
    let present = std::env::var_os(MIGRATION_SECRET).is_some();
    if present {
        std::env::remove_var(MIGRATION_SECRET);
    }
    present
}

/// Whether this process is the re-executed copy (see [`SCRUBBED_MARKER`]).
pub fn was_reexecuted() -> bool {
    std::env::var_os(SCRUBBED_MARKER).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    #[test]
    fn nothing_to_remove_means_no_reexec() {
        assert_eq!(
            without_migration_secret(vars(&[("DATABASE_URL", "postgres://srv@db/x")])),
            None
        );
    }

    #[test]
    fn the_secret_is_dropped_and_everything_else_kept_in_order() {
        let env = without_migration_secret(vars(&[
            ("A", "1"),
            (MIGRATION_SECRET, "postgres://owner:pw@db/x"),
            ("DATABASE_URL", "postgres://srv@db/x"),
            // A stale marker cannot be smuggled in twice.
            (SCRUBBED_MARKER, "0"),
        ]))
        .expect("a re-exec");
        assert_eq!(
            env,
            vars(&[
                ("A", "1"),
                ("DATABASE_URL", "postgres://srv@db/x"),
                (SCRUBBED_MARKER, "1")
            ])
        );
        // The re-executed environment has nothing left to remove: no loop.
        assert_eq!(without_migration_secret(env), None);
    }
}
