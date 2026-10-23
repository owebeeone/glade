//! Configured-peer retry timing and task ownership with a paused clock.
use super::support::{endpoint_of, fresh, A_KEY, A_SEED, B_KEY};
use crate::frame::MAX_FRAME_BYTES;
use crate::mesh::PeerPort;
use crate::netconf::{PeerEntry, Via};
use crate::peer::NodeIdentity;
use crate::server::Server;
use crate::tasks;
use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;

/// A bound transport with an unavailable network. Accept waits until terminal
/// close; dials fail, or stall until close, without pretending HELLO succeeded.
struct Unavailable {
    attempts: AtomicUsize,
    stall: bool,
    closed: watch::Sender<bool>,
}

impl CarrierPort for Unavailable {
    fn bind(&self, _: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        Box::pin(async {
            Err(if *self.closed.borrow() {
                CarrierError::Closed
            } else {
                CarrierError::AlreadyBound
            })
        })
    }
    fn dial<'a>(
        &'a self,
        _: &'a CarrierAddr,
    ) -> PortFuture<'a, Result<Box<dyn CarrierLink>, CarrierError>> {
        Box::pin(async move {
            if *self.closed.borrow() {
                return Err(CarrierError::Closed);
            }
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.stall {
                let mut closed = self.closed.subscribe();
                closed.wait_for(|closed| *closed).await.unwrap();
                Err(CarrierError::Closed)
            } else {
                Err(CarrierError::Transport("network unavailable".into()))
            }
        })
    }
    fn accept(&self) -> PortFuture<'_, Result<Option<Box<dyn CarrierLink>>, CarrierError>> {
        Box::pin(async move {
            let mut closed = self.closed.subscribe();
            closed.wait_for(|closed| *closed).await.unwrap();
            Ok(None)
        })
    }
    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async {
            self.closed.send_replace(true);
        })
    }
}

async fn fixture(name: &str, stall: bool) -> (Server, Arc<Unavailable>, tasks::Inbox, PeerEntry) {
    let server = Server::open(fresh(name)).unwrap();
    let (owners, sessions, _records) = tasks::owners();
    server.shared.tasks.own(owners).unwrap();
    let port = Arc::new(Unavailable {
        attempts: AtomicUsize::new(0),
        stall,
        closed: watch::channel(false).0,
    });
    server
        .enable_mesh(PeerPort {
            port: port.clone(),
            notes: None,
            identity: NodeIdentity::from_key(A_SEED),
            endpoint: endpoint_of(A_KEY),
            door: None,
            max_frame_bytes: MAX_FRAME_BYTES,
        })
        .await
        .unwrap();
    let entry = PeerEntry {
        key: endpoint_of(B_KEY),
        via: vec![Via::Ip("127.0.0.1:9".parse().unwrap())],
    };
    (server, port, sessions, entry)
}

async fn start_tasks(inbox: &mut tasks::Inbox) -> JoinSet<()> {
    let mut running = JoinSet::new();
    while let Ok(task) = inbox.try_recv() {
        running.spawn(task);
    }
    tokio::task::yield_now().await;
    running
}

#[tokio::test(start_paused = true)]
async fn configured_peer_retries_initial_failure_with_capped_backoff_and_no_duplicates() {
    let (server, port, mut inbox, entry) = fixture("reconnect-backoff", false).await;
    let admit_only = PeerEntry {
        via: vec![],
        ..entry.clone()
    };
    assert_eq!(
        server.maintain_peer(admit_only).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(port.attempts.load(Ordering::SeqCst), 0);
    assert!(server.maintain_peer(entry.clone()).await.is_err());
    assert_eq!(
        server.maintain_peer(entry).await.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    let running = start_tasks(&mut inbox).await;
    for (index, seconds) in [1, 2, 4, 8, 16, 30, 30].into_iter().enumerate() {
        tokio::time::advance(Duration::from_secs(seconds) - Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(port.attempts.load(Ordering::SeqCst), index + 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(port.attempts.load(Ordering::SeqCst), index + 2);
    }
    tasks::finish(inbox, running).await;
    let stopped = port.attempts.load(Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(300)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.attempts.load(Ordering::SeqCst), stopped);
    port.close().await;
}

#[tokio::test(start_paused = true)]
async fn configured_peer_bounds_stalled_attempts_and_shutdown_cancels_pending_dial() {
    let (server, port, mut inbox, entry) = fixture("reconnect-stalled", true).await;
    let began = tokio::time::Instant::now();
    assert_eq!(
        server.maintain_peer(entry).await.unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    assert_eq!(began.elapsed(), Duration::from_secs(30));
    let running = start_tasks(&mut inbox).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.attempts.load(Ordering::SeqCst), 2);
    tokio::time::advance(Duration::from_secs(29)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        port.attempts.load(Ordering::SeqCst),
        2,
        "no parallel dial while stalled"
    );
    tasks::finish(inbox, running).await;
    tokio::time::advance(Duration::from_secs(300)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.attempts.load(Ordering::SeqCst), 2);
    port.close().await;
}
