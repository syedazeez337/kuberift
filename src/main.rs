// RST-008: clippy lint configuration
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions, // K8sItem, K8sClient etc. are fine
    clippy::too_many_lines,           // watch_resources match arm is inherently long
)]

use anyhow::Result;
// RST-007: imports moved to module top
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use crossterm::event::{KeyCode, KeyModifiers};
use kuberift::actions::{
    action_delete, action_describe, action_exec, action_logs, action_portforward,
    action_rollout_restart, action_yaml, install_preview_toggle, preview_toggle_path, runtime_dir,
};
use kuberift::cli::Args;
use kuberift::config::{load_config, AppConfig};
use kuberift::cost::prefetch_cost_snapshot;
use kuberift::items::{K8sItem, ResourceKind};
#[allow(unused_imports)]
use kuberift::k8s::{
    client::{
        build_client_for_context, current_context, list_contexts, load_last_context,
        save_last_context,
    },
    resources::{watch_resources, ALL_KINDS},
};
use skim::prelude::*;
use std::{borrow::Cow, sync::Arc};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Shell completions — print and exit before any cluster I/O.
    if let Some(shell) = args.completions {
        let mut cmd = Args::command();
        generate(shell, &mut cmd, "kf", &mut std::io::stdout());
        return Ok(());
    }

    // Man page — print and exit before any cluster I/O.
    if args.mangen {
        let cmd = Args::command();
        let man = clap_mangen::Man::new(cmd);
        man.render(&mut std::io::stdout())?;
        return Ok(());
    }

    // Warn early if kubectl is missing — preview and all actions will fail without it.
    if std::process::Command::new("kubectl")
        .args(["version", "--client"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        eprintln!(
            "[kuberift] warning: kubectl not found in PATH.\n\
             Preview and all actions (logs, exec, delete, …) will be unavailable.\n\
             Install kubectl: https://kubernetes.io/docs/tasks/tools/"
        );
    }

    // Write the preview-toggle shell script and reset mode to 0 (describe)
    install_preview_toggle();

    let kinds: Vec<ResourceKind> = args.resource_filter().unwrap_or_else(|| ALL_KINDS.to_vec());
    let config = load_config();

    let kind_label = if kinds.len() == 1 {
        kinds[0].as_str().to_string()
    } else {
        "all".to_string()
    };

    if args.all_contexts {
        run_all_contexts(&args, &kinds, &kind_label, &config).await
    } else {
        run_single_context(&args, &kinds, &kind_label, args.read_only, &config).await
    }
}

// ─── Single-cluster mode (with ctrl-x context switching) ─────────────────────

async fn run_single_context(
    args: &Args,
    kinds: &[ResourceKind],
    kind_label: &str,
    read_only: bool,
    config: &AppConfig,
) -> Result<()> {
    let mut active_ctx = args
        .context
        .clone()
        .or_else(load_last_context)
        .unwrap_or_else(current_context);
    let kubeconfig = args.kubeconfig.as_deref();
    let namespace = args.namespace.as_deref();
    let label_selector = args.label.as_deref();

    loop {
        let cost_snapshot = prefetch_cost_snapshot(&active_ctx, kubeconfig, &config.cost).await;
        let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();

        let ctx_for_watcher = active_ctx.clone();
        let tx_k8s = tx.clone();
        let kinds_clone = kinds.to_vec();
        let kubeconfig_owned = kubeconfig.map(str::to_string);
        let namespace_owned = namespace.map(str::to_string);
        let label_owned = label_selector.map(str::to_string);
        let cost_snapshot_owned = cost_snapshot.clone();
        tokio::spawn(async move {
            match build_client_for_context(&ctx_for_watcher, kubeconfig_owned.as_deref()).await {
                Ok(client) => {
                    if let Err(e) = watch_resources(
                        client,
                        tx_k8s,
                        &kinds_clone,
                        "",
                        namespace_owned.as_deref(),
                        label_owned.as_deref(),
                        cost_snapshot_owned,
                    )
                    .await
                    {
                        eprintln!("\n[kuberift] {e}");
                    }
                }
                Err(e) => {
                    eprintln!("[kuberift] No cluster ({e}). Showing demo data.");
                    if tx_k8s.send(demo_items()).is_err() {
                        eprintln!("[kuberift] warning: failed to send demo items to skim");
                    }
                }
            }
        });

        drop(tx);

        let cost_header = cost_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.header_label(namespace));
        let options = build_skim_options(
            &active_ctx,
            kind_label,
            true,
            read_only,
            namespace,
            cost_header.as_deref(),
        )?;
        let output = Skim::run_with(options, Some(rx)).map_err(|e| anyhow::anyhow!("{e}"))?;

        if output.is_abort {
            break;
        }

        let key = output.final_key;
        if key.code == KeyCode::Char('x') && key.modifiers == KeyModifiers::CONTROL {
            if let Some(new_ctx) = pick_context()? {
                active_ctx = new_ctx;
                save_last_context(&active_ctx);
            }
            install_preview_toggle();
            continue;
        }

        dispatch(&output, read_only)?;
        install_preview_toggle();
    }

    let _ = std::fs::remove_dir_all(runtime_dir());
    Ok(())
}

