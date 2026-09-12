use glade_sync_api::{IngestReceipt, Page, ReadRequest, ReplicaSync, SyncError, conformance};
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};
// Synthetic byte records [sequence, payload]; NOT the Glade wire encoding.
struct Model {
    records: Mutex<Vec<Vec<u8>>>,
    partial: bool,
}
impl ReplicaSync for Model {
    type Cursor = u64;
    async fn read(&self, request: ReadRequest<u64>) -> Result<Page<u64>, SyncError> {
        let rows = self.records.lock().unwrap();
        let start = request.after.unwrap_or(0) as usize;
        if start > rows.len() {
            return Err(SyncError::InvalidCursor);
        }
        let mut used = 0;
        let mut operations = Vec::new();
        for row in &rows[start..] {
            if operations.len() == request.max_operations.get()
                || used + row.len() > request.max_bytes.get()
            {
                break;
            }
            used += row.len();
            operations.push(row.clone());
        }
        if operations.is_empty() && start < rows.len() {
            return Err(SyncError::Capacity);
        }
        let cursor = (start + operations.len()) as u64;
        Ok(Page {
            operations,
            cursor,
            more: cursor < (rows.len() as u64),
        })
    }
    async fn ingest(&self, operations: Vec<Vec<u8>>) -> Result<IngestReceipt<u64>, SyncError> {
        let mut rows = self.records.lock().unwrap();
        let mut staged = rows.clone();
        for row in operations {
            if row.len() != 2 || row[0] == 0 {
                if self.partial {
                    *rows = staged;
                }
                return Err(SyncError::InvalidOperation);
            }
            let index = usize::from(row[0] - 1);
            match index.cmp(&staged.len()) {
                std::cmp::Ordering::Less => {
                    if staged[index] != row {
                        return Err(SyncError::Conflict);
                    }
                }
                std::cmp::Ordering::Equal => staged.push(row),
                std::cmp::Ordering::Greater => return Err(SyncError::InvalidOperation),
            }
        }
        *rows = staged;
        Ok(IngestReceipt {
            cursor: rows.len() as u64,
        })
    }
}
fn model(partial: bool) -> Model {
    Model {
        records: Mutex::new(Vec::new()),
        partial,
    }
}
fn run(f: impl Future<Output = ()> + Send) {
    let mut f = std::pin::pin!(f);
    assert!(matches!(
        f.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    ));
}
#[test]
fn sy_001_ingest_dedup_and_bounded_read() {
    run(conformance::roundtrip(&model(false)));
}
#[test]
fn sy_002_invalid_batch_is_atomic() {
    run(conformance::invalid(&model(false)));
}
#[test]
fn sy_003_wrong_cursor_and_small_budget() {
    run(conformance::bounds(&model(false)));
}
#[test]
#[should_panic]
fn rejects_partial_batch() {
    run(conformance::invalid(&model(true)));
}

// A distinct cursor type proves adapter assertions do not depend on fixture u64.
struct TextCursor(Model);
impl ReplicaSync for TextCursor {
    type Cursor = String;
    async fn read(&self, r: ReadRequest<String>) -> Result<Page<String>, SyncError> {
        let after = r
            .after
            .map(|s| s.parse::<u64>().map_err(|_| SyncError::InvalidCursor))
            .transpose()?;
        let page = self
            .0
            .read(ReadRequest {
                after,
                max_operations: r.max_operations,
                max_bytes: r.max_bytes,
            })
            .await?;
        Ok(Page {
            operations: page.operations,
            cursor: page.cursor.to_string(),
            more: page.more,
        })
    }
    async fn ingest(&self, ops: Vec<Vec<u8>>) -> Result<IngestReceipt<String>, SyncError> {
        self.0.ingest(ops).await.map(|r| IngestReceipt {
            cursor: r.cursor.to_string(),
        })
    }
}
#[test]
fn sy_004_adapter_corpus_and_cursor_are_parameters() {
    run(async {
        let replica = TextCursor(model(false));
        let request = ReadRequest {
            after: None,
            max_operations: std::num::NonZeroUsize::new(8).unwrap(),
            max_bytes: std::num::NonZeroUsize::new(16).unwrap(),
        };
        let operations = vec![vec![1, 70], vec![2, 80]];
        conformance::assert_ingest_and_replay(
            &replica,
            operations.clone(),
            IngestReceipt { cursor: "2".into() },
            request.clone(),
            Page {
                operations,
                cursor: "2".into(),
                more: false,
            },
        )
        .await;
        conformance::assert_rejection(
            &replica,
            vec![vec![3, 90], vec![]],
            SyncError::InvalidOperation,
            request,
        )
        .await;
    });
}
#[test]
#[should_panic]
fn generic_assertions_reject_partial_ingest() {
    run(async {
        let replica = TextCursor(model(true));
        let request = ReadRequest {
            after: None,
            max_operations: std::num::NonZeroUsize::new(8).unwrap(),
            max_bytes: std::num::NonZeroUsize::new(16).unwrap(),
        };
        conformance::assert_rejection(
            &replica,
            vec![vec![1, 70], vec![]],
            SyncError::InvalidOperation,
            request,
        )
        .await;
    });
}
