# Glade/Gryth Identity & Trust Model — Analysis Prompt

Status: analysis prompt — **commissions comparative analyses; this is a
problem statement, not a design**

Purpose: state the identity/trust/capability problem for the Glade substrate
and the gryth deployment well enough that independent analyses can be run
against it. The substrate build (`GladeSubstrateV1.md`) deliberately punts on
the security model; this document defines what is being punted, what the punt
MUST NOT foreclose, and the questions the analyses must answer.

## 1. Context

- Substrate: `GladeSubstrateV1.md`. Per-origin op logs with shape-driven
  folds; sessions with destinations; glade server = router + store + resume;
  authorities attach as provider sessions. First target: **rust + taut +
  iroh p2p**, evolving the model last trialled in the grip-lab service.
- Topology: gryth SPA → (websocket, glade) → local rust+iroh node →
  (iroh, glade) → other nodes. grazel (build/workspace server) attaches as an
  authority provider session.
- MCP: nodes will expose MCP servers so agents can operate on workspaces.
  Agent access is a first-class access path, not an afterthought.

## 2. What changed since grip-lab

| Dimension | grip-lab | gryth/glade |
| --- | --- | --- |
| Workspace address | machine name + account | (VM or machine) + worktree |
| Transport identity | SSH (the entire security model) | iroh node id (ed25519) = machine identity, nothing more |
| Access subjects | humans with shell accounts | humans, nodes, **and agents via MCP** |
| Sharing | none (point-to-point) | multi-party shares, replicated logs, relayed frames |

SSH conflated identity, trust, and authorization into one mechanism and it
was adequate. The replacement must separate them, because the subjects
(humans / machines / agents), the resources (workspaces, shares, bindings),
and the paths (direct, relayed, replicated) no longer line up one-to-one.

## 3. The problem

Four sub-problems, deliberately separated:

