use glade_binding_api::{BindError, BindRequest, BindingResolver, BoundBinding, conformance};
use std::future::Future;
use std::task::{Context, Poll, Waker};
struct Resolver {
    wrong_scope: bool,
}
impl BindingResolver for Resolver {
    async fn resolve(&self, request: BindRequest) -> Result<BoundBinding, BindError> {
        if request.principal != "alice" {
            return Err(BindError::Denied);
        }
        if request.declaration.shape != glade_binding_api::Shape::Value {
            return Err(BindError::Unsupported);
        }
        if request.declaration.glade_id.id != "notes" {
            return Err(BindError::UnknownDeclaration);
        }
        if request.definition_version != "fixture-v1" {
            return Err(BindError::VersionMismatch);
        }
        if request.domain_instance != "workspace-a"
            || request.declaration != conformance::request().declaration
        {
            return Err(BindError::Denied);
        }
        if request.parameters != vec![1] {
            return Err(BindError::InvalidParameters);
        }
        let mut bound = conformance::expected(request);
        if self.wrong_scope {
            bound.share = "other".into();
        }
        Ok(bound)
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
fn bi_001_exact_binding_without_provider() {
    run(conformance::binding(&Resolver { wrong_scope: false }));
}
#[test]
fn bi_002_fail_closed() {
    run(conformance::rejections(&Resolver { wrong_scope: false }));
}
#[test]
#[should_panic]
fn rejects_wrong_scope() {
    run(conformance::binding(&Resolver { wrong_scope: true }));
}

#[test]
fn bi_003_scope_and_descriptor_cannot_escalate() {
    run(conformance::scope_rejections(&Resolver {
        wrong_scope: false,
    }));
}
