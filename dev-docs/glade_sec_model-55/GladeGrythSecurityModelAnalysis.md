# Glade/Gryth Identity And Trust Model Analysis

Status: analysis response for `GladeGrythSecurityModelAnalysisPrompt.md` -
not a stable contract.

Scope: this answers the prompt in
`/Users/owebeeone/limbo/glial-dev/glade/dev-docs/GladeGrythSecurityModelAnalysisPrompt.md`.
It makes no implementation proposal beyond the V1 seams that must exist for a
later security model to be retrofittable.

Inputs engaged:

- `/Users/owebeeone/limbo/glial-dev/glade/dev-docs/GladeSubstrateV1.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glade/GladeKernel.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glade/GladeRecordEnvelope.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glial/GlialTrustAndCapabilityModel.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glade/GladeP2PFirstTopology.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glade/GladeBootstrapModel.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glade/GladeDeclarationModel.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/glade/GladeProviderPlacement.md`
- `/Users/owebeeone/limbo/glial-dev/dev-docs/DecisionLog.md`

## Executive Recommendation

Glade/Gryth SHOULD use a hybrid model:

```text
signed genesis
  -> pluggable trust providers
  -> normalized principal facts
  -> signed/replicated capability grants
  -> short-lived holder-bound presentations on sessions and frames
  -> mechanical Glade enforcement hooks
```

The trust provider answers "is this principal/key/group assertion real?". The
capability model answers "may this principal perform this verb on this Glade
resource right now?". These MUST remain separate.

Directory systems, OIDC, Kerberos, SSH CAs, SPIFFE/SPIRE, and local bootstrap
state SHOULD be adapters that produce normalized principal facts and key
bindings. They SHOULD NOT become the authority model embedded in Glade frames.
Glade enforcement SHOULD evaluate signed capability grants, active policy, and
revocation state against a canonical `ActionRequest`.

Capability grants SHOULD be durable Glade records in the system/declaration
plane. Frames SHOULD carry either a `capability_ref` to those records, a compact
presentation proof derived from them, or both. Presentations SHOULD be
holder-bound to the session or key that uses them; pure bearer tokens SHOULD be
reserved for short-lived local default cases only.

Agent access MUST be delegated access. An agent principal MUST NOT inherit whole
user session visibility. Its effective rights MUST be the intersection of:

- the sponsoring user's rights,
- the explicit agent grant,
- the active workspace/share policy,
- the live session binding,
- the MCP server's local tool policy.

V1 can still ship allow-all. The important point is that V1 MUST carry the
identity, capability-reference, policy-reference, and hook seams now, even when
the hook implementation returns `allow`.

## Candidate Survey

Scores: 5 = strong fit, 1 = poor fit.

| Candidate | Pluggable backends | Zero-infra default | Local-first/offline | Agent attenuation | Revocation honesty | Taut/cross-runtime fit | Overall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Directory-backed ACLs | 5 | 1 | 2 | 2 | 3 | 4 | 2.8 |
| Pure capability tokens | 3 | 5 | 5 | 5 | 3 | 4 | 4.2 |
| Workload-identity first | 4 | 2 | 3 | 2 | 3 | 4 | 3.0 |
| Recommended hybrid | 5 | 5 | 4 | 5 | 4 | 4 | 4.5 |

### Directory-backed ACLs

Shape: authenticate users through Kerberos, AD, LDAP, or OIDC; evaluate Glade
actions against group/role ACLs at the node or server.

Strengths:

- Enterprise fit is excellent.
- User and group lifecycle can reuse existing administration.
- Revocation can be fast when online and centrally checked.

Weaknesses:

- It violates the zero-infra default if used as the primary model.
- It is weak under partition unless all decisions are cached as explicit
  offline-capable grants.
- It is a poor fit for narrow agent delegation unless every agent receives a
  first-class identity and scoped grant anyway.
- It tends to make the online directory the hot-path authority, which conflicts
  with local-first Glade.

Verdict: useful as a trust/facts provider, not as the Glade authorization model.

### Pure Capability Tokens

Shape: every frame carries a bearer or holder-bound capability token
(macaroon/Biscuit/UCAN-style), and authorization is token verification.