1. **Identity** — who is acting. Principal types: human users, machines/nodes
   (iroh keypairs exist already), and agents (MCP clients acting on behalf of
   a user, with rights that must be narrower than the user's). What binds an
   agent to its sponsoring user? What binds a node to an owner?
2. **Trust** — who vouches. How does a node decide an asserted principal is
   real: local database, enterprise directory, federation, web-of-trust,
   TOFU? Multi-machine single-user (the common gryth case) and multi-user
   team cases both matter.
3. **Capability** — what is allowed. The unit of authorization should align
   with glade's units: workspace, share, binding (glade id), key, and verb
   class (subscribe / append / exchange / channel). The central product
   requirement: **control which agents can access which workspaces**, with
   attenuation (an agent gets a strict subset of its sponsor's rights) and
   revocation.
4. **Enforcement** — where checks happen. Candidate enforcement points are
   already visible in the substrate: session `HELLO` (admit/deny + principal
   binding), `SUBSCRIBE` (read), `APPEND` (write), `EXCHANGE`/`CHANNEL`
   (invoke), and store-side replay (does a follower verify, or trust its
   node?). Relayed/replicated paths mean some peers see envelopes without
   being authorized for payloads (metadata exposure — GDL-010).

## 4. Constraints

- **Pluggable backends.** The trust and capability sources MUST be pluggable:
  Kerberos, MS AD, LDAP, or any trust/capability-shaped API. Analyses SHOULD
  also cover OIDC/OAuth2, SSH CAs, SPIFFE/SPIRE workload identity, and
  capability-token families (macaroons, biscuit, UCAN) as candidate shapes —
  the goal is the right *interface*, not a blessed vendor.
- **Zero-infra default.** A default provider MUST work with no security
  infrastructure: single user, their machines, local database, sensible
  trust bootstrap (e.g. TOFU + iroh node keys + a local principal store).
  Setting up gryth on a laptop must not require a directory server.
- **Local-first survives.** Offline operation and the optional glade server
  are load-bearing substrate properties. Authorization MUST NOT require a
  central online authority on the hot path; revocation semantics under
  partition must be stated honestly (GDL-009).
- **Punt without foreclosing.** V1 substrate ships allow-all, but the seams
  ship now: principal id asserted at `HELLO`, capability-ref slots in the
  record envelope (per `GladeRecordEnvelope.md` authority fields), per-frame
  enforcement points as no-op hooks, and the per-origin hash chain (GQ-9)
  as the integrity base signatures can later attach to. The analyses MUST
  identify any seam that cannot be added later so it gets into V1.
- **Cross-runtime.** Whatever tokens/proofs exist must be taut-expressible
  (deterministic CBOR, Rust/TS/Python parity).

## 5. Existing material to engage (not reinvent)

- `glial-dev/dev-docs/glade/GladeKernel.md` — delegated capability
  invariants: narrower than session visibility, attributable, revocable.
- `glial-dev/dev-docs/glade/GladeRecordEnvelope.md` — envelope authority
  fields (`principal_id`, `capability_ref`, `policy_ref`, signature scope).
- `glial-dev/dev-docs/glial/GlialTrustAndCapabilityModel.md` — prior
  layer-level thinking.
- `glial-dev/dev-docs/DecisionLog.md` — GDL-004 (delegated references),
  GDL-008 (genesis authority), GDL-009 (capability revocation offline),
  GDL-010 (metadata exposure to relay-only peers), GDL-016 (provisioning
  authority).

## 6. Questions the analyses must answer

1. Principal model: one principal type with attributes, or distinct
   human/node/agent types? How is agent-on-behalf-of-user represented and
   attenuated?
2. Trust plug interface: what is the minimal provider API (authenticate,
   resolve principal, enumerate groups/roles?) that Kerberos/AD/LDAP/OIDC and
   a local default can all implement? Where does it sit — node, glade
   server, both?
3. Capability shape: ambient ACLs evaluated at enforcement points vs bearer
   capability tokens (macaroon/biscuit/UCAN-style) carried in frames? Hybrid?
   How do capabilities name glade resources (workspace / share / glade id /
   key / verb)?
4. Agent⇄workspace control: concretely, how does "agent X may access
   workspaces A,B read-only, C with exchange rights" get declared, checked at
   MCP and at glade enforcement points, and revoked?
5. Revocation and offline: what guarantees are honest under partition —
   TTL-bounded grants, revocation records in a share, both? What is the
   blast radius of a stolen node key / agent credential?
6. Metadata exposure: what do relay/store-only nodes legitimately see
   (envelopes, glade ids, sizes, timing) and is that acceptable per workspace
   sensitivity?
7. Multi-party shares: when a share spans principals from different trust
   backends (one user on AD, one on the local default), who admits, who
   arbitrates, what does the genesis/ownership record carry (GDL-008)?
8. V1 seam audit: which envelope fields, frame fields, and node-side hooks
   must exist *now* (even unenforced) for each candidate model to be
   retrofittable?

## 7. Deliverables expected from an analysis

1. A comparative survey of 2–4 candidate models (e.g. directory-backed ACL,
   capability-token, workload-identity hybrid) scored against §4 constraints.
2. A recommended principal/trust/capability model with the provider plug
   interface sketched (taut-shaped types).
3. The V1 seam list (§6 Q8) — concrete fields and hook points, ranked by
   retrofit cost if omitted.
4. A default-mode design sketch: zero-infra single-user multi-machine setup,
   including the bootstrap ceremony (first node, adding a machine, adding an
   agent).
5. Explicit statement of what is NOT defensible in the recommended model
   (threat classes out of scope, e.g. malicious local root).

## 8. Non-goals

- Implementing any of it now — V1 ships allow-all with seams.
- Product-level policy UX (sharing dialogs, consent flows) — Glial-layer.
- Payload encryption-at-rest strategy — related, but analyzed separately
  unless a candidate model forces it (state if it does).