// ─── Multi-cluster mode (--all-contexts) ─────────────────────────────────────

async fn run_all_contexts(
    args: &Args,
    kinds: &[ResourceKind],
    kind_label: &str,
    config: &AppConfig,
) -> Result<()> {
    let contexts = list_contexts();
    if contexts.is_empty() {
        eprintln!("[kuberift] No contexts found in kubeconfig.");
        return Ok(());
    }

    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    let kubeconfig = args.kubeconfig.as_deref();
    let namespace = args.namespace.as_deref();
    let label_selector = args.label.as_deref();

    for ctx_name in &contexts {
        let tx_clone = tx.clone();
        let ctx_clone = ctx_name.clone();
        let kinds_clone = kinds.to_vec();
        let kubeconfig_owned = kubeconfig.map(str::to_string);
        let namespace_owned = namespace.map(str::to_string);
        let label_owned = label_selector.map(str::to_string);
        let cost_config = config.cost.clone();

        tokio::spawn(async move {
            match build_client_for_context(&ctx_clone, kubeconfig_owned.as_deref()).await {
                Ok(client) => {
                    let cost_snapshot = prefetch_cost_snapshot(
                        &ctx_clone,
                        kubeconfig_owned.as_deref(),
                        &cost_config,
                    )
                    .await;
                    if let Err(e) = watch_resources(
                        client,
                        tx_clone,
                        &kinds_clone,
                        &ctx_clone,
                        namespace_owned.as_deref(),
                        label_owned.as_deref(),
                        cost_snapshot,
                    )
                    .await
                    {
                        eprintln!("[kuberift:{ctx_clone}] {e}");
                    }
                }
                Err(e) => {
                    eprintln!("[kuberift] Cannot connect to '{ctx_clone}': {e}");
                }
            }
        });
    }

    drop(tx);

    let ctx_label = "all-contexts";
    let options = build_skim_options(
        ctx_label,
        kind_label,
        false,
        args.read_only,
        namespace,
        None,
    )?;
    let output = Skim::run_with(options, Some(rx)).map_err(|e| anyhow::anyhow!("{e}"))?;

    if output.is_abort {
        return Ok(());
    }

    dispatch(&output, args.read_only)
}

// ─── Context picker (ctrl-x) ──────────────────────────────────────────────────

fn pick_context() -> Result<Option<String>> {
    let contexts = list_contexts();
    if contexts.is_empty() {
        eprintln!("[kuberift] No contexts found in kubeconfig.");
        return Ok(None);
    }

    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for ctx in &contexts {
        // RST-009: log send failures
        if tx
            .send(vec![Arc::new(ContextItem(ctx.clone())) as Arc<dyn SkimItem>])
            .is_err()
        {
            eprintln!("[kuberift] warning: failed to send context item to skim");
        }
    }
    drop(tx);

    let options = SkimOptionsBuilder::default()
        .header("Select context  (Esc to cancel)")
        .prompt("context ❯ ")
        .height("40%")
        .build()?;

    let output = Skim::run_with(options, Some(rx)).map_err(|e| anyhow::anyhow!("{e}"))?;

    if output.is_abort || output.selected_items.is_empty() {
        return Ok(None);
    }

    Ok(Some(output.selected_items[0].output().to_string()))
}

/// Minimal `SkimItem` wrapper for a kubeconfig context name string.
struct ContextItem(String);

impl SkimItem for ContextItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.0)
    }
    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.0)
    }
}

// ─── Shared skim options builder ──────────────────────────────────────────────

