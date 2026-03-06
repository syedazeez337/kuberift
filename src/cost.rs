use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, process::Command, sync::Arc, time::Duration};

use crate::{
    config::{CostConfig, CostDisplayPeriod},
    items::{K8sItem, ResourceKind},
};

const OPENCOST_QUERY_TIMEOUT_MS: u64 = 1500;
const OPENCOST_WINDOW: &str = "1d";
const OPENCOST_AGGREGATE: &str = "pod";

#[derive(Debug, Clone)]
pub struct CostSnapshot {
    period: CostDisplayPeriod,
    highlight_threshold_daily: f64,
    item_daily_costs: HashMap<ResourceRef, f64>,
    namespace_daily_costs: HashMap<String, f64>,
    cluster_daily_cost: f64,
}

impl CostSnapshot {
    pub fn apply(&self, item: &mut K8sItem) {
        let Some(daily_cost) = self.lookup_daily_cost(item.kind(), item.namespace(), item.name())
        else {
            item.clear_cost();
            return;
        };

        item.set_cost(
            self.render_cost(daily_cost),
            daily_cost >= self.highlight_threshold_daily,
        );
    }

    pub fn header_label(&self, namespace: Option<&str>) -> Option<String> {
        match namespace {
            Some(ns) => self
                .namespace_daily_costs
                .get(ns)
                .copied()
                .map(|daily_cost| format!("ns-cost:{}", self.render_cost(daily_cost))),
            None if self.cluster_daily_cost > 0.0 => Some(format!(
                "cluster-cost:{}",
                self.render_cost(self.cluster_daily_cost)
            )),
            None => None,
        }
    }

    fn lookup_daily_cost(&self, kind: ResourceKind, namespace: &str, name: &str) -> Option<f64> {
        if kind == ResourceKind::Namespace {
            return self.namespace_daily_costs.get(name).copied();
        }

        self.item_daily_costs
            .get(&ResourceRef::new(kind, namespace, name))
            .copied()
    }

