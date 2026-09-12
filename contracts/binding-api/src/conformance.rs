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
