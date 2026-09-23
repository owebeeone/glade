use glade_binding_api::{
    BindError, BindRequest, BindingResolver, BoundBinding, Shape, conformance,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Deliberately wrong behaviours, each caught by one probe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrong {
    /// Binds the right request to another share.
    Scope,
    /// Folds `atom` into `value`: an `Atom` request resolves as the value
    /// fixture (COD-P3-5).
    AtomAsValue,
}

struct Resolver {
    wrong: Option<Wrong>,
}
impl BindingResolver for Resolver {
    async fn resolve(&self, request: BindRequest) -> Result<BoundBinding, BindError> {
        let mut declaration = request.declaration.clone();
        if self.wrong == Some(Wrong::AtomAsValue) && declaration.shape == Shape::Atom {
            declaration.shape = Shape::Value;
        }
        if request.principal != "alice" {
            return Err(BindError::Denied);
        }
        if declaration.shape != Shape::Value {
            return Err(BindError::Unsupported);
        }
        if declaration.glade_id.id != "notes" {
            return Err(BindError::UnknownDeclaration);
        }
        if request.definition_version != "fixture-v1" {
            return Err(BindError::VersionMismatch);
        }
        if request.domain_instance != "workspace-a"
            || declaration != conformance::request().declaration
        {
            return Err(BindError::Denied);
        }
        if request.parameters != vec![1] {
            return Err(BindError::InvalidParameters);
        }
        let mut bound = conformance::expected(request);
        if self.wrong == Some(Wrong::Scope) {
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
    run(conformance::binding(&Resolver { wrong: None }));
}
#[test]
fn bi_002_fail_closed() {
    run(conformance::rejections(&Resolver { wrong: None }));
}
#[test]
#[should_panic]
fn rejects_wrong_scope() {
    run(conformance::binding(&Resolver {
        wrong: Some(Wrong::Scope),
    }));
}

#[test]
#[should_panic(expected = "BI-002 Atom must not fall back to value")]
fn rejects_atom_resolved_as_value() {
    run(conformance::rejections(&Resolver {
        wrong: Some(Wrong::AtomAsValue),
    }));
}

#[test]
fn bi_003_scope_and_descriptor_cannot_escalate() {
    run(conformance::scope_rejections(&Resolver { wrong: None }));
}
