use std::path::Path;

use kuberift::config::{parse_config, CostDisplayPeriod};

#[test]
fn parse_config_uses_defaults_when_cost_section_missing() {
    let cfg = parse_config("", Path::new("config.toml"));
    assert!(!cfg.cost.enabled);
    assert_eq!(cfg.cost.display_period, CostDisplayPeriod::Daily);
    assert_eq!(cfg.cost.highlight_threshold, 10.0);
}

#[test]
fn parse_config_reads_cost_section() {
    let cfg = parse_config(
        r#"
        [cost]
        enabled = true
        opencost_endpoint = "http://127.0.0.1:9003"
        display_period = "monthly"
        highlight_threshold = 24.5
        "#,
        Path::new("config.toml"),
    );

    assert!(cfg.cost.enabled);
    assert_eq!(cfg.cost.opencost_endpoint, "http://127.0.0.1:9003");
    assert_eq!(cfg.cost.display_period, CostDisplayPeriod::Monthly);
    assert_eq!(cfg.cost.highlight_threshold, 24.5);
}
