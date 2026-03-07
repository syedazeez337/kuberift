use anyhow::Result;
use kube::{
    api::DynamicObject,
    discovery::{verbs, ApiResource, Discovery, Scope},
    Client,
};

/// A CRD (or other non-built-in API resource) discovered at runtime.
#[derive(Debug, Clone)]
pub struct DiscoveredCrd {
    pub api_resource: ApiResource,
    pub kind_name: String,
    pub plural: String,
    pub group: String,
    pub namespaced: bool,
}

/// Built-in resources that we handle via typed watchers.
/// Anything discovered that ISN'T in this list is treated as a CRD.
const BUILTIN_RESOURCES: &[(&str, &str)] = &[
    ("", "pods"),
    ("", "services"),
    ("", "configmaps"),
    ("", "secrets"),
    ("", "namespaces"),
    ("", "nodes"),
    ("", "persistentvolumes"),
    ("", "persistentvolumeclaims"),
    ("apps", "deployments"),
    ("apps", "statefulsets"),
    ("apps", "daemonsets"),
    ("batch", "jobs"),
    ("batch", "cronjobs"),
    ("networking.k8s.io", "ingresses"),
];

fn is_builtin(group: &str, plural: &str) -> bool {
    BUILTIN_RESOURCES
        .iter()
        .any(|(g, p)| *g == group && *p == plural)
}

/// Discover all non-built-in API resources that support LIST and WATCH.
pub async fn discover_crds(client: &Client) -> Result<Vec<DiscoveredCrd>> {
    let discovery = Discovery::new(client.clone()).run().await?;
    let mut crds = Vec::new();

    for group in discovery.groups() {
        for (ar, caps) in group.recommended_resources() {
            if !caps.supports_operation(verbs::LIST) || !caps.supports_operation(verbs::WATCH) {
                continue;
            }
            if is_builtin(&ar.group, &ar.plural) {
                continue;
            }
            crds.push(DiscoveredCrd {
                kind_name: ar.kind.clone(),
                plural: ar.plural.clone(),
                group: ar.group.clone(),
                namespaced: matches!(caps.scope, Scope::Namespaced),
                api_resource: ar,
            });
        }
    }

    Ok(crds)
}

/// Extract status from a `DynamicObject` by checking `.status.conditions[]`
/// for a condition with `type == "Ready"`, falling back to `.status.phase`.
pub fn dynamic_status(obj: &DynamicObject) -> String {
    let Some(status) = obj.data.get("status") else {
        return "Unknown".to_string();
    };

    // Check .status.conditions[] for type=Ready
    if let Some(conditions) = status.get("conditions").and_then(|c| c.as_array()) {
        for cond in conditions {
            let cond_type = cond.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if cond_type == "Ready" || cond_type == "Available" {
                let cond_status = cond
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("Unknown");
                let reason = cond.get("reason").and_then(|r| r.as_str());
                return match cond_status {
                    "True" => "Ready".to_string(),
                    "False" => reason.unwrap_or("NotReady").to_string(),
                    _ => reason.unwrap_or("Unknown").to_string(),
                };
            }
        }
    }

    // Fallback: .status.phase
    if let Some(phase) = status.get("phase").and_then(|p| p.as_str()) {
        return phase.to_string();
    }

    "Unknown".to_string()
}
