//! Tests for kuberift::items — StatusHealth, ResourceKind, K8sItem, and helpers.

use kuberift::items::{context_color, truncate_name, K8sItem, ResourceKind, StatusHealth};
use ratatui::style::Color;

// ── Helper ────────────────────────────────────────────────────────────────────

fn pod(status: &str) -> K8sItem {
    K8sItem::new(ResourceKind::Pod, "default", "test-pod", status, "1d", "")
}

// ── ResourceKind::as_str ──────────────────────────────────────────────────────

#[test]
fn kind_as_str_all_variants() {
    assert_eq!(ResourceKind::Pod.as_str(), "pod");
    assert_eq!(ResourceKind::Service.as_str(), "svc");
    assert_eq!(ResourceKind::Deployment.as_str(), "deploy");
    assert_eq!(ResourceKind::StatefulSet.as_str(), "sts");
    assert_eq!(ResourceKind::DaemonSet.as_str(), "ds");
    assert_eq!(ResourceKind::ConfigMap.as_str(), "cm");
    assert_eq!(ResourceKind::Secret.as_str(), "secret");
    assert_eq!(ResourceKind::Ingress.as_str(), "ing");
    assert_eq!(ResourceKind::Node.as_str(), "node");
    assert_eq!(ResourceKind::Namespace.as_str(), "ns");
    assert_eq!(ResourceKind::PersistentVolumeClaim.as_str(), "pvc");
    assert_eq!(ResourceKind::Job.as_str(), "job");
    assert_eq!(ResourceKind::CronJob.as_str(), "cronjob");
}

// ── ResourceKind::color ───────────────────────────────────────────────────────

#[test]
fn kind_color_all_variants() {
    assert_eq!(ResourceKind::Pod.color(), Color::Green);
    assert_eq!(ResourceKind::Service.color(), Color::Blue);
    assert_eq!(ResourceKind::Deployment.color(), Color::Yellow);
    assert_eq!(ResourceKind::StatefulSet.color(), Color::Yellow);
    assert_eq!(ResourceKind::DaemonSet.color(), Color::Yellow);
    assert_eq!(ResourceKind::ConfigMap.color(), Color::Magenta);
    assert_eq!(ResourceKind::Secret.color(), Color::Magenta);
    assert_eq!(ResourceKind::Ingress.color(), Color::Cyan);
    assert_eq!(ResourceKind::Node.color(), Color::White);
    assert_eq!(ResourceKind::Namespace.color(), Color::White);
    assert_eq!(
        ResourceKind::PersistentVolumeClaim.color(),
        Color::LightMagenta
    );
    assert_eq!(ResourceKind::Job.color(), Color::LightBlue);
    assert_eq!(ResourceKind::CronJob.color(), Color::LightBlue);
}

// ── ResourceKind::Display ─────────────────────────────────────────────────────

#[test]
fn kind_display_matches_as_str_for_all_variants() {
    let kinds = [
        ResourceKind::Pod,
        ResourceKind::Service,
        ResourceKind::Deployment,
        ResourceKind::StatefulSet,
        ResourceKind::DaemonSet,
        ResourceKind::ConfigMap,
        ResourceKind::Secret,
        ResourceKind::Ingress,
        ResourceKind::Node,
        ResourceKind::Namespace,
        ResourceKind::PersistentVolumeClaim,
        ResourceKind::Job,
        ResourceKind::CronJob,
    ];
    for kind in &kinds {
        assert_eq!(
            format!("{kind}"),
            kind.as_str(),
            "Display != as_str for {kind:?}"
        );
    }
}

// ── StatusHealth::classify — critical exact ───────────────────────────────────

#[test]
fn classify_critical_exact_matches() {
    let cases = [
        "Failed",
        "Error",
        "OOMKilled",
        "NotReady",
        "Lost",
        "Evicted",
        "BackOff",
    ];
    for s in &cases {
        assert_eq!(
            StatusHealth::classify(s),
            StatusHealth::Critical,
            "'{s}' should be Critical"
        );
    }
}

// ── StatusHealth::classify — critical prefix ──────────────────────────────────

#[test]
fn classify_critical_prefix_matches() {
    let cases = [
        "CrashLoopBackOff",
        "CrashLoop",
        "ErrImagePull",
        "ErrImageNeverPull",
        "ImagePullBackOff",
        "Init:Error",
        "Init:ErrImagePull",
        "Init:ImagePullBackOff",
        "Failed(3)",
        "Failed(0)",
    ];
    for s in &cases {
        assert_eq!(
            StatusHealth::classify(s),
            StatusHealth::Critical,
            "'{s}' should be Critical"
        );
    }
}

