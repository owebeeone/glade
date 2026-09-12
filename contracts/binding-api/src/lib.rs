//! Draft substrate binding resolution; not Glial mount orchestration or provider routing.
pub use glade_decl::{BindingDecl, Shape};
use std::future::Future;
#[cfg(feature = "conformance")]
pub mod conformance;

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
