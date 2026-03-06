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
    fmt::Debug,
    pin::pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

use crate::cost::CostSnapshot;
use crate::items::{K8sItem, ResourceKind};

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
    context: &str,
    namespace: Option<&str>,
    label_selector: Option<&str>,
    cost_snapshot: Option<Arc<CostSnapshot>>,
) -> Result<()> {
    let total_watchers = kinds.len();
    let global_init: Arc<Mutex<Vec<K8sItem>>> = Arc::new(Mutex::new(Vec::new()));
    let done_count: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let all_init_done: Arc<Notify> = Arc::new(Notify::new());

    // Coordinator task: waits for all watchers to finish initial list (or 8s timeout),
    // then globally sorts and sends the full initial batch to skim.
    {
        let global_init = global_init.clone();
        let tx_coord = tx.clone();
        let all_init_done = all_init_done.clone();
        let cost_snapshot = cost_snapshot.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = all_init_done.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(8)) => {}
            }
            let mut buf = global_init.lock().unwrap();
            if let Some(snapshot) = cost_snapshot.as_deref() {
                for item in buf.iter_mut() {
                    snapshot.apply(item);
                }
            }
            buf.sort_by_key(|item| std::cmp::Reverse(status_priority(item.status())));
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
        let k = *kind;
        let ctx = context.to_string();
        let ns = namespace.map(str::to_string);
        let ls = label_selector.map(str::to_string);
        let gi = global_init.clone();
        let dc = done_count.clone();
        let aid = all_init_done.clone();
        let cost_state = cost_snapshot.clone();

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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
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
                        cost_state,
                    )
                    .await
                }
            };

            if let Err(e) = result {
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
    cost_snapshot: Option<Arc<CostSnapshot>>,
) -> Result<()>
where
    T: Resource<DynamicType = ()> + DeserializeOwned + Clone + Send + Sync + Debug + 'static,
    F: Fn(&T) -> String,
{
    let api: Api<T> = Api::all(client);
    let mut watcher_config = match namespace.as_deref() {
        Some(ns) => watcher::Config::default().fields(&format!("metadata.namespace={ns}")),
        None => watcher::Config::default(),
    };
    if let Some(sel) = label_selector.as_deref() {
        watcher_config = watcher_config.labels(sel);
    }
    let mut stream = pin!(watcher(api, watcher_config).default_backoff());

    // Buffer for initial items so we can sort before the first render.
    // Stored as K8sItem (concrete type) so we can sort without downcast.
    let mut init_batch: Vec<K8sItem> = Vec::new();
    let mut in_init = true;
    // Track whether this watcher has already contributed to the global init batch.
    // On reconnects we sort locally and send directly rather than touching global state.
    let mut first_init_done = false;

    while let Some(event) = stream.next().await {
        match event {
            // ── Init cycle start ──────────────────────────────────────────────
            Ok(watcher::Event::Init) => {
                init_batch.clear();
                in_init = true;
            }

            // ── Existing object during initial list ───────────────────────────
            Ok(watcher::Event::InitApply(r)) => {
                let item = make_item(
                    &r,
                    kind,
                    &status_fn,
                    false,
                    &context,
                    cost_snapshot.as_deref(),
                );
                if in_init {
                    init_batch.push(item);
                } else {
                    // Shouldn't occur, but handle gracefully.
                    if tx
                        .send(vec![Arc::new(item) as Arc<dyn skim::SkimItem>])
                        .is_err()
                    {
                        break;
                    }
                }
            }

            // ── Initial list complete ─────────────────────────────────────────
            Ok(watcher::Event::InitDone) => {
                if first_init_done {
                    // Reconnect after a watch error: sort this watcher's batch locally
                    // and send directly — global coordination already happened.
                    // Skim renders higher-indexed items at the TOP of the list.
                    // Sending unhealthy items LAST gives them the highest indices.
                    init_batch
                        .sort_by_key(|item| std::cmp::Reverse(status_priority(item.status())));
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
                let item = make_item(
                    &r,
                    kind,
                    &status_fn,
                    false,
                    &context,
                    cost_snapshot.as_deref(),
                );
                if tx
                    .send(vec![Arc::new(item) as Arc<dyn skim::SkimItem>])
                    .is_err()
                {
                    break;
                }
            }

            // ── Live deletion ─────────────────────────────────────────────────
            Ok(watcher::Event::Delete(r)) => {
                let item = make_item(
                    &r,
                    kind,
                    &status_fn,
                    true,
                    &context,
                    cost_snapshot.as_deref(),
                );
                if tx
                    .send(vec![Arc::new(item) as Arc<dyn skim::SkimItem>])
                    .is_err()
                {
                    break;
                }
            }

            // ── Watch error — default_backoff handles retry ───────────────────
            Err(e) => {
                eprintln!("[kuberift] watch error ({}): {e}", kind.as_str());
            }
        }
    }

    Ok(())
}

// ─── Item constructor helper ─────────────────────────────────────────────────

fn make_item<T>(
    r: &T,
    kind: ResourceKind,
    status_fn: &impl Fn(&T) -> String,
    deleted: bool,
    context: &str,
    cost_snapshot: Option<&CostSnapshot>,
) -> K8sItem
where
    T: Resource<DynamicType = ()>,
{
    let ns = r.meta().namespace.clone().unwrap_or_default();
    let name = r.name_any();
    let status = if deleted {
        "[DELETED]".to_string()
    } else {
        status_fn(r)
    };
    let age = resource_age(r.meta());
    let mut item = K8sItem::new(kind, ns, name, status, age, context);
    if let Some(snapshot) = cost_snapshot {
        snapshot.apply(&mut item);
    }
    item
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
