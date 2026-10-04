//! The child processes of this test executable, spawned one at a time and
//! never while a test reopens a root lock it released.
//!
//! On macOS `Command::spawn` is `posix_spawn` for every child these tests
//! start (an absolute program, no `pre_exec`, uid, gid, groups, chroot, cwd
//! or setsid), and the new process holds a copy of every descriptor of this
//! process, close-on-exec ones included, until its exec completes, which is
//! before `spawn` returns. A `flock` root lock (`independent::disk`) that one
//! test drops while another test's child is mid-spawn so stays held by that
//! copy, and an immediate reopen of the root is refused `Ownership`, by this
//! process or by a child that reopens it. Production never spawns a child, so
//! this is the test executable's own hazard, held here and nowhere else:
//!
//! - every child is spawned through [`spawn`], which holds [`SPAWNING`]
//!   exclusively, so no other spawn is in flight and none starts meanwhile:
//!   a child started after its test released a root never meets a copy of
//!   that root's lock;
//! - a test that releases a root and reopens it in this process calls
//!   [`settled`] in between, which waits out a spawn in flight: no copy of a
//!   descriptor closed before the call survives it.
//!
//! Neither retries anything: a lock some other holder really keeps is still
//! refused exactly as before.
use std::io;
use std::process::{Child, Command};
use std::sync::{PoisonError, RwLock};

/// Held for writing across each child's spawn, for reading by [`settled`].
static SPAWNING: RwLock<()> = RwLock::new(());

/// Spawn `command`'s child with no other child of this process mid-spawn.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    let _alone = SPAWNING.write().unwrap_or_else(PoisonError::into_inner);
    command.spawn()
}

/// Return once no child of this process is mid-spawn: every descriptor this
/// process closed before the call is then closed everywhere.
pub(crate) fn settled() {
    drop(SPAWNING.read().unwrap_or_else(PoisonError::into_inner));
}