// ── StatusHealth::classify — warning ─────────────────────────────────────────

#[test]
fn classify_warning_statuses() {
    let cases = [
        "Pending",
        "Terminating",
        "ContainerCreating",
        "Unknown",
        "Init:0/1",
        "Init:1/3",
        "Init:2/3",
    ];
    for s in &cases {
        assert_eq!(
            StatusHealth::classify(s),
            StatusHealth::Warning,
            "'{s}' should be Warning"
        );
    }
}

// ── StatusHealth::classify — deleted ─────────────────────────────────────────

#[test]
fn classify_deleted_is_unknown() {
    assert_eq!(StatusHealth::classify("[DELETED]"), StatusHealth::Unknown);
}

// ── StatusHealth::classify — healthy exact ────────────────────────────────────

#[test]
fn classify_healthy_exact_matches() {
    let cases = [
        "Running",
        "Active",
        "Bound",
        "Complete",
        "Succeeded",
        "Ready",
        "Scheduled",
        "ClusterIP",
        "NodePort",
        "LoadBalancer",
    ];
    for s in &cases {
        assert_eq!(
            StatusHealth::classify(s),
            StatusHealth::Healthy,
            "'{s}' should be Healthy"
        );
    }
}

// ── StatusHealth::classify — ratio ───────────────────────────────────────────

#[test]
fn classify_ratio_equal_is_healthy() {
    assert_eq!(StatusHealth::classify("3/3"), StatusHealth::Healthy);
    assert_eq!(StatusHealth::classify("1/1"), StatusHealth::Healthy);
    assert_eq!(StatusHealth::classify("0/0"), StatusHealth::Healthy);
}

#[test]
fn classify_ratio_unequal_is_warning() {
    assert_eq!(StatusHealth::classify("0/3"), StatusHealth::Warning);
    assert_eq!(StatusHealth::classify("1/3"), StatusHealth::Warning);
    assert_eq!(StatusHealth::classify("2/3"), StatusHealth::Warning);
}

// ── StatusHealth::classify — Active prefix ────────────────────────────────────

#[test]
fn classify_active_prefix_is_healthy() {
    assert_eq!(StatusHealth::classify("Active(2)"), StatusHealth::Healthy);
    assert_eq!(StatusHealth::classify("Active(0)"), StatusHealth::Healthy);
}

// ── StatusHealth::classify — unknown default ──────────────────────────────────

#[test]
fn classify_unknown_status_defaults_to_healthy() {
    assert_eq!(
        StatusHealth::classify("SomeRandomStatus"),
        StatusHealth::Healthy
    );
    assert_eq!(StatusHealth::classify(""), StatusHealth::Healthy);
}

// ── StatusHealth::color ───────────────────────────────────────────────────────

#[test]
fn status_health_colors_all_variants() {
    assert_eq!(StatusHealth::Critical.color(), Color::Red);
    assert_eq!(StatusHealth::Warning.color(), Color::Yellow);
    assert_eq!(StatusHealth::Healthy.color(), Color::Green);
    assert_eq!(StatusHealth::Unknown.color(), Color::DarkGray);
}

// ── StatusHealth::priority ────────────────────────────────────────────────────

#[test]
fn status_health_priorities_all_variants() {
    assert_eq!(StatusHealth::Critical.priority(), 0);
    assert_eq!(StatusHealth::Warning.priority(), 1);
    assert_eq!(StatusHealth::Unknown.priority(), 1); // same tier as Warning
    assert_eq!(StatusHealth::Healthy.priority(), 2);
}

// ── BUG-003 regression: Terminating stays in warning tier ────────────────────

#[test]
fn terminating_is_warning_not_critical() {
    assert_eq!(StatusHealth::classify("Terminating"), StatusHealth::Warning);
    assert_eq!(StatusHealth::Warning.priority(), 1);
    assert_eq!(StatusHealth::Warning.color(), Color::Yellow);
}

// ── K8sItem getters ───────────────────────────────────────────────────────────

#[test]
fn k8s_item_getters_round_trip() {
    let item = K8sItem::new(
        ResourceKind::Service,
        "kube-system",
        "kube-dns",
        "ClusterIP",
        "30d",
        "prod",
    );
    assert_eq!(item.kind(), ResourceKind::Service);
    assert_eq!(item.namespace(), "kube-system");
    assert_eq!(item.name(), "kube-dns");
    assert_eq!(item.status(), "ClusterIP");
    assert_eq!(item.context(), "prod");
}