Strengths:

- Strong local-first fit: a peer can validate grants offline if it has genesis,
  issuer keys, policy, and revocation state.
- Natural attenuation: delegated agent tokens can be narrower than user rights.
- Zero-infra default is straightforward.
- It aligns with Glade's requirement that delegation be narrower than session
  visibility.

Weaknesses:

- Enterprise identity lifecycle still needs a bridge.
- Revocation under partition is TTL-bounded at best.
- Bearer-only tokens create unacceptable blast radius if copied out of an agent
  process.
- Choosing a token family too early risks freezing a non-taut or
  non-cross-language proof format.

Verdict: right core semantics, but should be represented as Glade capability
records plus compact presentations, not as an externally blessed token family.

### Workload-Identity First

Shape: node and provider processes use workload identities such as SPIFFE/SPIRE
SVIDs, SSH CAs, or mTLS certificates. Human access is mapped indirectly through
the workload.

Strengths:

- Good for providers, build daemons, and node-to-node service identity.
- Clean key rotation and process identity are possible in managed deployments.
- It distinguishes transport/node identity from application principal identity
  better than raw iroh keys.

Weaknesses:

- It does not solve human, workspace, or agent policy by itself.
- It is infrastructure-heavy for the common personal Gryth case.
- It can accidentally authorize a machine rather than the human or agent
  responsible for an action.

Verdict: useful for nodes/providers and managed deployments, but insufficient
as the whole model.

### Recommended Hybrid

Shape: genesis names accepted trust providers and capability issuers. Trust
providers authenticate and resolve principal facts. Glial/workspace policy
issues signed capability grants into Glade records. Sessions and frames present
holder-bound capability proofs. Nodes, providers, followers, and MCP servers
all evaluate the same canonical action shape.

Strengths:

- Zero-infra default works with a local principal store, local issuer, TOFU node
  binding, and signed local genesis.
- Enterprise deployments can plug in AD/LDAP/Kerberos/OIDC for identity facts
  without making the directory the only runtime authority.
- Agents receive narrow, attributable, revocable grants.
- Offline operation is possible with honest TTL and revocation semantics.
- Glade remains mechanically verifiable: records bind principal, capability,
  policy, payload hash, and signature.

Weaknesses:

- It is more moving parts than pure ACLs.
- Revocation semantics must be explicitly policy-classed by operation risk.
- The exact presentation proof family remains a decision.

Verdict: best fit for the prompt constraints and the existing Glade docs.

## Principal Model

Glade SHOULD use one canonical `Principal` envelope with an explicit
`principal_kind` discriminator. It SHOULD NOT use one untyped string namespace
where humans, nodes, services, and agents are indistinguishable.

Recommended kinds:

| Kind | Meaning | Notes |
| --- | --- | --- |
| `human` | A human user or account. | May be backed by local store, OIDC subject, Kerberos principal, AD/LDAP DN, or federated identity. |
| `node` | A machine/node identity. | Usually bound to an iroh ed25519 node key. It proves machine possession, not human intent. |
| `agent` | An MCP client or AI/tool principal acting for a sponsor. | MUST carry `sponsor_principal_id` and explicit delegated grants. |
| `provider` | A service principal such as grazel or a workspace authority. | Claims work through provider/session records and leases. |
| `group` | A resolved collection principal. | Should be treated as facts for policy and grant issuance, not as the actor on records. |

`Session` MUST remain separate from `Principal`. A session binds:

- `session_id`,
- `origin_id`,
- asserted `principal_id`,
- `principal_kind`,
- transport peer or websocket binding,
- node id if present,
- proof/presentation references,
- policy and revocation freshness,
- destination set.

`TransportPeer` and iroh node id MUST NOT be treated as authorization identity.
They MAY be evidence in a `NodeBinding` or `SessionBinding`.

Agent-on-behalf-of-user SHOULD be represented explicitly:

```text
agent_principal {
  principal_id: "agent:local:codex:01J..."
  principal_kind: agent
  sponsor_principal_id: "human:local:alice"
  agent_class: "mcp-client"
  public_key_ref: "key:agent:01J..."
}
```

The agent's capability is valid only if:

```text
effective_rights =
  sponsor_rights
  intersect agent_grant_rights
  intersect workspace_policy
  intersect session_binding_rights
  intersect mcp_tool_policy
```

This intersection rule is the central agent safety property.

## Trust And Capability Plug Interfaces

The plug point SHOULD sit in both places:

- the local gryth/glade node, because it admits browser sessions, provider
  sessions, MCP sessions, and node peers;
- any optional glade server/router/store, because it must not route or store
  unauthorized traffic merely because a client got past another node.

Followers SHOULD also validate replayed records when they have the required
policy material. A follower MAY trust its local node for payload delivery, but
the record format MUST allow independent validation.

The interface should be split into trust and capability. Trust providers produce
facts. Capability providers/evaluators produce decisions.

### Taut-Shaped Type Sketch

This is a shape sketch, not final taut syntax:

```python
Enum("PrincipalKind",
    human=0, node=1, agent=2, provider=3, group=4)

Enum("TrustProofKind",
    local_signature=0, oidc_jwt=1, kerberos_ap_req=2,
    ldap_bind=3, ssh_ca_cert=4, spiffe_svid=5,
    iroh_node_signature=6)

Enum("GladeVerb",
    subscribe=0, append=1, exchange=2, channel=3,
    claim=4, host=5, delegate=6, decrypt=7,
    route=8, store=9, resume=10, policy_update=11)

Msg("PrincipalRef",
    F("principal_id", 1, STR),
    F("principal_kind", 2, Ref("PrincipalKind")),
    F("trust_domain", 3, STR))

Msg("PrincipalFacts",
    F("principal", 1, Ref("PrincipalRef")),
    F("display_name", 2, STR, optional=True),
    F("issuer", 3, STR),
    F("groups", 4, List(Ref("PrincipalRef"))),
    F("key_refs", 5, List(STR)),
    F("not_before_ms", 6, INT),
    F("expires_at_ms", 7, INT, optional=True),
    F("proof_hash", 8, BYTES))

Msg("ResourceSelector",
    F("workspace_id", 1, STR),
    F("share_id", 2, STR, optional=True),
    F("glade_id", 3, STR, optional=True),
    F("key_hash", 4, BYTES, optional=True),
    F("definition_id", 5, STR, optional=True),
    F("instance_id", 6, STR, optional=True))

Msg("CapabilityGrant",
    F("capability_id", 1, STR),
    F("issuer", 2, Ref("PrincipalRef")),
    F("subject", 3, Ref("PrincipalRef")),
    F("sponsor", 4, Ref("PrincipalRef"), optional=True),
    F("audience", 5, STR, optional=True),
    F("resources", 6, List(Ref("ResourceSelector"))),
    F("verbs", 7, List(Ref("GladeVerb"))),
    F("may_delegate", 8, BOOL),
    F("max_delegation_depth", 9, INT),
    F("revocation_log_ref", 10, STR),
    F("policy_ref", 11, STR),
    F("issued_at_ms", 12, INT),
    F("not_before_ms", 13, INT),
    F("expires_at_ms", 14, INT),
    F("signature", 15, BYTES))

Msg("CapabilityPresentation",
    F("presentation_id", 1, STR),
    F("session_id", 2, STR),
    F("capability_refs", 3, List(STR)),
    F("holder_key_ref", 4, STR),
    F("nonce", 5, BYTES),
    F("revocation_cursor", 6, STR),
    F("expires_at_ms", 7, INT),
    F("proof", 8, BYTES))

Msg("ActionRequest",
    F("session_id", 1, STR),
    F("principal", 2, Ref("PrincipalRef")),
    F("verb", 3, Ref("GladeVerb")),
    F("resource", 4, Ref("ResourceSelector")),
    F("frame_kind", 5, STR),
    F("capability_ref", 6, STR, optional=True),
    F("policy_ref", 7, STR),
    F("payload_hash", 8, BYTES, optional=True))
```

Trust provider service:

```python
service("TrustProvider",
    method("authenticate",
        params=[("proof_kind", Ref("TrustProofKind")), ("proof", BYTES)],
        out=Ref("PrincipalFacts")),
    method("resolve",
        params=[("principal", Ref("PrincipalRef"))],
        out=Ref("PrincipalFacts")),
    method("groups",
        params=[("principal", Ref("PrincipalRef")), ("workspace_id", STR)],
        out=List(Ref("PrincipalRef"))),
    method("verify_binding",
        params=[("principal", Ref("PrincipalRef")), ("key_ref", STR), ("proof", BYTES)],
        out=BOOL))
```

Capability evaluator service:

```python
service("CapabilityEvaluator",
    method("lookup_grant",
        params=[("capability_ref", STR)],
        out=Ref("CapabilityGrant")),
    method("validate_presentation",
        params=[("presentation", Ref("CapabilityPresentation"))],
        out=BOOL),
    method("authorize",
        params=[("action", Ref("ActionRequest"))],
        out=BOOL),
    method("revocation_status",
        params=[("capability_ref", STR), ("revocation_cursor", STR)],
        out=STR))
```

Backends map naturally:

- Local default: local principal records, local issuer key, local revocation log.
- Kerberos: `authenticate` validates an AP request or local OS ticket;
  `resolve/groups` maps to principal and group facts.
- AD/LDAP: `resolve/groups` maps DNs/SIDs/groups into Glade principal refs.
- OIDC/OAuth2: `authenticate` validates token issuer/audience/signature;
  claims become facts, but Glade grants still define resource authority.
- SSH CA: validates node/provider/user key certificates as trust evidence.
- SPIFFE/SPIRE: validates workload SVIDs for node/provider principals.
- Macaroons/Biscuit/UCAN: candidate encodings for
  `CapabilityPresentation.proof`, not the only Glade capability representation.

## Capability Shape

The authorization unit SHOULD match Glade's units:

- workspace,
- share,
- binding/glade id,
- keyed binding entry,
- definition,
- instance,
- channel,
- exchange,
- content/decrypt key,
- route/store/resume role.

The verb classes SHOULD be:

| Verb | Enforced at | Meaning |
| --- | --- | --- |
| `subscribe` | `SUBSCRIBE`, replay/hydration | Read or observe a binding/share/key. |
| `append` | `APPEND` | Write an op to an owned origin log for a binding/key. |
| `exchange` | `EXCHANGE` request/response | Invoke or answer a bounded exchange. |
| `channel` | `CHANNEL` open/data/close | Open or use a directed live channel. |
| `claim` | provider/control records | Claim work, authority, or ownership. |
| `host` | provider/node sessions | Host content, provider work, or live services. |
| `delegate` | capability-grant records | Grant narrower rights to another principal. |
| `decrypt` | key unwrap/payload access | Receive or use content keys. |
| `route` | router | Forward traffic without content access. |
| `store` | store | Persist encrypted payloads or opaque logs. |
| `resume` | resume/head exchange | Serve heads and gaps. |
| `policy_update` | system records | Update policy/trust/capability issuers. |

Ambient ACLs alone are not enough. Bearer tokens alone are also not enough. The
recommended model is:

```text
authorization decision =
  verified principal facts
  + signed capability grants
  + active policy
  + revocation state
  + session/key holder proof
  + frame action
```

## Agent To Workspace Control

Concrete declaration:

```text
agent:codex-local-01
  sponsor: human:local:alice
  audience: mcp:grazel + glade:local-node
  resources:
    workspace:A verbs: subscribe
    workspace:B verbs: subscribe
    workspace:C verbs: subscribe, exchange
  ttl: 15 minutes
  may_delegate: false
```

Checks:

1. On MCP session start, the MCP server MUST authenticate the agent principal
   and bind it to a session/key.
2. MCP workspace-listing tools MUST filter by the agent's effective
   `subscribe` rights.
3. MCP file, shell, build, and workspace tools MUST translate each call into an
   `ActionRequest`; the tool MUST deny before touching the workspace if the
   action is not authorized.
4. The glade node MUST check `SUBSCRIBE` against read/observe rights.
5. The glade node MUST check `APPEND` against write rights and origin/session
   binding.
6. The glade node/provider MUST check `EXCHANGE` and `CHANNEL` against invoke
   or channel rights.
