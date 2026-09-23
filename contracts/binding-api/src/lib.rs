//! Draft substrate binding resolution; not Glial mount orchestration or provider routing.
pub use glade_decl::{BindingDecl, Shape};
use std::future::Future;

#[derive(Clone, Debug, PartialEq)]
pub struct BindRequest {
    pub principal: String,
    pub declaration: BindingDecl,
    /// Exact declaration/profile version; no implicit latest-version negotiation.
    pub definition_version: String,
    pub domain_instance: String,
    /// Canonical parameters under the declaration's separately versioned key schema.
    pub parameters: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BoundBinding {
    pub request: BindRequest,
    pub share: String,
    pub key: Vec<u8>,
    /// Exact supported adapter capability, not merely a recognized shape name.
    pub capability: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindError {
    Denied,
    UnknownDeclaration,
    VersionMismatch,
    Unsupported,
    InvalidParameters,
    Unavailable,
}

/// Resolve an already registered declaration for a trusted authenticated caller.
/// The input principal MUST be established by ingress, not trusted from wire DTOs.
/// Implementations MUST check exact declaration/version, domain/zone mapping,
/// complete source authorization and supported adapter before returning a binding.
/// Unknown/unsupported shapes MUST NOT fall back to value; resolution MUST NOT
/// register declarations, attach providers, instantiate services or mutate grants.
/// Runtime authority comes from authenticated folds, never manifest ACL seeds.
///
/// Results MUST preserve the entire request and bind the exact share/key/capability.
/// A result is descriptive data, NOT a bearer grant: callers MUST reauthorize every
/// operation. It promises neither provider existence nor connectivity/freshness.
/// Domain/zone/key schema migration remains versioned; no new mapping is ratified here.
/// Futures MUST be lazy and all outcomes bounded by host-configured resource limits.
///
/// ```compile_fail
/// use glade_binding_api::BindingResolver;
/// struct Missing;
/// impl BindingResolver for Missing {}
/// ```
pub trait BindingResolver: Send + Sync {
    fn resolve(
        &self,
        request: BindRequest,
    ) -> impl Future<Output = Result<BoundBinding, BindError>> + Send;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Deterministic probes for an exclusively configured fixture declaration.
    use crate::*;
    pub fn request() -> BindRequest {
        let mut declaration = BindingDecl::default();
        declaration.glade_id.id = "notes".into();
        BindRequest {
            principal: "alice".into(),
            declaration,
            definition_version: "fixture-v1".into(),
            domain_instance: "workspace-a".into(),
            parameters: vec![1],
        }
    }
    pub fn expected(request: BindRequest) -> BoundBinding {
        BoundBinding {
            share: request.domain_instance.clone(),
            key: request.parameters.clone(),
            capability: "fixture.value/v1".into(),
            request,
        }
    }
    /// BI-001. Fixture maps workspace-a/[1] to the same share/key with value/v1.
    pub async fn binding<R: BindingResolver>(resolver: &R) {
        let request = request();
        let expected = BoundBinding {
            request: request.clone(),
            share: "workspace-a".into(),
            key: vec![1],
            capability: "fixture.value/v1".into(),
        };
        assert_resolution(resolver, request, Ok(expected)).await;
    }
    /// Adapter-reusable oracle; expected mapping MUST be independently specified.
    pub async fn assert_resolution<R: BindingResolver>(
        resolver: &R,
        request: BindRequest,
        expected: Result<BoundBinding, BindError>,
    ) {
        if let Ok(bound) = &expected {
            assert_eq!(bound.request, request);
        }
        assert_eq!(resolver.resolve(request).await, expected);
    }
    /// BI-002. Fixture permits only alice, notes, fixture-v1, and value.
    pub async fn rejections<R: BindingResolver>(resolver: &R) {
        let mut r = request();
        r.principal = "mallory".into();
        assert_eq!(resolver.resolve(r).await, Err(BindError::Denied));
        let mut r = request();
        r.definition_version = "other".into();
        assert_eq!(resolver.resolve(r).await, Err(BindError::VersionMismatch));
        for shape in [Shape::Message, Shape::Window, Shape::Stream, Shape::Crdt] {
            let mut r = request();
            r.declaration.shape = shape;
            assert_eq!(resolver.resolve(r).await, Err(BindError::Unsupported));
        }
        let mut r = request();
        r.declaration.glade_id.id = "missing".into();
        assert_eq!(
            resolver.resolve(r).await,
            Err(BindError::UnknownDeclaration)
        );
    }

    /// BI-003. Descriptor must equal the registered declaration; scope is not caller authority.
    pub async fn scope_rejections<R: BindingResolver>(resolver: &R) {
        let mut r = request();
        r.domain_instance = "other".into();
        assert_eq!(resolver.resolve(r).await, Err(BindError::Denied));
        let mut r = request();
        r.parameters = vec![];
        assert_eq!(resolver.resolve(r).await, Err(BindError::InvalidParameters));
        let mut r = request();
        r.declaration.source = Some("ungranted-source".into());
        assert_eq!(resolver.resolve(r).await, Err(BindError::Denied));
    }
}