#[test]
fn k8s_item_empty_namespace_and_context() {
    let item = K8sItem::new(ResourceKind::Node, "", "node-1", "Ready", "?", "");
    assert_eq!(item.namespace(), "");
    assert_eq!(item.context(), "");
}

#[test]
fn k8s_item_cost_round_trip() {
    let mut item = K8sItem::new(ResourceKind::Deployment, "default", "api", "2/2", "1d", "");
    assert_eq!(item.cost_label(), None);
    item.set_cost("$12.00/d", true);
    assert_eq!(item.cost_label(), Some("$12.00/d"));
    item.clear_cost();
    assert_eq!(item.cost_label(), None);
}

// ── K8sItem::status_color ─────────────────────────────────────────────────────

#[test]
fn status_color_running_is_green() {
    assert_eq!(pod("Running").status_color(), Color::Green);
}

#[test]
fn status_color_crashloop_is_red() {
    assert_eq!(pod("CrashLoopBackOff").status_color(), Color::Red);
}

#[test]
fn status_color_pending_is_yellow() {
    assert_eq!(pod("Pending").status_color(), Color::Yellow);
}

#[test]
fn status_color_deleted_is_dark_gray() {
    assert_eq!(pod("[DELETED]").status_color(), Color::DarkGray);
}

// ── K8sItem::output_str ───────────────────────────────────────────────────────

#[test]
fn output_str_namespaced_single_cluster() {
    let item = K8sItem::new(ResourceKind::Pod, "default", "nginx", "Running", "1d", "");
    assert_eq!(item.output_str(), "pod/default/nginx");
}

#[test]
fn output_str_cluster_scoped_no_namespace() {
    let item = K8sItem::new(ResourceKind::Node, "", "node-1", "Ready", "7d", "");
    assert_eq!(item.output_str(), "node/node-1");
}

#[test]
fn output_str_multi_cluster_with_namespace() {
    let item = K8sItem::new(ResourceKind::Pod, "ns", "p", "Running", "1d", "prod");
    assert_eq!(item.output_str(), "prod:pod/ns/p");
}

#[test]
fn output_str_multi_cluster_no_namespace() {
    let item = K8sItem::new(
        ResourceKind::Namespace,
        "",
        "default",
        "Active",
        "30d",
        "prod",
    );
    assert_eq!(item.output_str(), "prod:ns/default");
}

// ── truncate_name ─────────────────────────────────────────────────────────────

#[test]
fn truncate_short_name_returned_as_is() {
    assert_eq!(truncate_name("nginx", 31).as_ref(), "nginx");
}

#[test]
fn truncate_name_at_exact_boundary_unchanged() {
    let name = "a".repeat(31);
    assert_eq!(truncate_name(&name, 31).as_ref(), name.as_str());
}

#[test]
fn truncate_long_name_appends_ellipsis() {
    let name = "a".repeat(40);
    let result = truncate_name(&name, 31);
    assert!(result.contains('…'), "expected ellipsis in truncated name");
    assert!(
        result.len() <= 31 + '…'.len_utf8(),
        "truncated name exceeds max_chars + ellipsis byte length"
    );
}

#[test]
fn truncate_empty_string_unchanged() {
    assert_eq!(truncate_name("", 31).as_ref(), "");
}

#[test]
fn truncate_handles_multibyte_utf8_boundary() {
    // "é" is 2 bytes; splitting at byte 31 would land mid-char without the boundary walk
    let name = format!("{}é{}", "a".repeat(30), "suffix");
    let result = truncate_name(&name, 31);
    assert!(
        std::str::from_utf8(result.as_bytes()).is_ok(),
        "truncated result must be valid UTF-8"
    );
}

// ── context_color ─────────────────────────────────────────────────────────────

#[test]
fn context_color_is_deterministic() {
    assert_eq!(context_color("prod"), context_color("prod"));
    assert_eq!(context_color("staging"), context_color("staging"));
}

#[test]
fn context_color_empty_string_does_not_panic() {
    let _ = context_color("");
}

#[test]
fn context_color_various_names_do_not_panic() {
    for ctx in &[
        "prod",
        "staging",
        "dev",
        "k8s-cluster-1",
        "my-prod-eu-west-1",
    ] {
        let _ = context_color(ctx);
    }
}
