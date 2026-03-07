use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        apps::v1::{DaemonSet, Deployment, StatefulSet},
        batch::v1::{CronJob, Job},
        core::v1::{
            ConfigMap, Namespace, Node, PersistentVolume, PersistentVolumeClaim, Pod, Secret,
            Service,
        },
        networking::v1::Ingress,
    },
    apimachinery::pkg::apis::meta::v1::ObjectMeta,
    jiff::{SpanRound, Timestamp, Unit},
};
use kube::{
    api::Api,
    runtime::{watcher, WatchStreamExt},
    Client, Resource, ResourceExt,
};
use serde::de::DeserializeOwned;
use skim::SkimItemSender;
use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    pin::pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Duration,
};
use tokio::sync::Notify;

use crate::items::{ItemState, K8sItem, ResourceKind};
use crate::k8s::discovery::{dynamic_status, DiscoveredCrd};

/// All resource kinds to watch when no filter is given.
pub const ALL_KINDS: &[ResourceKind] = &[
    ResourceKind::Pod,
    ResourceKind::Deployment,
    ResourceKind::StatefulSet,
    ResourceKind::DaemonSet,
    ResourceKind::Service,
    ResourceKind::Ingress,
    ResourceKind::Job,
    ResourceKind::CronJob,
    ResourceKind::ConfigMap,
    ResourceKind::Secret,
    ResourceKind::PersistentVolume,
    ResourceKind::PersistentVolumeClaim,
    ResourceKind::Namespace,
    ResourceKind::Node,
];

