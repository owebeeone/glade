//! Bounded real carrier rounds, owned and drained by the actual Records service.
use super::*;
use crate::lifecycle::Console;
use crate::signing::{public_key, random_seed};
use glade_carrier_api::{CarrierAddr, CarrierLink, CarrierPort};
use glade_crdt_recovery_api::{PersistResult, ReconstructResult, ReplicaRecoverySession};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;
use tokio::task::JoinSet;
const EXPORTER: &[u8] = b"glade/ic3/node-exchange/v1";

fn unavailable(_: impl std::fmt::Debug) -> Fault {
    Fault::Unavailable
}
fn with<T>(
    replica: &IndependentReplica,
    run: impl FnOnce(&mut DiskSession<CurrentSource>) -> Result<T, Fault>,
) -> Result<T, Fault> {
    let mut held = replica.session.lock().map_err(unavailable)?;
    run(held.as_mut().ok_or(Fault::Ownership)?)
}
async fn send(link: &dyn CarrierLink, raw: &Bytes, deadline: u64) -> Result<(), Fault> {
    match tokio::time::timeout(Duration::from_millis(deadline), link.send(raw.value())).await {
        Ok(Ok(())) => Ok(()),
        _ => {
            link.close().await;
            Err(Fault::Unavailable)
        }
    }
}
async fn control(
    link: &dyn CarrierLink,
    replica: &IndependentReplica,
) -> Result<SignedRecord, Fault> {
    let raw = tokio::time::timeout(
        Duration::from_millis(replica.provision.deadline_ms),
        link.recv(),
    )
    .await
    .map_err(unavailable)?
    .map_err(unavailable)?
    .ok_or(Fault::Unavailable)?;
    decode_profile(&raw, &replica.provision.request.limits.decode).map_err(|_| Fault::Integrity)
}
async fn handshake(
    replica: &IndependentReplica,
    link: &dyn CarrierLink,
    dialing: bool,
    observed: &(TrustedCut, PolicyBody),
) -> Result<(Digest, Digest, Digest, Digest), Fault> {
    let endpoint: Digest = link
        .remote_id()
        .ok_or(Fault::Ownership)?
        .0
        .try_into()
        .map_err(|_| Fault::Ownership)?;
    let (remote, _) = replica
        .provision
        .peers
        .iter()
        .find(|(_, (key, _))| *key == endpoint)
        .ok_or(Fault::Ownership)?;
    let remote = *remote;
    let local_endpoint = public_key(&replica.provision.endpoint_seed);
    let (dialer, acceptor) = if dialing {
        (local_endpoint, endpoint)
    } else {
        (endpoint, local_endpoint)
    };
    let exporter = link.channel_binding(EXPORTER).ok_or(Fault::Ownership)?.0;
    let nonce = random_seed().map_err(unavailable)?;
    let own_role = if dialing { "dialer" } else { "acceptor" };
    let peer_role = if dialing { "acceptor" } else { "dialer" };
    let hello = with(replica, |session| {
        session.node_hello(
            observed,
            remote,
            (dialer, acceptor),
            exporter,
            nonce,
            own_role,
        )
    })?;
    send(
        link,
        &record_bytes(&hello, &replica.provision.request.limits.decode)?,
        replica.provision.deadline_ms,
    )
    .await?;
    let peer = control(link, replica).await?;
    with(replica, |session| {
        session.node_verify_hello(
            observed,
            &peer,
            remote,
            (dialer, acceptor),
            exporter,
            peer_role,
        )
    })?;
    let peer_body: ExchangeHello =
        decode_signed_body(&peer, &replica.provision.request.limits.decode)
            .map_err(|_| Fault::Integrity)?;
    let confirmed = with(replica, |session| {
        session.node_hello(
            observed,
            remote,
            (dialer, acceptor),
            exporter,
            peer_body.nonce,
            &format!("{own_role}-confirmed"),
        )
    })?;
    send(
        link,
        &record_bytes(&confirmed, &replica.provision.request.limits.decode)?,
        replica.provision.deadline_ms,
    )
    .await?;
    let response = control(link, replica).await?;
    with(replica, |session| {
        session.node_verify_hello(
            observed,
            &response,
            remote,
            (dialer, acceptor),
            exporter,
            &format!("{peer_role}-confirmed"),
        )
    })?;
    let body: ExchangeHello =
        decode_signed_body(&response, &replica.provision.request.limits.decode)
            .map_err(|_| Fault::Integrity)?;
    if body.nonce != nonce {
        return Err(Fault::Integrity);
    }
    Ok((remote, exporter, nonce, peer_body.nonce))
}
async fn recv_frame(link: &dyn CarrierLink, bound: usize) -> Result<Bytes, Fault> {
    let raw = link
        .recv()
        .await
        .map_err(unavailable)?
        .ok_or(Fault::Unavailable)?;
    if raw.len() > bound {
        return Err(Fault::Capacity);
    }
    Ok(Bytes::new(raw))
}
async fn guarded<F: Future<Output = Result<Bytes, Fault>>>(
    replica: &IndependentReplica,
    link: &dyn CarrierLink,
    observed: &(TrustedCut, PolicyBody),
    source: Digest,
    channel: Digest,
    role: &str,
    make: impl FnOnce(usize) -> F,
) -> Result<Bytes, Fault> {
    let received = with(replica, |session| {
        let guard = session.node_guard(
            &observed.0,
            source,
            channel,
            role,
            replica.provision.deadline_ms,
        )?;
        let bound = guard.max_bytes;
        let permit = session.begin_ingress(guard)?;
        session.receive_ingress(permit, || make(bound))
    })?;
    tokio::pin!(received);
    let (permit, result) = tokio::select! {
        result=&mut received=>result,
        ()=tokio::time::sleep(Duration::from_millis(replica.provision.deadline_ms))=> {
            link.close().await;
            received.await
        }
    };
    match result {
        Ok(raw) => {
            let expected = (|| {
                if role == "bundle" {
                    let bundle: TransferBundle =
                        decode_profile(raw.value(), &replica.provision.request.limits.decode)
                            .map_err(|_| Fault::Integrity)?;
                    Ok(vec![crate::independent::records::node_reference(
                        &bundle, &raw,
                    )])
                } else {
                    let (_, inventory): (SignedRecord, Inventory) =
                        decode(raw.value(), &replica.provision.request.limits.decode)
                            .map_err(|_| Fault::Integrity)?;
                    Ok(inventory.entries)
                }
            })();
            let expected = match expected {
                Ok(expected) => expected,
                Err(error) => {
                    with(replica, |session| {
                        match session.retain_ingress_loss(permit, raw) {
                            PersistResult::Committed { .. } => Ok(()),
                            PersistResult::Unknown => Err(Fault::Unavailable),
                            PersistResult::Refused(fault) => Err(fault),
                        }
                    })?;
                    return Err(error);
                }
            };
            with(replica, |session| {
                session.settle_node_input(permit, raw.clone(), expected)
            })?;
            Ok(raw)
        }
        Err(error) => {
            with(replica, |session| {
                match session.retain_ingress_loss(permit, Bytes::new(vec![])) {
                    PersistResult::Committed { .. } => Ok(()),
                    PersistResult::Unknown => Err(Fault::Unavailable),
                    PersistResult::Refused(fault) => Err(fault),
                }
            })?;
            Err(error)
        }
    }
}
fn inventory_body(inventory: &Inventory, limits: &DecodeLimits) -> Result<Bytes, Fault> {
    encode_profile(
        &InventoryPage {
            instance: inventory.instance.clone(),
            inventory: encode_profile(inventory, limits)
                .map_err(|_| Fault::Capacity)?
                .digest(),
            index: 0,
            count: 1,
            entries: inventory.entries.clone(),
        },
        limits,
    )
    .map_err(|_| Fault::Capacity)
}
async fn inventory_phase(
    replica: &IndependentReplica,
    link: &dyn CarrierLink,
    observed: &(TrustedCut, PolicyBody),
    remote: Digest,
    channel: Digest,
    role: &str,
    own: &Inventory,
) -> Result<Inventory, Fault> {
    let header = with(replica, |session| {
        session.node_inventory_header(observed, own, remote, channel)
    })?;
    // Both sides send their finite phase frames, then only this single owned
    // receive consumes the peer's exact two frames. No competing receiver.
    send(
        link,
        &record_bytes(&header, &replica.provision.request.limits.decode)?,
        replica.provision.deadline_ms,
    )
    .await?;
    if !own.entries.is_empty() {
        send(
            link,
            &inventory_body(own, &replica.provision.request.limits.decode)?,
            replica.provision.deadline_ms,
        )
        .await?;
    }
    let receive = |bound: usize| async move {
        let header = recv_frame(link, bound).await?;
        let signed: SignedRecord =
            decode_profile(header.value(), &replica.provision.request.limits.decode)
                .map_err(|_| Fault::Integrity)?;
        let body: InventoryHeaderBody =
            decode_signed_body(&signed, &replica.provision.request.limits.decode)
                .map_err(|_| Fault::Integrity)?;
        if body.pages != usize::from(body.entries != 0)
            || body.entries > replica.provision.request.limits.inventory_items
            || body.total_bytes > replica.provision.request.limits.inventory_bytes
        {
            return Err(Fault::Capacity);
        }
        let entries = if body.pages == 0 {
            vec![]
        } else {
            let rest = bound
                .checked_sub(header.value().len())
                .ok_or(Fault::Capacity)?;
            let page = recv_frame(link, rest).await?;
            let page: InventoryPage =
                decode_profile(page.value(), &replica.provision.request.limits.decode)
                    .map_err(|_| Fault::Integrity)?;
            if page.instance != body.instance
                || page.inventory != body.manifest
                || page.index != 0
                || page.count != body.pages
                || page.entries.len() != body.entries
            {
                return Err(Fault::BindingMismatch);
            }
            page.entries
        };
        let inventory = Inventory {
            instance: body.instance.clone(),
            source: body.source,
            number: body.number,
            entries,
            total_bytes: body.total_bytes,
        };
        let mut limits = replica.provision.request.limits.decode.clone();
        limits.bytes = bound;
        encode(&(signed, inventory), &limits).map_err(|_| Fault::Capacity)
    };
    let raw = guarded(replica, link, observed, remote, channel, role, receive).await?;
    let (_, inventory): (SignedRecord, Inventory) =
        decode(raw.value(), &replica.provision.request.limits.decode)
            .map_err(|_| Fault::Integrity)?;
    Ok(inventory)
}
async fn round(
    replica: &IndependentReplica,
    link: Arc<dyn CarrierLink>,
    dialing: bool,
    console: &dyn Console,
) -> Result<Digest, Fault> {
    // One trusted observation per round, made by the exchange gate's holder
    // before its first HELLO; all of the round's steps authorize against it.
    // A busy round (gate held elsewhere) answers on its own single observation
    // and begins no guard.
    let owner = replica.exchange_gate.try_lock();
    let observed = with(replica, |session| session.node_observe())?;
    let (remote, channel, own_nonce, peer_nonce) =
        handshake(replica, link.as_ref(), dialing, &observed).await?;
    let local_endpoint = public_key(&replica.provision.endpoint_seed);
    let remote_endpoint: Digest = link
        .remote_id()
        .ok_or(Fault::Ownership)?
        .0
        .try_into()
        .map_err(|_| Fault::Ownership)?;
    let (dialer, acceptor) = if dialing {
        (local_endpoint, remote_endpoint)
    } else {
        (remote_endpoint, local_endpoint)
    };
    let own_role = if dialing { "dialer" } else { "acceptor" };
    let peer_role = if dialing { "acceptor" } else { "dialer" };
    let role = format!(
        "{own_role}-{}",
        if owner.is_ok() { "ready" } else { "busy" }
    );
    let ready = with(replica, |session| {
        session.node_hello(
            &observed,
            remote,
            (dialer, acceptor),
            channel,
            peer_nonce,
            &role,
        )
    })?;
    send(
        link.as_ref(),
        &record_bytes(&ready, &replica.provision.request.limits.decode)?,
        replica.provision.deadline_ms,
    )
    .await?;
    let _owner = owner.map_err(|_| Fault::Capacity)?;
    *replica.exchange_active.lock().map_err(unavailable)? = Some((
        if dialing {
            public_key(&replica.provision.node_seed)
        } else {
            remote
        },
        link.clone(),
    ));
    let result = async {
        let response = control(link.as_ref(), replica).await?;
        let body: ExchangeHello =
            decode_signed_body(&response, &replica.provision.request.limits.decode)
                .map_err(|_| Fault::Integrity)?;
        let busy = body.role == format!("{peer_role}-busy");
        let expected = format!("{peer_role}-{}", if busy { "busy" } else { "ready" });
        with(replica, |session| {
            session.node_verify_hello(
                &observed,
                &response,
                remote,
                (dialer, acceptor),
                channel,
                &expected,
            )
        })?;
        if body.nonce != own_nonce {
            return Err(Fault::Integrity);
        }
        if busy {
            return Err(Fault::Capacity);
        }
        round_history(replica, link.as_ref(), &observed, remote, channel, console).await
    }
    .await;
    replica.exchange_active.lock().map_err(unavailable)?.take();
    result
}