// RST-006: return Result instead of panicking with .expect()
fn build_skim_options(
    ctx_label: &str,
    kind_label: &str,
    show_ctx_switch: bool,
    read_only: bool,
    namespace: Option<&str>,
    cost_header: Option<&str>,
) -> Result<SkimOptions> {
    let ctx_hint = if show_ctx_switch {
        "  ctrl-x switch-ctx"
    } else {
        ""
    };
    let ro_hint = if read_only { "  [READ-ONLY]" } else { "" };
    let ns_hint = namespace.map(|n| format!("  ns:{n}")).unwrap_or_default();
    let cost_hint = cost_header
        .map(|cost| format!("  {cost}"))
        .unwrap_or_default();

    Ok(SkimOptionsBuilder::default()
        .multi(true)
        .preview(String::new())
        .preview_window("right:50%")
        .height("60%")
        .header(format!(
            "KubeRift  ctx:{ctx_label}{ns_hint}{cost_hint}  res:{kind_label}{ro_hint}\n\
             <tab> select  <enter> describe  ctrl-l logs  ctrl-e exec  \
             ctrl-d delete  ctrl-f forward  ctrl-r restart  ctrl-y yaml  \
             ctrl-p cycle-preview{ctx_hint}",
        ))
        .prompt("❯ ")
        .bind({
            let mut binds = vec![
                "ctrl-l:accept".to_string(),
                "ctrl-e:accept".to_string(),
                "ctrl-d:accept".to_string(),
                "ctrl-f:accept".to_string(),
                "ctrl-r:accept".to_string(),
                "ctrl-y:accept".to_string(),
                format!(
                    "ctrl-p:execute({})+refresh-preview",
                    preview_toggle_path().display()
                ),
            ];
            if show_ctx_switch {
                binds.push("ctrl-x:accept".to_string());
            }
            binds
        })
        .build()?)
}

// ─── Action dispatch ──────────────────────────────────────────────────────────

// RST-005: removed `async` — all action functions are synchronous
fn dispatch(output: &SkimOutput, read_only: bool) -> Result<()> {
    let items: Vec<&K8sItem> = output
        .selected_items
        .iter()
        .filter_map(|matched| {
            let inner: &dyn SkimItem = &*matched.item;
            inner.as_any().downcast_ref::<K8sItem>()
        })
        .collect();

    if items.is_empty() {
        return Ok(());
    }

    let key = output.final_key;
    let ctrl = |c: char| key.code == KeyCode::Char(c) && key.modifiers == KeyModifiers::CONTROL;

    if ctrl('l') {
        action_logs(&items)?;
    } else if ctrl('e') {
        if read_only {
            eprintln!("[kuberift] read-only mode: exec is disabled");
        } else if let Some(item) = items.first() {
            action_exec(item)?;
        }
    } else if ctrl('d') {
        if read_only {
            eprintln!("[kuberift] read-only mode: delete is disabled");
        } else {
            action_delete(&items)?;
        }
    } else if ctrl('f') {
        if read_only {
            eprintln!("[kuberift] read-only mode: port-forward is disabled");
        } else if let Some(item) = items.first() {
            action_portforward(item)?;
        }
    } else if ctrl('r') {
        if read_only {
            eprintln!("[kuberift] read-only mode: rollout-restart is disabled");
        } else {
            action_rollout_restart(&items)?;
        }
    } else if ctrl('y') {
        action_yaml(&items)?;
    } else {
        action_describe(&items)?;
    }

    Ok(())
}

// ─── Demo data (no cluster) ───────────────────────────────────────────────────

fn demo_items() -> Vec<Arc<dyn SkimItem>> {
    vec![
        Arc::new(K8sItem::new(
            ResourceKind::Pod,
            "production",
            "api-server-7d9f8b6c5-xk2lp",
            "CrashLoopBackOff",
            "1h",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Pod,
            "staging",
            "frontend-5c7d8e9f0-ab1cd",
            "Pending",
            "5m",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Pod,
            "production",
            "worker-6f8b9c4d7-mn3qr",
            "Running",
            "2d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Deployment,
            "production",
            "api-server",
            "2/3",
            "2d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Deployment,
            "staging",
            "frontend",
            "0/1",
            "5m",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Service,
            "production",
            "api-service",
            "ClusterIP",
            "2d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::ConfigMap,
            "production",
            "app-config",
            "ConfigMap",
            "2d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Secret,
            "production",
            "api-tls",
            "kubernetes.io/tls",
            "30d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Node,
            "",
            "kind-control-plane",
            "Ready",
            "7d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Namespace,
            "",
            "production",
            "Active",
            "30d",
            "",
        )),
        Arc::new(K8sItem::new(
            ResourceKind::Namespace,
            "",
            "staging",
            "Active",
            "10d",
            "",
        )),
    ]
}