7. Followers SHOULD validate replayed records against the record envelope and
   capability material when available.

Revocation:

- A `CapabilityRevocation` record MUST invalidate new use once observed.
- Short-lived agent grants SHOULD be the default, so partitioned revocation is
  TTL-bounded.
- On revocation observation, live agent sessions SHOULD be terminated or have
  their presentations invalidated.
- Already decrypted plaintext, local tool outputs, and filesystem side effects
  cannot be clawed back.

## Revocation And Offline Semantics

The model MUST be honest:

- Immediate global revocation under partition is impossible.
- A peer that has not observed a revocation may continue to accept an unexpired
  grant unless policy says stale revocation state fails closed.
- A peer that observes a revocation MUST reject new uses of the revoked grant.
- Previously accepted records remain attributable. Whether they remain part of
  the fold is policy and record-kind specific.

Recommended policy classes:

| Class | Example | Offline behavior |
| --- | --- | --- |
| Low risk read | local cached docs | MAY allow with unexpired grant and stale revocation cursor. |
| Normal write | append workspace op | MAY append locally, but remote peers MAY quarantine until revocation freshness is acceptable. |
| High risk invoke | shell/build/channel/control | SHOULD require fresh revocation state or a very short TTL. |
| Trust/policy update | issuer/root updates | MUST require fresh policy and should fail closed under uncertainty. |

Every grant SHOULD include:

- `expires_at`,
- `revocation_log_ref`,
- `policy_ref`,
- maximum accepted revocation staleness,
- issuer signature,
- holder/session binding where possible.

Stolen credential blast radius:

- Stolen node key: attacker can impersonate that machine for node-scoped rights
  until node bindings and node grants are revoked or expire. It MUST NOT imply
  the owner's human rights unless those rights were also delegated to the node.
- Stolen agent credential: attacker is confined to that agent's explicit
  resources, verbs, TTL, and audience, plus any plaintext already exposed to the
  agent.
- Stolen human credential: this is upstream of Glade. Glade can preserve audit
  and capability issuance records, but MFA/account recovery belongs to the trust
  provider.

## Metadata Exposure

Relay-only or store-only peers legitimately see some metadata unless extra
privacy work is done.

Minimum visible set by role:

| Role | Legitimately sees |
| --- | --- |
| Relay-only | transport peer ids, connection timing, frame sizes, opaque route/topic ids, maybe session ids. |
| Store-only | workspace/share opaque ids, origin ids, seq/head hashes, encrypted payload sizes, retention class, timing. |
| Router | subscription keys needed for fan-out: share/glade id/key hash unless blinded. |
| Authorized follower | full envelope and payload fields allowed by capability. |

The design SHOULD treat human-readable workspace names, repo paths, binding
names, operation names, and principal display names as sensitive metadata.

Mitigations:

- opaque workspace/share/glade ids on transport paths,
- encrypted payloads by workspace or share,
- blinded or hashed keyed-binding route keys,
- separate relay/store capabilities from decrypt capabilities,
- metadata classification in frame schemas,
- workspace policy that can disallow relay/store-only placement for sensitive
  workspaces.

GDL-010 should remain open until the exact redaction/blinding rules are chosen.

## Multi-Party Shares

When a share spans principals from different trust backends, no backend should
become the global authority by accident. The share genesis/ownership record
SHOULD carry:

- share/workspace identity,
- genesis id/hash,
- accepted trust domains/providers,
- accepted capability issuers,
- root owner/admin principals or quorum,
- policy update rules,
- membership/capability grant rules,
- revocation authorities,
- metadata exposure policy,
- key distribution policy,
- conflict/arbitration rule for cross-domain claims.

Example:

```text
workspace:team-alpha
  trust_domains:
    corp-ad.example
    local-invite:alice-laptop
  capability_issuers:
    principal:corp:workspace-admins
    principal:local:alice
  owner_policy:
    policy updates require corp admin OR 2-of-3 owner quorum
```

AD groups, LDAP groups, local invite records, and OIDC claims are facts. They
do not directly authorize Glade actions until a workspace policy or capability
grant maps them to Glade verbs and resources.

## V1 Seam Audit

Retrofit-cost ranking:

- Critical: omission would force wire/protocol/log identity changes later.
- High: omission would preserve wire shape but require invasive node/provider
  rewrites.
- Medium: omission is annoying but mostly additive.

| Rank | Seam | V1 requirement |
| --- | --- | --- |
| Critical | `HELLO` identity fields | `HELLO` MUST carry slots for `session_id`, `origin_id`, asserted `principal_id`, `principal_kind`, node/transport key binding, auth proof or presentation refs, `policy_ref`, feature set, and nonce/challenge. V1 may accept all, but it must parse and persist the shape. |
| Critical | Session/origin separation | `origin_id` MUST be stable for the per-origin op log and MUST NOT equal socket id or transport peer id. Origin epoch SHOULD be recorded. |
| Critical | Frame auth context | `SUBSCRIBE`, `APPEND`, `HEADS`, `EXCHANGE`, and `CHANNEL` frames MUST have fields or extension slots for `principal_id`, `capability_ref`, `policy_ref`, and action resource. |
| Critical | Record envelope authority fields | `principal_id`, `capability_ref`, `policy_ref`, expiry, revocation/supersession refs, payload hash, and signature slot MUST remain in the envelope plan. |
| Critical | Signable op base | Per-origin `(origin, seq, prev_hash, causal refs, payload_hash)` MUST be stable so later signatures can bind to the existing hash chain. |
| Critical | Enforcement hook API | The node MUST route every admission and frame decision through a no-op `authorize(ActionRequest)` hook even in allow-all mode. |
| Critical | Store/replay validation hook | Store-side append and replay MUST have a validation hook that can reject or quarantine well-formed but unauthorized records later. |
| Critical | MCP auth context propagation | MCP provider sessions MUST carry principal/session/capability context into tool calls. Otherwise agent workspace controls will be bolted on above the real access path. |
| Critical | Capability and revocation record kinds | The declaration/system plane MUST reserve `capability-grant` and `capability-revocation` record kinds and stable references. |
| High | Provider/session claim fields | Provider instance and claim records SHOULD include provider principal, session id, node binding, lease, owner term, and capability ref. |
| High | Revocation freshness | `HELLO` or session state SHOULD expose revocation cursor/freshness so policy can distinguish fresh from partitioned decisions. |
| High | Metadata boundary | Frame and envelope schemas SHOULD identify which fields are relay-visible versus encrypted payload-bound. |
| High | Key/decrypt references | Capability and envelope shapes SHOULD leave slots for key refs or wrapped-key refs, even if payload encryption is deferred. |
| High | Audit hooks | Admission, subscribe, append, exchange, channel, and MCP decisions SHOULD emit structured audit events with principal, session, action, resource, decision, and policy ref. |
| Medium | Specific proof family | Biscuit, UCAN, macaroons, JWT/CWT, or custom signed taut proofs can be chosen later if the proof bytes/ref slots exist now. |
| Medium | Enterprise connector implementations | AD/LDAP/Kerberos/OIDC/SPIFFE adapters are additive if the normalized trust-provider interface is stable. |
| Medium | Rich policy language | A first policy evaluator can be simple. The seam is the canonical `ActionRequest` and deterministic decision result. |

Minimum V1 allow-all behavior:

```text
authorize(action) -> allow(reason="v1.allow_all")
```

It should still receive the full action object and emit diagnostics. That makes
future regression tests possible without replacing the frame path.

## Zero-Infra Default Design

### First Node

1. Generate a local human principal:
   `human:local:<user>`.
2. Generate a local issuer/root key.
3. Generate or read the iroh node key.
4. Create a signed genesis bundle with:
   - local trust domain,
   - local capability issuer,
   - local revocation log,
   - accepted signature scheme,
   - system declaration space id,
   - default policy.
5. Create a `NodeBinding` that binds the iroh node key to the local human owner.
6. Issue:
   - human admin/workspace capability,
   - node `route/store/resume` capability,
   - provider capability for the local grazel authority provider if present.

This bootstrap MUST work with no directory, no hosted server, and no internet.

### Add A Machine

1. New machine generates or exposes its iroh node id and pairing nonce.
2. Existing trusted node displays/verifies the node fingerprint through a local
   pairing ceremony.