async fn round_history(
    replica: &IndependentReplica,
    link: &dyn CarrierLink,
    observed: &(TrustedCut, PolicyBody),
    remote: Digest,
    channel: Digest,
    console: &dyn Console,
) -> Result<Digest, Fault> {
    let records = with(replica, |session| session.node_history())?;
    let snapshot = encode(&records, &replica.provision.request.limits.decode)
        .map_err(|_| Fault::Capacity)?
        .digest();
    let own = with(replica, |session| session.node_inventory(&records))?;
    let incoming = inventory_phase(replica, link, observed, remote, channel, "inventory", &own)
        .await
        .map_err(|error| {
            console.err(&format!("independent inventory: {error:?}"));
            error
        })?;
    let missing = with(replica, |session| session.node_missing(&incoming))?;
    let requested = inventory_phase(replica, link, observed, remote, channel, "page", &missing)
        .await
        .map_err(|error| {
            console.err(&format!("independent request: {error:?}"));
            error
        })?;
    if requested
        .entries
        .iter()
        .any(|reference| !own.entries.contains(reference))
        || requested.total_bytes != 0
    {
        return Err(Fault::BindingMismatch);
    }
    let sender = async {
        for reference in &requested.entries {
            let bundle = records
                .iter()
                .find(|bundle| {
                    encode_profile(*bundle, &replica.provision.request.limits.decode).is_ok_and(
                        |raw| {
                            crate::independent::records::node_reference(bundle, &raw) == *reference
                        },
                    )
                })
                .ok_or(Fault::Integrity)?;
            let raw = encode_profile(bundle, &replica.provision.request.limits.decode)
                .map_err(|_| Fault::Capacity)?;
            send(link, &raw, replica.provision.deadline_ms).await?;
        }
        Ok::<_, Fault>(())
    };
    let receiver = async {
        for reference in &missing.entries {
            let raw = guarded(
                replica,
                link,
                observed,
                remote,
                channel,
                "bundle",
                |bound| recv_frame(link, bound),
            )
            .await?;
            let bundle: TransferBundle =
                decode_profile(raw.value(), &replica.provision.request.limits.decode)
                    .map_err(|_| Fault::Integrity)?;
            with(replica, |session| {
                session.node_offer(bundle, reference, &incoming, remote, channel)
            })
            .map_err(|error| {
                console.err(&format!("independent native offer: {error:?}"));
                error
            })?;
        }
        Ok::<_, Fault>(())
    };
    let send_owned = async {
        let result = sender.await;
        if result.is_err() {
            link.close().await;
        }
        result
    };
    let recv_owned = async {
        let result = receiver.await;
        if result.is_err() {
            link.close().await;
        }
        result
    };
    let (sent, received) = tokio::join!(send_owned, recv_owned);
    sent?;
    received?;
    with(replica, |session| session.classify_node_inputs())?;
    if !missing.entries.is_empty() {
        console.out(&format!("independent exchanged {}", missing.entries.len()));
    }
    Ok(snapshot)
}
pub(crate) async fn serve(
    replica: Arc<IndependentReplica>,
    console: Arc<dyn Console>,
    stop: impl Future<Output = ()>,
) -> io::Result<()> {
    let links = Arc::new(Mutex::new(BTreeMap::<u64, Arc<dyn CarrierLink>>::new()));
    let mut tasks = JoinSet::new();
    let mut next = 0_u64;
    let mut successful = BTreeMap::<Digest, Digest>::new();
    let mut active = std::collections::BTreeSet::new();
    let mut retry_after = BTreeMap::<Digest, tokio::time::Instant>::new();
    let mut reported_idle = None;
    let mut tick = tokio::time::interval(Duration::from_millis(replica.provision.tick_ms));
    tokio::pin!(stop);
    let result=async {
    // One inbound attempt at a time, kept across the loop's iterations: a tick
    // or a finished task never drops a handshake in flight (nor its
    // connection). It is re-created only once it completes; the one pending
    // when the loop ends is dropped once, with this block, before the drain.
    let mut accepting=replica.carrier.accept();
    loop {
        tokio::select! {
            ()=&mut stop=> {break;}
            _=tick.tick()=> {
                let current=with(&replica,|session|session.node_history()).and_then(|records|encode(&records,&replica.provision.request.limits.decode).map(|raw|raw.digest()).map_err(|_|Fault::Capacity));
                let Ok(current)=current else {continue;};
                if !replica.provision.peers.is_empty() && tasks.is_empty() && active.is_empty()
                    && replica.provision.peers.iter().all(|(node,_)|successful.get(node)==Some(&current)) && reported_idle!=Some(current) {
                    let settled=with(&replica,|session|{
                        let glade_crdt_recovery_api::LoadResult::Validated(loaded)=session.load()else{return Err(Fault::Unavailable);};
                        if !loaded.image.permanent_loss.is_empty() || loaded.authoritative_floor.guards.values().any(|guard|matches!(guard,GuardStatus::Active(_))) || loaded.image.obligations.iter().any(|item|matches!(item.classification,Classification::Missing)) || !loaded.image.state.queries.is_empty() || !loaded.image.state.preparing.is_empty() || !loaded.image.state.commits.is_empty() || !loaded.image.state.unknown_commits.is_empty() || !loaded.image.state.storage_invocations.is_empty() || !loaded.image.state.lookups.is_empty(){return Err(Fault::Unavailable);}
                        let request=session.reconstruction_request()?;match session.reconstruct_cut(request){ReconstructResult::Cut(cut)=>Ok(cut.complete_local),ReconstructResult::Refused(fault)=>Err(fault),ReconstructResult::Unavailable=>Err(Fault::Unavailable)}
                    });
                    if let Ok(complete)=settled{console.out(&format!("independent idle {} complete={complete}",crate::transport::hex(&current)));reported_idle=Some(current);}
                }
                for (node,(_,address)) in &replica.provision.peers {
                    if active.contains(node)||successful.get(node)==Some(&current)||tasks.len()>=16 ||retry_after.get(node).is_some_and(|time|*time>tokio::time::Instant::now()) {continue;}
                    let remote=*node;let address=address.clone();let owned=replica.clone();let report=console.clone();let held=links.clone();
                    let id=next;next=next.checked_add(1).ok_or_else(||invalid("link namespace exhausted"))?;
                    active.insert(remote);
                    tasks.spawn(async move {
                        let result=async {
                            let link=tokio::time::timeout(Duration::from_millis(owned.provision.deadline_ms),owned.carrier.dial(&CarrierAddr(address))).await.map_err(unavailable)?.map_err(unavailable)?;
                            let link:Arc<dyn CarrierLink>=Arc::from(link);
                            held.lock().map_err(unavailable)?.insert(id,link.clone());
                            let result=round(&owned,link.clone(),true,report.as_ref()).await;
                            link.close().await;held.lock().map_err(unavailable)?.remove(&id);
                            result
                        }.await;
                        (Some(remote),true,result)
                    });
                }
            }
            accepted=&mut accepting,if tasks.len()<16=> {
                accepting=replica.carrier.accept();
                if let Ok(Some(link))=accepted {
                    let id=next;next=next.checked_add(1).ok_or_else(||invalid("link namespace exhausted"))?;
                    let link:Arc<dyn CarrierLink>=Arc::from(link);
                    links.lock().map_err(|_|invalid("link custody poisoned"))?.insert(id,link.clone());
                    let remote=link.remote_id().and_then(|id|{let endpoint:Digest=id.0.try_into().ok()?;replica.provision.peers.iter().find_map(|(node,(key,_))|(*key==endpoint).then_some(*node))});
                    let owned=replica.clone();let report=console.clone();let held=links.clone();
                    tasks.spawn(async move {
                        let result=round(&owned,link.clone(),false,report.as_ref()).await;
                        link.close().await;held.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
                        (remote,false,result)
                    });
                }
            }
            joined=tasks.join_next(),if !tasks.is_empty()=> {
                if let Some(Err(error))=&joined {return Err(invalid(error));}
                if let Some(Ok((remote,outbound,result)))=joined {
                    if let Some(remote)=remote {
                        completed(&mut active,remote,outbound);
                        if let Ok(snapshot)=&result {successful.insert(remote,*snapshot);retry_after.remove(&remote);}
                        if result==Err(Fault::Capacity) && public_key(&replica.provision.node_seed)>remote {retry_after.insert(remote,tokio::time::Instant::now()+Duration::from_millis(replica.provision.deadline_ms));}
                    }
                    if let Err(error)=result {console.err(&format!("independent exchange refused: {error:?}"));}
                }
            }
        }
    }
    Ok(())
    }.await;
    finish_service(result, links, tasks).await
}
type Work = JoinSet<(Option<Digest>, bool, Result<Digest, Fault>)>;
fn completed(active: &mut std::collections::BTreeSet<Digest>, remote: Digest, outbound: bool) {
    if outbound {
        active.remove(&remote);
    }
}
async fn finish_service(
    result: io::Result<()>,
    links: Arc<Mutex<BTreeMap<u64, Arc<dyn CarrierLink>>>>,
    mut tasks: Work,
) -> io::Result<()> {
    let owned: Vec<_> = links
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for link in owned {
        link.close().await;
    }
    let mut failed = None;
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            failed = Some(invalid(error));
        }
    }
    result?;
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod interruption;
    #[test]
    fn inbound_busy_completion_keeps_original_outbound_worker_registered() {
        let remote = [1; 32];
        let mut active = std::collections::BTreeSet::from([remote]);
        completed(&mut active, remote, false);
        assert!(
            active.contains(&remote),
            "a refused inbound link cannot release another owned outbound worker"
        );
        completed(&mut active, remote, true);
        assert!(active.is_empty());
    }
    #[tokio::test]
    async fn normal_service_exit_drains_owned_callbacks() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let done = Arc::new(AtomicBool::new(false));
        let held = done.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            tokio::task::yield_now().await;
            held.store(true, Ordering::SeqCst);
            (None, false, Ok([0; 32]))
        });
        finish_service(Ok(()), Arc::new(Mutex::new(BTreeMap::new())), tasks)
            .await
            .unwrap();
        assert!(done.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn failed_service_exit_drains_owned_callbacks_before_returning_error() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let done = Arc::new(AtomicBool::new(false));
        let held = done.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            tokio::task::yield_now().await;
            held.store(true, Ordering::SeqCst);
            (None, false, Ok([0; 32]))
        });
        let result = finish_service(
            Err(invalid("owned service failure")),
            Arc::new(Mutex::new(BTreeMap::new())),
            tasks,
        )
        .await;
        assert!(result.is_err());
        assert!(
            done.load(Ordering::SeqCst),
            "service returned before owned callback drain"
        );
    }
    #[test]
    fn inventory_body_uses_qualified_indexed_manifest_bound_page() {
        let inventory = Inventory {
            instance: Instance {
                descriptor: [1; 32],
                incarnation: "page".into(),
            },
            source: [2; 32],
            number: 3,
            entries: vec![RecordRef {
                kind: RecordKind::Candidate,
                origin: "writer".into(),
                seq: 0,
                operation: [3; 32],
                bundle: [4; 32],
                receipt: None,
            }],
            total_bytes: 10,
        };
        let limits = super::super::bounds();
        let raw = inventory_body(&inventory, &limits).unwrap();
        let page: InventoryPage =
            decode_profile(raw.value(), &limits).expect("exact qualified InventoryPage body");
        assert_eq!(page.instance, inventory.instance);
        assert_eq!(page.index, 0);
        assert_eq!(page.count, 1);
        assert_eq!(
            page.inventory,
            encode_profile(&inventory, &limits).unwrap().digest()
        );
        assert_eq!(page.entries, inventory.entries);
    }
}
