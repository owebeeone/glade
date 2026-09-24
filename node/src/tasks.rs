//! The task-owner seam (plan Step 3.3). Every task the node spawns goes
//! through [`Tasks::spawn`], named by its [`Site`], which says where it goes.
//!
//! - **Unowned**, which is what `Server::open` makes and all the hand-written
//!   root ever has: the task is `tokio::spawn`ed, detached, exactly as before.
//!   The same task starts at the same place, and a writer is still aborted
//!   through the handle its caller keeps.
//! - **Owned**, which the assembled root sets (`src/lifecycle.rs`): the task is
//!   sent to the owner of its site's role, one of the plan's two services
//!   (`Sessions`, `Records`). The owner spawns it into its `JoinSet`, reaps it
//!   when it ends and, at its stop, closes its inbox, cancels every task at
//!   its next await point and joins it ([`finish`]). A task sent after that is
//!   refused and dropped: admission has closed.
//!
//! The owner is what sdax sees: its stop is bounded by the plan, and an owner
//! that cannot finish is named in `report.incomplete`. Not one sdax instance
//! per task: a run keeps history for every instance it ever spawned, and the
//! node spawns per stream and per push (`dev-docs/GladeNodeAssembly.md`,
//! "Lifecycle (plan Step 3.3)").

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::OnceLock;

use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};

/// A task, boxed so one owner can hold tasks of every site.
pub(crate) type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// An owner's inbox: the tasks sent to it, in order.
pub(crate) type Inbox = mpsc::UnboundedReceiver<Task>;

/// Every place the node spawns a task: the nine of `mesh.rs` and the renewal
/// loop the plan names, and the four it does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Site {
    /// `mesh.rs`, `enable_mesh`: the peer accept loop.
    AcceptLoop,
    /// `mesh.rs`, the accept loop: one accepted link's driver.
    AcceptedLink,
    /// `mesh.rs`, `run_link`: unlink the link once it closes.
    Unlink,
    /// `mesh.rs`, `run_link`: dispatch a link's inbound streams.
    StreamDispatch,
    /// `mesh.rs`, the dispatcher: one inbound peer stream.
    PeerStream,
    /// `mesh.rs`, `run_link`: the acceptor serves the dialer's stream 0.
    StreamZero,
    /// `mesh.rs`, `push_home`: one push of minted records to one link.
    RecordPush,
    /// `mesh.rs`, `serve_peer_subscribe`: a served subscription's writer.
    SubscriptionWriter,
    /// `mesh.rs`, `forward_interest`: an interest forwarded to a claim holder.
    ForwardInterest,
    /// `claims.rs`, `adopt_boot_tuned`: the lease-renewal loop.
    Renewal,
    /// `exchange.rs`, `handle_request`: an exchange forwarded to a claim holder.
    ForwardExchange,
    /// `exchange.rs`, `handle_create`: a `workspace.create` forwarded to its target.
    ForwardCreate,
    /// `server.rs`, the accept loop: one client session.
    ClientSession,
    /// `server.rs`, `handle`: a client session's writer.
    ClientWriter,
}

impl Site {
    /// Whose task it is: the renewal loop and the pushes of what it and every
    /// other mint write are `Records`'; the rest ride a link or a client
    /// connection and are `Sessions'`.
    fn role(self) -> Role {
        match self {
            Site::Renewal | Site::RecordPush => Role::Records,
            _ => Role::Sessions,
        }
    }
}

#[derive(Clone, Copy)]
enum Role {
    Sessions,
    Records,
}

/// The owners' inboxes, as the seam sends to them.
pub(crate) struct Owners {
    sessions: mpsc::UnboundedSender<Task>,
    records: mpsc::UnboundedSender<Task>,
}

/// A pair of owners: the seam's half, then the `Sessions` and `Records`
/// inboxes, which the two services take.
pub(crate) fn owners() -> (Owners, Inbox, Inbox) {
    let (sessions, sessions_inbox) = mpsc::unbounded_channel();
    let (records, records_inbox) = mpsc::unbounded_channel();
    (Owners { sessions, records }, sessions_inbox, records_inbox)
}

/// Where a node's spawned tasks go. See the module docs.
pub(crate) struct Tasks {
    owners: OnceLock<Owners>,
}

impl Tasks {
    /// Unowned: every task is spawned detached, as the hand-written root does.
    pub(crate) fn unowned() -> Tasks {
        Tasks {
            owners: OnceLock::new(),
        }
    }

    /// Hand every later task to `owners`. Once, before anything is spawned.
    pub(crate) fn own(&self, owners: Owners) -> io::Result<()> {
        self.owners
            .set(owners)
            .map_err(|_| io::Error::other("the node's tasks already have owners"))
    }

    /// Spawn `task` for `site`: detached when unowned, else sent to its owner.
    pub(crate) fn spawn<F>(&self, site: Site, task: F) -> TaskHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let Some(owners) = self.owners.get() else {
            return TaskHandle(Handle::Detached(tokio::spawn(task)));
        };
        let inbox = match site.role() {
            Role::Sessions => &owners.sessions,
            Role::Records => &owners.records,
        };
        // An abort through the handle ends the task as its owner's stop would.
        // A handle dropped unused closes the channel without a value, which
        // the `Ok` pattern ignores: it neither aborts nor detaches anything.
        let (abort, aborted) = oneshot::channel::<()>();
        let task: Task = Box::pin(async move {
            tokio::select! {
                () = task => {}
                Ok(()) = aborted => {}
            }
        });
        match inbox.send(task) {
            Ok(()) => TaskHandle(Handle::Owned(abort)),
            Err(_) => TaskHandle(Handle::Refused),
        }
    }
}

/// What a spawn hands back. Dropping it changes nothing; [`TaskHandle::abort`]
/// ends the task.
pub(crate) struct TaskHandle(Handle);

enum Handle {
    Detached(JoinHandle<()>),
    Owned(oneshot::Sender<()>),
    Refused,
}

impl TaskHandle {
    /// End the task at its next await point, as `JoinHandle::abort` does.
    pub(crate) fn abort(self) {
        match self.0 {
            Handle::Detached(task) => task.abort(),
            Handle::Owned(abort) => {
                let _ = abort.send(());
            }
            Handle::Refused => {}
        }
    }
}

/// Run one owner: spawn every task its inbox receives and reap every task
/// that ends. Never returns; the owner's service ends it at its stop.
pub(crate) async fn own(inbox: &mut Inbox, tasks: &mut JoinSet<()>) {
    // False once every sender is gone with the node state; reaping goes on.
    let mut open = true;
    loop {
        tokio::select! {
            task = inbox.recv(), if open => match task {
                Some(task) => {
                    tasks.spawn(task);
                }
                None => open = false,
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            else => std::future::pending::<()>().await,
        }
    }
}

/// An owner's stop: admission closes (a later send is refused and its task
/// dropped), a task queued and never started is dropped, and every running
/// task is cancelled at its next await point and joined. When this returns,
/// nothing the owner held is running. Returns how many tasks were cancelled.
pub(crate) async fn finish(mut inbox: Inbox, mut tasks: JoinSet<()>) -> usize {
    inbox.close();
    while inbox.try_recv().is_ok() {}
    let cancelled = tasks.len();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    cancelled
}