3. Existing node signs a `NodeBinding` for the new node.
4. Existing node grants selected workspace route/store/resume/decrypt rights as
   policy permits.
5. New node receives genesis, system declarations, capability grants, and latest
   revocation cursor.

TOFU MAY be allowed in personal mode, but once a node is signed, future
connections SHOULD validate the signed binding rather than repeat blind TOFU.

### Add An Agent

1. Agent generates or registers a client key.
2. User/session creates an `agent` principal sponsored by the human.
3. User grants explicit workspace/resource/verb rights with short TTL and
   `may_delegate=false` by default.
4. MCP server receives or can resolve the agent grant.
5. Glade session `HELLO` binds the agent principal, holder key, session id, and
   capability presentation.
6. MCP tools and Glade frames enforce the same effective rights.

Default agent grants SHOULD be narrow and time-bounded. "Same as my user
session" SHOULD NOT be the default.

## What Is Not Defensible

The recommended model does not defend against:

- malicious local root on a machine that holds plaintext or private keys,
- full recovery of already decrypted/cached plaintext after revocation,
- immediate global revocation while peers are partitioned,
- semantic correctness of data from an authorized but malicious writer,
- privacy of timing/size/route metadata unless GDL-010 mitigations are applied,
- human account takeover at the external identity provider,
- an MCP server that ignores the Glade action hook and directly accesses local
  files outside the authorized workspace path,
- treating iroh node id as a human identity.

The model can make these failures attributable and bounded; it cannot make them
impossible.

## Test Obligations For The Future Implementation

No code is implemented by this analysis, but the future implementation should
make the following requirements testable:

| ID | Requirement | Test shape |
| --- | --- | --- |
| SEC-55-001 | `HELLO` binds principal, session, origin, and holder key. | Unknown key denied; known key admitted; swapped key rejected. |
| SEC-55-002 | Agent rights are attenuated. | Agent with user sponsor cannot access a workspace absent from the agent grant. |
| SEC-55-003 | MCP and Glade enforce the same action. | MCP denies file/build/shell action that Glade would deny. |
| SEC-55-004 | Revocation stops new use once observed. | Grant works, revocation record arrives, same frame is rejected. |
| SEC-55-005 | Offline revocation is TTL-bounded. | Partitioned peer accepts only until grant expiry or policy staleness limit. |
| SEC-55-006 | Follower replay can reject unauthorized records. | Replay includes unauthorized append; follower rejects or quarantines it. |
| SEC-55-007 | Capability proof bytes are deterministic. | Rust/TS/Python encode the same grant/presentation to identical CBOR bytes. |
| SEC-55-008 | Relay-only peer lacks decrypt authority. | Relay forwards encrypted payload but cannot unwrap keys or inspect content. |
| SEC-55-009 | Metadata classification is enforced. | Sensitive workspace refuses transport path exposing disallowed fields. |
| SEC-55-010 | Node key theft is bounded to node grants. | Node key alone cannot exercise human or agent workspace write rights. |

## Decision Log Items To Carry Forward

This analysis does not edit the root decision log, but it suggests these
decision records:

| ID | Decision |
| --- | --- |
| SEC-55-D1 | Choose the capability presentation proof family: custom signed taut grant, Biscuit, UCAN, macaroon, JWT/CWT, or hybrid. |
| SEC-55-D2 | Define revocation freshness policy by action class and workspace sensitivity. |
| SEC-55-D3 | Define metadata exposure classes and required transport/envelope redaction for GDL-010. |
| SEC-55-D4 | Define genesis/ownership authority for multi-party shares and issuer quorum rules for GDL-008. |
| SEC-55-D5 | Define node/provider key storage and rotation requirements. |
| SEC-55-D6 | Define MCP server obligations for path confinement, tool-call attribution, and Glade action propagation. |

## Bottom Line

The model to preserve is not "pick AD" or "pick UCAN". The model to preserve is
a narrow Glade security kernel:

```text
transport proves connection
trust provider proves principal/key facts
capability grant proves resource verbs
policy evaluates action
record envelope preserves attribution
revocation bounds future use
```

Everything else should plug into that kernel.