    fn render_cost(&self, daily_cost: f64) -> String {
        match self.period {
            CostDisplayPeriod::Hourly => format!("${:.2}/h", daily_cost / 24.0),
            CostDisplayPeriod::Daily => format!("${:.2}/d", daily_cost),
            CostDisplayPeriod::Monthly => format!("${:.2}/mo", daily_cost * 30.0),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ResourceRef {
    kind: ResourceKind,
    namespace: String,
    name: String,
}

impl ResourceRef {
    fn new(kind: ResourceKind, namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind,
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct ProxyTarget {
    namespace: String,
    service: String,
    port_name: Option<String>,
    port: i64,
}

impl ProxyTarget {
    fn raw_path(&self) -> String {
        let svc_port = self.port_name.as_deref().map_or_else(
            || format!("{}:{}", self.service, self.port),
            |name| format!("{}:{}", self.service, name),
        );

        format!(
            "/api/v1/namespaces/{}/services/{svc_port}/proxy/allocation/compute?window={OPENCOST_WINDOW}&aggregate={OPENCOST_AGGREGATE}",
            self.namespace,
        )
    }
}

#[derive(Debug, Clone)]
struct PodOwnerIndex {
    controllers: HashMap<(String, String), ResourceRef>,
}

impl PodOwnerIndex {
    fn controller_for(&self, namespace: &str, pod: &str) -> Option<&ResourceRef> {
        self.controllers
            .get(&(namespace.to_string(), pod.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct ServiceList {
    items: Vec<ServiceItem>,
}

#[derive(Debug, Deserialize)]
struct ServiceItem {
    metadata: Metadata,
    spec: Option<ServiceSpec>,
}

#[derive(Debug, Deserialize)]
struct ServiceSpec {
    ports: Option<Vec<ServicePort>>,
}

#[derive(Debug, Deserialize)]
struct ServicePort {
    name: Option<String>,
    port: i64,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    name: Option<String>,
    namespace: Option<String>,
    #[serde(rename = "ownerReferences")]
    owner_references: Option<Vec<OwnerReference>>,
}

#[derive(Debug, Clone, Deserialize)]
struct OwnerReference {
    kind: String,
    name: String,
    controller: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct PodList {
    items: Vec<PodItem>,
}

#[derive(Debug, Deserialize)]
struct PodItem {
    metadata: Metadata,
}

#[derive(Debug, Deserialize)]
struct JobList {
    items: Vec<JobItem>,
}

#[derive(Debug, Deserialize)]
struct JobItem {
    metadata: Metadata,
}

#[derive(Debug, Deserialize)]
struct AllocationResponse {
    data: Vec<HashMap<String, AllocationEntry>>,
}

#[derive(Debug, Deserialize)]
struct AllocationEntry {
    name: Option<String>,
    #[serde(rename = "totalCost")]
    total_cost: Option<f64>,
    properties: Option<AllocationProperties>,
}

#[derive(Debug, Deserialize)]
struct AllocationProperties {
    namespace: Option<String>,
    pod: Option<String>,
    controller: Option<String>,
    #[serde(rename = "controllerKind")]
    controller_kind: Option<String>,
}

pub async fn prefetch_cost_snapshot(
    context: &str,
    kubeconfig: Option<&str>,
    cost_config: &CostConfig,
) -> Option<Arc<CostSnapshot>> {
    if !cost_config.enabled {
        return None;
    }

    match tokio::time::timeout(
        Duration::from_millis(OPENCOST_QUERY_TIMEOUT_MS),
        fetch_snapshot(context, kubeconfig, cost_config),
    )
    .await
    {
        Ok(Ok(snapshot)) => Some(Arc::new(snapshot)),
        Ok(Err(err)) => {
            eprintln!("[kuberift] warning: OpenCost disabled for this session: {err}");
            None
        }
        Err(_) => {
            eprintln!(
                "[kuberift] warning: OpenCost query timed out; disabling cost for this session"
            );
            None
        }
    }
}

async fn fetch_snapshot(
    context: &str,
    kubeconfig: Option<&str>,
    cost_config: &CostConfig,
) -> Result<CostSnapshot> {
    let allocations_json = if cost_config.opencost_endpoint.trim().is_empty() {
        let services_json =
            run_kubectl_json(context, kubeconfig, ["get", "svc", "-A", "-o", "json"]).await?;
        let proxy_target = detect_proxy_target(&services_json)
            .ok_or_else(|| anyhow!("OpenCost service not found"))?;
        run_kubectl_json(
            context,
            kubeconfig,
            ["get", "--raw", &proxy_target.raw_path()],
        )
        .await?
    } else {
        fetch_direct_allocation_json(&cost_config.opencost_endpoint).await?
    };

    let pods_json_fut = run_kubectl_json(context, kubeconfig, ["get", "pods", "-A", "-o", "json"]);
    let jobs_json_fut = run_kubectl_json(context, kubeconfig, ["get", "jobs", "-A", "-o", "json"]);
    let (pods_json, jobs_json) = tokio::join!(pods_json_fut, jobs_json_fut);

    build_snapshot_from_inputs(
        &allocations_json,
        pods_json.ok().as_deref(),
        jobs_json.ok().as_deref(),
        cost_config.display_period,
        cost_config.highlight_threshold,
    )
}

async fn fetch_direct_allocation_json(endpoint: &str) -> Result<String> {
    let base = endpoint.trim_end_matches('/');
    let url = if base.contains("/allocation/compute") {
        base.to_string()
    } else {
        format!("{base}/allocation/compute?window={OPENCOST_WINDOW}&aggregate={OPENCOST_AGGREGATE}")
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(OPENCOST_QUERY_TIMEOUT_MS))
        .build()
        .context("failed to build HTTP client")?;

    client
        .get(url)
        .send()
        .await
        .context("failed to reach configured OpenCost endpoint")?
        .error_for_status()
        .context("OpenCost endpoint returned an error status")?
        .text()
        .await
        .context("failed to read OpenCost response body")
}

async fn run_kubectl_json<I, S>(context: &str, kubeconfig: Option<&str>, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let context = context.to_string();
    let kubeconfig = kubeconfig.map(str::to_string);
    let args_vec: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect();

    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("kubectl");
        if let Some(path) = kubeconfig.as_deref() {
            cmd.arg("--kubeconfig").arg(path);
        }
        if !context.is_empty() {
            cmd.arg("--context").arg(&context);
        }
        cmd.args(&args_vec);

        let output = cmd.output().context("failed to execute kubectl")?;
        if !output.status.success() {
            return Err(anyhow!(
                "{}",
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            ));
        }

        String::from_utf8(output.stdout).context("kubectl output was not valid UTF-8")
    })
    .await
    .context("kubectl task join failed")?
}

fn detect_proxy_target(raw: &str) -> Option<ProxyTarget> {
    let services: ServiceList = serde_json::from_str(raw).ok()?;
    let candidates = [("opencost", "opencost"), ("opencost", ""), ("kubecost", "")];

    for (name_match, namespace_match) in candidates {
        for svc in &services.items {
            let name = svc.metadata.name.as_deref()?;
            let namespace = svc.metadata.namespace.as_deref()?;
            if name == "kubernetes" {
                continue;
            }
            if (!namespace_match.is_empty() && namespace != namespace_match)
                || !name.contains(name_match)
            {
                continue;
            }

            let port = pick_service_port(svc.spec.as_ref()?.ports.as_ref()?)?;
            return Some(ProxyTarget {
                namespace: namespace.to_string(),
                service: name.to_string(),
                port_name: port.name.clone(),
                port: port.port,
            });
        }
    }

    None
}

fn pick_service_port<'a>(ports: &'a [ServicePort]) -> Option<&'a ServicePort> {
    ports
        .iter()
        .find(|port| {
            port.name
                .as_deref()
                .is_some_and(|name| name.contains("http"))
        })
        .or_else(|| ports.iter().find(|port| port.port == 9003))
        .or_else(|| ports.first())
}

fn build_pod_owner_index(pods_raw: Option<&str>, jobs_raw: Option<&str>) -> Result<PodOwnerIndex> {
    let mut job_to_cronjob: HashMap<(String, String), String> = HashMap::new();

    if let Some(raw) = jobs_raw {
        let jobs: JobList = serde_json::from_str(raw).context("failed to parse jobs JSON")?;
        for job in jobs.items {
            let namespace = job.metadata.namespace.unwrap_or_default();
            let Some(owner) = controller_owner(job.metadata.owner_references.as_deref()) else {
                continue;
            };
            if owner.kind == "CronJob" {
                job_to_cronjob.insert(
                    (namespace, job.metadata.name.unwrap_or_default()),
                    owner.name,
                );
            }
        }
    }

    let mut controllers = HashMap::new();
    if let Some(raw) = pods_raw {
        let pods: PodList = serde_json::from_str(raw).context("failed to parse pods JSON")?;
        for pod in pods.items {
            let namespace = pod.metadata.namespace.unwrap_or_default();
            let pod_name = pod.metadata.name.unwrap_or_default();
            let Some(owner) = controller_owner(pod.metadata.owner_references.as_deref()) else {
                continue;
            };
            let Some(target) = resource_ref_for_owner(&namespace, &owner, &job_to_cronjob) else {
                continue;
            };
            controllers.insert((namespace, pod_name), target);
        }
    }

    Ok(PodOwnerIndex { controllers })
}

fn controller_owner(owners: Option<&[OwnerReference]>) -> Option<OwnerReference> {
    owners?
        .iter()
        .find(|owner| owner.controller.unwrap_or(false))
        .cloned()
}

fn resource_ref_for_owner(
    namespace: &str,
    owner: &OwnerReference,
    job_to_cronjob: &HashMap<(String, String), String>,
) -> Option<ResourceRef> {
    match owner.kind.as_str() {
        "ReplicaSet" => infer_deployment_name(&owner.name)
            .map(|deployment| ResourceRef::new(ResourceKind::Deployment, namespace, deployment)),
        "Deployment" => Some(ResourceRef::new(
            ResourceKind::Deployment,
            namespace,
            &owner.name,
        )),
        "StatefulSet" => Some(ResourceRef::new(
            ResourceKind::StatefulSet,
            namespace,
            &owner.name,
        )),
        "DaemonSet" => Some(ResourceRef::new(
            ResourceKind::DaemonSet,
            namespace,
            &owner.name,
        )),
        "Job" => job_to_cronjob
            .get(&(namespace.to_string(), owner.name.clone()))
            .map(|cronjob| ResourceRef::new(ResourceKind::CronJob, namespace, cronjob))
            .or_else(|| Some(ResourceRef::new(ResourceKind::Job, namespace, &owner.name))),
        "CronJob" => Some(ResourceRef::new(
            ResourceKind::CronJob,
            namespace,
            &owner.name,
        )),
        _ => None,
    }
}

fn infer_deployment_name(replica_set_name: &str) -> Option<String> {
    let (deployment, hash) = replica_set_name.rsplit_once('-')?;
    if hash.len() >= 6 && hash.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        Some(deployment.to_string())
    } else {
        None
    }
}

pub fn build_snapshot_from_inputs(
    allocations_raw: &str,
    pods_raw: Option<&str>,
    jobs_raw: Option<&str>,
    period: CostDisplayPeriod,
    highlight_threshold: f64,
) -> Result<CostSnapshot> {
    let response: AllocationResponse =
        serde_json::from_str(allocations_raw).context("failed to parse OpenCost response")?;
    let pod_owner_index = build_pod_owner_index(pods_raw, jobs_raw)?;

    let mut item_daily_costs = HashMap::new();
    let mut namespace_daily_costs = HashMap::new();
    let mut cluster_daily_cost = 0.0;

    for bucket in response.data {
        for (key, entry) in bucket {
            let Some(daily_cost) = entry.total_cost else {
                continue;
            };
            if daily_cost <= 0.0 {
                continue;
            }

            let Some(namespace) = allocation_namespace(&key, &entry) else {
                continue;
            };
            if namespace == "__idle__" {
                continue;
            }

            let Some(pod_name) = allocation_pod_name(&key, &entry) else {
                continue;
            };

            cluster_daily_cost += daily_cost;
            *namespace_daily_costs
                .entry(namespace.clone())
                .or_insert(0.0) += daily_cost;
            *item_daily_costs
                .entry(ResourceRef::new(ResourceKind::Pod, &namespace, &pod_name))
                .or_insert(0.0) += daily_cost;

            if let Some(controller) =
                allocation_controller_ref(&namespace, &entry, &pod_owner_index, &pod_name)
            {
                *item_daily_costs.entry(controller).or_insert(0.0) += daily_cost;
            }
        }
    }

    Ok(CostSnapshot {
        period,
        highlight_threshold_daily: highlight_threshold,
        item_daily_costs,
        namespace_daily_costs,
        cluster_daily_cost,
    })
}

fn allocation_namespace(key: &str, entry: &AllocationEntry) -> Option<String> {
    entry
        .properties
        .as_ref()
        .and_then(|props| props.namespace.clone())
        .or_else(|| key.split('/').nth_back(1).map(ToOwned::to_owned))
}

fn allocation_pod_name(key: &str, entry: &AllocationEntry) -> Option<String> {
    entry
        .properties
        .as_ref()
        .and_then(|props| props.pod.clone())
        .or_else(|| entry.name.clone())
        .or_else(|| key.rsplit('/').next().map(ToOwned::to_owned))
        .filter(|name| name != "__idle__")
}

fn allocation_controller_ref(
    namespace: &str,
    entry: &AllocationEntry,
    pod_owner_index: &PodOwnerIndex,
    pod_name: &str,
) -> Option<ResourceRef> {
    if let Some(props) = entry.properties.as_ref() {
        if let (Some(controller), Some(kind)) = (&props.controller, &props.controller_kind) {
            let kind = match kind.as_str() {
                "deployment" | "Deployment" => ResourceKind::Deployment,
                "statefulset" | "StatefulSet" => ResourceKind::StatefulSet,
                "daemonset" | "DaemonSet" => ResourceKind::DaemonSet,
                "job" | "Job" => ResourceKind::Job,
                "cronjob" | "CronJob" => ResourceKind::CronJob,
                _ => return None,
            };

            return Some(ResourceRef::new(kind, namespace, controller));
        }
    }

    pod_owner_index.controller_for(namespace, pod_name).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_proxy_target_prefers_named_http_port() {
        let services = r#"
        {
          "items": [
            {
              "metadata": { "name": "opencost", "namespace": "opencost" },
              "spec": {
                "ports": [
                  { "name": "http", "port": 9003 },
                  { "name": "metrics", "port": 9004 }
                ]
              }
            }
          ]
        }"#;

        let target = detect_proxy_target(services).expect("expected target");
        assert_eq!(target.namespace, "opencost");
        assert_eq!(target.service, "opencost");
        assert_eq!(target.port_name.as_deref(), Some("http"));
        assert_eq!(
            target.raw_path(),
            "/api/v1/namespaces/opencost/services/opencost:http/proxy/allocation/compute?window=1d&aggregate=pod"
        );
    }

    #[test]
    fn build_snapshot_maps_pod_deployment_and_namespace_totals() {
        let allocations = r#"
        {
          "data": [
            {
              "cluster-one/default/api-7d9f8b6c5-xk2lp": {
                "name": "api-7d9f8b6c5-xk2lp",
                "properties": {
                  "namespace": "default",
                  "pod": "api-7d9f8b6c5-xk2lp"
                },
                "totalCost": 2.5
              },
              "cluster-one/default/api-7d9f8b6c5-jm4rn": {
                "name": "api-7d9f8b6c5-jm4rn",
                "properties": {
                  "namespace": "default",
                  "pod": "api-7d9f8b6c5-jm4rn"
                },
                "totalCost": 3.5
              },
              "__idle__": {
                "name": "__idle__",
                "properties": {
                  "namespace": "__idle__"
                },
                "totalCost": 99.0
              }
            }
          ]
        }"#;
        let pods = r#"
        {
          "items": [
            {
              "metadata": {
                "name": "api-7d9f8b6c5-xk2lp",
                "namespace": "default",
                "ownerReferences": [
                  { "kind": "ReplicaSet", "name": "api-7d9f8b6c5", "controller": true }
                ]
              }
            },
            {
              "metadata": {
                "name": "api-7d9f8b6c5-jm4rn",
                "namespace": "default",
                "ownerReferences": [
                  { "kind": "ReplicaSet", "name": "api-7d9f8b6c5", "controller": true }
                ]
              }
            }
          ]
        }"#;

        let snapshot = build_snapshot_from_inputs(
            allocations,
            Some(pods),
            None,
            CostDisplayPeriod::Monthly,
            4.0,
        )
        .expect("snapshot");

        let mut pod = K8sItem::new(
            ResourceKind::Pod,
            "default",
            "api-7d9f8b6c5-xk2lp",
            "Running",
            "1d",
            "",
        );
        snapshot.apply(&mut pod);
        assert_eq!(pod.cost_label(), Some("$75.00/mo"));

        let mut deployment =
            K8sItem::new(ResourceKind::Deployment, "default", "api", "2/2", "1d", "");
        snapshot.apply(&mut deployment);
        assert_eq!(deployment.cost_label(), Some("$180.00/mo"));

        let mut namespace =
            K8sItem::new(ResourceKind::Namespace, "", "default", "Active", "1d", "");
        snapshot.apply(&mut namespace);
        assert_eq!(namespace.cost_label(), Some("$180.00/mo"));
        assert_eq!(
            snapshot.header_label(Some("default")).as_deref(),
            Some("ns-cost:$180.00/mo")
        );
    }
}