/// Watch the given resource kinds from the cluster, streaming live updates into skim.
/// `context` is a display label attached to every item (empty string in single-cluster mode).
/// Initial items from ALL watchers are collected into a shared buffer and sent as a single
/// globally-sorted (unhealthy first) batch once every watcher has completed its `InitDone`.
/// Falls back to sending whatever was collected after 8 seconds to handle slow/failing watchers.
/// Subsequent Apply/Delete events are streamed in real-time.
/// Automatically reconnects on watch failures via `default_backoff`.
pub async fn watch_resources(
    client: Client,
    tx: SkimItemSender,
    kinds: &[ResourceKind],
    crds: &[DiscoveredCrd],
    context: &str,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> Result<()> {
    let total_watchers = kinds.len() + crds.len();
    let global_init: Arc<Mutex<Vec<K8sItem>>> = Arc::new(Mutex::new(Vec::new()));
    let done_count: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let all_init_done: Arc<Notify> = Arc::new(Notify::new());

    // Coordinator task: waits for all watchers to finish initial list (or 8s timeout),
    // then globally sorts and sends the full initial batch to skim.
    {
        let global_init = global_init.clone();
        let tx_coord = tx.clone();
        let all_init_done = all_init_done.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = all_init_done.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(8)) => {}
            }
            let mut buf = global_init.lock().unwrap();
            buf.sort_by_key(|item| {
                let status = item.status();
                std::cmp::Reverse(status_priority(&status))
            });
            let sorted: Vec<Arc<dyn skim::SkimItem>> = buf
                .drain(..)
                .map(|item| Arc::new(item) as Arc<dyn skim::SkimItem>)
                .collect();
            if !sorted.is_empty() {
                let _ = tx_coord.send(sorted);
            }
        });
    }

    let mut tasks = Vec::new();

    for kind in kinds {
        let c = client.clone();
        let t = tx.clone();
        let k = kind.clone();
        let ctx = context.to_string();
        let ns = namespace.map(str::to_string);
        let ls = label_selector.map(str::to_string);
        let gi = global_init.clone();
        let dc = done_count.clone();
        let aid = all_init_done.clone();

        tasks.push(tokio::spawn(async move {
            let result = match k {
                ResourceKind::Pod => {
                    watch_typed::<Pod, _>(
                        c,
                        t,
                        ResourceKind::Pod,
                        pod_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Service => {
                    watch_typed::<Service, _>(
                        c,
                        t,
                        ResourceKind::Service,
                        service_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Deployment => {
                    watch_typed::<Deployment, _>(
                        c,
                        t,
                        ResourceKind::Deployment,
                        deploy_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::StatefulSet => {
                    watch_typed::<StatefulSet, _>(
                        c,
                        t,
                        ResourceKind::StatefulSet,
                        statefulset_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::DaemonSet => {
                    watch_typed::<DaemonSet, _>(
                        c,
                        t,
                        ResourceKind::DaemonSet,
                        daemonset_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::ConfigMap => {
                    watch_typed::<ConfigMap, _>(
                        c,
                        t,
                        ResourceKind::ConfigMap,
                        |_| "ConfigMap".to_string(),
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Secret => {
                    watch_typed::<Secret, _>(
                        c,
                        t,
                        ResourceKind::Secret,
                        secret_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Ingress => {
                    watch_typed::<Ingress, _>(
                        c,
                        t,
                        ResourceKind::Ingress,
                        ingress_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                // Cluster-scoped resources always use Api::all regardless of --namespace
                ResourceKind::Node => {
                    watch_typed::<Node, _>(
                        c,
                        t,
                        ResourceKind::Node,
                        node_status,
                        ctx,
                        None,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Namespace => {
                    watch_typed::<Namespace, _>(
                        c,
                        t,
                        ResourceKind::Namespace,
                        namespace_status,
                        ctx,
                        None,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                // Cluster-scoped — namespace ignored
                ResourceKind::PersistentVolume => {
                    watch_typed::<PersistentVolume, _>(
                        c,
                        t,
                        ResourceKind::PersistentVolume,
                        pv_status,
                        ctx,
                        None,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::PersistentVolumeClaim => {
                    watch_typed::<PersistentVolumeClaim, _>(
                        c,
                        t,
                        ResourceKind::PersistentVolumeClaim,
                        pvc_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Job => {
                    watch_typed::<Job, _>(
                        c,
                        t,
                        ResourceKind::Job,
                        job_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::CronJob => {
                    watch_typed::<CronJob, _>(
                        c,
                        t,
                        ResourceKind::CronJob,
                        cronjob_status,
                        ctx,
                        ns,
                        ls.clone(),
                        gi,
                        dc,
                        total_watchers,
                        aid,
                    )
                    .await
                }
                ResourceKind::Custom(_) => {
                    // CRDs are handled via watch_dynamic, not watch_typed.
                    Ok(())
                }
            };

            if let Err(e) = result {
                eprintln!("\n[kuberift] {e}");
            }
        }));
    }

    // Spawn dynamic watchers for each discovered CRD.
    for crd in crds {
        let c = client.clone();
        let t = tx.clone();
        let ctx = context.to_string();
        let ns = namespace.map(str::to_string);
        let ls = label_selector.map(str::to_string);
        let gi = global_init.clone();
        let dc = done_count.clone();
        let aid = all_init_done.clone();
        let kind = ResourceKind::Custom(crd.plural.clone());
        let ar = crd.api_resource.clone();
        let namespaced = crd.namespaced;

        tasks.push(tokio::spawn(async move {
            if let Err(e) = watch_dynamic(
                c,
                t,
                kind,
                ar,
                namespaced,
                dynamic_status,
                ctx,
                ns,
                ls,
                gi,
                dc,
                total_watchers,
                aid,
            )
            .await
            {
                eprintln!("\n[kuberift] {e}");
            }
        }));
    }

    for task in tasks {
        if let Err(e) = task.await {
            eprintln!("[kuberift] warning: watcher task panicked: {e}");
        }
    }

    Ok(())
}

// ─── Generic typed watcher ───────────────────────────────────────────────────

/// Watch all resources of type `T` across all namespaces.
///
/// Lifecycle:
/// - `Init`      → new watch cycle starting; clear the init buffer.
/// - `InitApply` → existing object; buffer it.
/// - `InitDone`  → on first init: push to global buffer and signal coordinator;
///   on reconnects: sort locally and send directly to skim.
/// - `Apply`     → live add/modify; send immediately.
/// - `Delete`    → live deletion; send with `[DELETED]` status so it's visible.
///
/// The watcher reconnects automatically on failures via `default_backoff()`.
/// The loop exits cleanly when skim closes the channel (send returns Err).
#[allow(clippy::too_many_arguments)]
async fn watch_typed<T, F>(
    client: Client,
    tx: SkimItemSender,
    kind: ResourceKind,
    status_fn: F,
    context: String,
    namespace: Option<String>,
    label_selector: Option<String>,
    global_init: Arc<Mutex<Vec<K8sItem>>>,
    done_count: Arc<AtomicUsize>,
    total_watchers: usize,
    all_init_done: Arc<Notify>,
) -> Result<()>
where
    T: Resource<DynamicType = ()> + DeserializeOwned + Clone + Send + Sync + Debug + 'static,
    F: Fn(&T) -> String,
{
    let api: Api<T> = Api::all(client);
    watch_stream(
        api,
        tx,
        kind,
        status_fn,
        context,
        namespace,
        label_selector,
        global_init,
        done_count,
        total_watchers,
        all_init_done,
    )
    .await
}

/// Watch a CRD/dynamic resource using `DynamicObject`.
#[allow(clippy::too_many_arguments)]
pub async fn watch_dynamic(
    client: Client,
    tx: SkimItemSender,
    kind: ResourceKind,
    api_resource: kube::discovery::ApiResource,
    namespaced: bool,
    status_fn: fn(&kube::api::DynamicObject) -> String,
    context: String,
    namespace: Option<String>,
    label_selector: Option<String>,
    global_init: Arc<Mutex<Vec<K8sItem>>>,
    done_count: Arc<AtomicUsize>,
    total_watchers: usize,
    all_init_done: Arc<Notify>,
) -> Result<()> {
    let api: Api<kube::api::DynamicObject> = Api::all_with(client, &api_resource);
    // For cluster-scoped CRDs, ignore the namespace filter.
    let ns = if namespaced { namespace } else { None };
    watch_stream(
        api,
        tx,
        kind,
        status_fn,
        context,
        ns,
        label_selector,
        global_init,
        done_count,
        total_watchers,
        all_init_done,
    )
    .await
}

/// Core watcher loop shared by typed and dynamic watchers.
#[allow(clippy::too_many_arguments)]
async fn watch_stream<T, F>(
    api: Api<T>,
    tx: SkimItemSender,
    kind: ResourceKind,
    status_fn: F,
    context: String,
    namespace: Option<String>,
    label_selector: Option<String>,
    global_init: Arc<Mutex<Vec<K8sItem>>>,
    done_count: Arc<AtomicUsize>,
    total_watchers: usize,
    all_init_done: Arc<Notify>,
) -> Result<()>
where
    T: Resource + DeserializeOwned + Clone + Send + Sync + Debug + 'static,
    F: Fn(&T) -> String,
{
    let mut watcher_config = match namespace.as_deref() {
        Some(ns) => watcher::Config::default().fields(&format!("metadata.namespace={ns}")),
        None => watcher::Config::default(),
    };
    if let Some(sel) = label_selector.as_deref() {
        watcher_config = watcher_config.labels(sel);
    }
    let mut stream = pin!(watcher(api, watcher_config).default_backoff());

    // Buffer for initial items so we can sort before the first render.
    let mut init_batch: Vec<K8sItem> = Vec::new();
    let mut in_init = true;
    let mut first_init_done = false;

    // Track live resources: (namespace, name) → shared state handle.
    // Updates to existing resources mutate state in-place; skim items
    // read from the same Arc on every display() call.
    let mut seen: HashMap<(String, String), Arc<RwLock<ItemState>>> = HashMap::new();
    // Keys observed during the current Init cycle, used to detect
    // resources deleted while disconnected during a reconnect.
    let mut init_keys: HashSet<(String, String)> = HashSet::new();

    while let Some(event) = stream.next().await {
        match event {
            // ── Init cycle start ──────────────────────────────────────────────
            Ok(watcher::Event::Init) => {
                init_batch.clear();
                init_keys.clear();
                in_init = true;
            }

            // ── Existing object during initial list ───────────────────────────
            Ok(watcher::Event::InitApply(r)) => {
                let ns = r.meta().namespace.clone().unwrap_or_default();
                let name = r.name_any();
                let status = status_fn(&r);
                let age = resource_age(r.meta());
                let key = (ns.clone(), name.clone());

                init_keys.insert(key.clone());

                if let Some(existing) = seen.get(&key) {
                    // Resource already tracked from a previous watch cycle —
                    // update its state in-place so the existing skim entry refreshes.
                    let mut state = existing.write().unwrap();
                    state.status = status;
                    state.age = age;
                } else {
                    let item_state = Arc::new(RwLock::new(ItemState { status, age }));
                    seen.insert(key, item_state.clone());
                    let item = K8sItem::new_live(kind.clone(), ns, name, &context, item_state);
                    if in_init {
                        init_batch.push(item);
                    } else if tx
                        .send(vec![Arc::new(item) as Arc<dyn skim::SkimItem>])
                        .is_err()
                    {
                        break;
                    }
                }
            }

            // ── Initial list complete ─────────────────────────────────────────
            Ok(watcher::Event::InitDone) => {
                // Mark resources not seen in this init cycle as deleted
                // (handles resources removed while disconnected).
                for (key, state) in &seen {
                    if !init_keys.contains(key) {
                        state.write().unwrap().status = "[DELETED]".to_string();
                    }
                }

                if first_init_done {
                    // Reconnect: sort and send only NEW items directly.
                    init_batch.sort_by_key(|item| {
                        let status = item.status();
                        std::cmp::Reverse(status_priority(&status))
                    });
                    let sorted: Vec<Arc<dyn skim::SkimItem>> = init_batch
                        .drain(..)
                        .map(|item| Arc::new(item) as Arc<dyn skim::SkimItem>)
                        .collect();
                    if !sorted.is_empty() && tx.send(sorted).is_err() {
                        break;
                    }
                } else {
                    // First init: push into the shared global buffer. The coordinator
                    // task in watch_resources will do a single globally-sorted send
                    // once all watchers (or the 8s timeout) complete.
                    {
                        let mut buf = global_init.lock().unwrap();
                        buf.extend(init_batch.drain(..));
                    }
                    let finished = done_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if finished >= total_watchers {
                        all_init_done.notify_one();
                    }
                    first_init_done = true;
                }
                in_init = false;
            }

            // ── Live add / update ─────────────────────────────────────────────
            Ok(watcher::Event::Apply(r)) => {
                let ns = r.meta().namespace.clone().unwrap_or_default();
                let name = r.name_any();
                let status = status_fn(&r);
                let age = resource_age(r.meta());
                let key = (ns.clone(), name.clone());

                if let Some(existing) = seen.get(&key) {
                    // Existing resource — update state in-place.
                    let mut state = existing.write().unwrap();
                    state.status = status;
                    state.age = age;
                } else {
                    // New resource appeared after init — send to skim.
                    let item_state = Arc::new(RwLock::new(ItemState { status, age }));
                    seen.insert(key, item_state.clone());
                    let item = K8sItem::new_live(kind.clone(), ns, name, &context, item_state);
                    if tx
                        .send(vec![Arc::new(item) as Arc<dyn skim::SkimItem>])
                        .is_err()
                    {
                        break;
                    }
                }
            }

            // ── Live deletion ─────────────────────────────────────────────────
            Ok(watcher::Event::Delete(r)) => {
                let ns = r.meta().namespace.clone().unwrap_or_default();
                let name = r.name_any();
                let key = (ns, name);

                if let Some(existing) = seen.get(&key) {
                    existing.write().unwrap().status = "[DELETED]".to_string();
                }
                // No new item sent — existing skim item updates via shared state.
            }

            // ── Watch error — default_backoff handles retry ───────────────────
            Err(e) => {
                eprintln!("[kuberift] watch error ({}): {e}", kind.as_str());
            }
        }
    }

    Ok(())
}

// ─── Status priority (lower = shown first) ───────────────────────────────────

pub fn status_priority(status: &str) -> u8 {
    crate::items::StatusHealth::classify(status).priority()
}

// ─── Per-resource status extractors ──────────────────────────────────────────

pub fn pod_status(pod: &Pod) -> String {
    if pod.metadata.deletion_timestamp.is_some() {
        return "Terminating".to_string();
    }
    let Some(status) = &pod.status else {
        return "Unknown".to_string();
    };
    // Container-level waiting reasons (CrashLoopBackOff, etc.)
    if let Some(css) = &status.container_statuses {
        for cs in css {
            if let Some(state) = &cs.state {
                if let Some(waiting) = &state.waiting {
                    if let Some(reason) = &waiting.reason {
                        // "PodInitializing" = main container waiting for init containers —
                        // skip it and let the init container block below compute Init:X/Y
                        if reason != "ContainerCreating" && reason != "PodInitializing" {
                            return reason.clone();
                        }
                    }
                }
                if let Some(terminated) = &state.terminated {
                    if terminated.exit_code != 0 {
                        return terminated
                            .reason
                            .clone()
                            .unwrap_or_else(|| "Error".to_string());
                    }
                }
            }
        }
    }
    // Init container status — check waiting reason first, then running progress
    if let Some(ics) = &status.init_container_statuses {
        // Explicit waiting reason (e.g. Init:ErrImagePull)
        for cs in ics {
            if let Some(state) = &cs.state {
                if let Some(waiting) = &state.waiting {
                    if let Some(reason) = &waiting.reason {
                        return format!("Init:{reason}");
                    }
                }
            }
        }
        // One or more init containers are running — show X/Y progress like kubectl
        let total = ics.len();
        let done = ics
            .iter()
            .filter(|cs| {
                cs.state
                    .as_ref()
                    .and_then(|s| s.terminated.as_ref())
                    .is_some_and(|t| t.exit_code == 0)
            })
            .count();
        if done < total {
            return format!("Init:{done}/{total}");
        }
    }
    status
        .phase
        .clone()
        .unwrap_or_else(|| "Unknown".to_string())
}

pub fn service_status(svc: &Service) -> String {
    svc.spec
        .as_ref()
        .and_then(|s| s.type_.as_deref())
        .unwrap_or("ClusterIP")
        .to_string()
}

pub fn deploy_status(d: &Deployment) -> String {
    let ready = d
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let desired = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
    format!("{ready}/{desired}")
}

pub fn statefulset_status(sts: &StatefulSet) -> String {
    let ready = sts
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let total = sts.status.as_ref().map_or(0, |s| s.replicas);
    format!("{ready}/{total}")
}

pub fn daemonset_status(ds: &DaemonSet) -> String {
    let ready = ds.status.as_ref().map_or(0, |s| s.number_ready);
    let desired = ds.status.as_ref().map_or(0, |s| s.desired_number_scheduled);
    format!("{ready}/{desired}")
}

pub fn secret_status(s: &Secret) -> String {
    s.type_.clone().unwrap_or_else(|| "Opaque".to_string())
}

pub fn ingress_status(ing: &Ingress) -> String {
    ing.status
        .as_ref()
        .and_then(|s| s.load_balancer.as_ref())
        .and_then(|lb| lb.ingress.as_ref())
        .and_then(|v| v.first())
        .and_then(|i| i.ip.as_deref().or(i.hostname.as_deref()))
        .unwrap_or("<pending>")
        .to_string()
}

pub fn node_status(node: &Node) -> String {
    node.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|conds| conds.iter().find(|c| c.type_ == "Ready"))
        .map_or_else(
            || "Unknown".to_string(),
            |c| {
                if c.status == "True" {
                    "Ready".to_string()
                } else {
                    "NotReady".to_string()
                }
            },
        )
}

pub fn namespace_status(ns: &Namespace) -> String {
    ns.status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Active")
        .to_string()
}

pub fn pv_status(pv: &PersistentVolume) -> String {
    pv.status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown")
        .to_string()
}

pub fn pvc_status(pvc: &PersistentVolumeClaim) -> String {
    pvc.status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown")
        .to_string()
}

pub fn job_status(job: &Job) -> String {
    let s = job.status.as_ref();
    if s.and_then(|s| s.completion_time.as_ref()).is_some() {
        return "Complete".to_string();
    }
    if let Some(failed) = s.and_then(|s| s.failed) {
        if failed > 0 {
            return format!("Failed({failed})");
        }
    }
    let active = s.and_then(|s| s.active).unwrap_or(0);
    if active > 0 {
        return format!("Active({active})");
    }
    "Unknown".to_string()
}

pub fn cronjob_status(cj: &CronJob) -> String {
    let active = cj
        .status
        .as_ref()
        .and_then(|s| s.active.as_ref())
        .map_or(0, Vec::len);
    if active > 0 {
        format!("Active({active})")
    } else {
        "Scheduled".to_string()
    }
}

// ─── Age helper ───────────────────────────────────────────────────────────────

pub fn resource_age(meta: &ObjectMeta) -> String {
    meta.creation_timestamp
        .as_ref()
        .and_then(|t| {
            Timestamp::now()
                .since(t.0)
                .ok()
                .and_then(|dur| {
                    dur.round(
                        SpanRound::new()
                            .largest(Unit::Day)
                            .days_are_24_hours()
                            .smallest(Unit::Minute),
                    )
                    .ok()
                })
                .map(
                    |dur| match (dur.get_days(), dur.get_hours(), dur.get_minutes()) {
                        (d, _, _) if d > 0 => format!("{d}d"),
                        (_, h, _) if h > 0 => format!("{h}h"),
                        (_, _, m) => format!("{m}m"),
                    },
                )
        })
        .unwrap_or_else(|| "?".to_string())
}
