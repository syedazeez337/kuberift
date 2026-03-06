# kf — KubeRift

> A fuzzy-first interactive Kubernetes resource navigator

`kf` lets you fuzzy-search every resource across every namespace in your cluster from a single terminal window. Select one or many, then describe, exec, tail logs, delete, port-forward, restart, or dump YAML — all without typing a single `kubectl` command.

![demo](https://raw.githubusercontent.com/syedazeez337/kuberift/master/docs/media/demo.gif)

---

## Features

- **Fuzzy search everything** — pods, deployments, services, secrets, configmaps, nodes, namespaces, PVCs, jobs, cronjobs, statefulsets, daemonsets, ingresses — all at once
- **Live preview pane** — inline `describe`, YAML manifest, or pod logs, cycled with `ctrl-p`
- **Live watch** — resources appear and update in real time as the cluster changes; deleted resources show `[DELETED]`
- **Unhealthy-first ordering** — `CrashLoopBackOff`, `Error`, `ImagePullBackOff` pods surface to the top automatically
- **Color-coded status** — red for critical, yellow for warning, green for healthy, dimmed for deleted
- **Optional OpenCost column** — show per-pod / per-controller cost inline, with namespace or cluster aggregate in the header
- **Multi-select bulk actions** — `tab` to select multiple resources, then describe/delete/restart them all at once
- **Multi-cluster support** — watch all kubeconfig contexts simultaneously with `--all-contexts`, or switch contexts interactively with `ctrl-x`
- **Context persistence** — last-used context is remembered across sessions
- **Demo mode** — works without a cluster; shows sample data so you can explore the UI

---

## Documentation

A full user manual — installation, every keybinding, all actions, architecture, multi-cluster workflows, and the story of how skim shaped the design — is available as a compiled PDF:

**[📄 docs/manual/kuberift-manual.pdf](docs/manual/kuberift-manual.pdf)**

---

## Requirements

- Rust toolchain (`cargo`) — to build from source
- `kubectl` in `$PATH` — used for all actions (describe, logs, exec, delete, etc.)
- A valid kubeconfig (`~/.kube/config` or `$KUBECONFIG`) — optional; demo mode activates automatically if absent

---

## Installation

```bash
git clone https://github.com/syedazeez337/kuberift.git
cd kuberift
cargo build --release
# Binary is at target/release/kf
# Optionally move it somewhere on your PATH:
sudo mv target/release/kf /usr/local/bin/kf
```

> **Note:** The skim dependency is pulled automatically from a patched git fork during `cargo build`. No separate clone is required.

---

## Usage

```
kf [RESOURCE] [OPTIONS]
```

### Show all resources (default)

```bash
kf
```

Opens the TUI with every resource type streaming from the current kubeconfig context.

### Filter to a specific resource type

```bash
kf pods        # or: pod, po
kf deploy      # or: deployment, deployments
kf svc         # or: service, services
kf sts         # or: statefulset, statefulsets
kf ds          # or: daemonset, daemonsets
kf cm          # or: configmap, configmaps
kf secret      # or: secrets
kf ing         # or: ingress, ingresses
kf node        # or: nodes, no
kf ns          # or: namespace, namespaces
kf pvc         # or: persistentvolumeclaim
kf job         # or: jobs
kf cj          # or: cronjob, cronjobs
```

### Use a specific context

```bash
kf --context my-prod-cluster
```

Overrides both the kubeconfig current context and the last-saved context.

### Watch all contexts simultaneously

```bash
kf --all-contexts
```

Streams resources from every context in your kubeconfig in parallel. Each item is prefixed with its cluster name (color-coded per cluster).

---

## Keybindings

### Navigation

| Key | Action |
|-----|--------|
| Type anything | Fuzzy filter the list in real time |
| `↑` / `↓` | Move cursor |
| `tab` | Toggle selection on current item (multi-select) |
| `esc` | Quit |

### Actions (on selected item(s))

| Key | Action | Multi-select |
|-----|--------|:---:|
| `enter` | `kubectl describe` | ✓ |
| `ctrl-l` | Stream pod logs (`--tail=200`) | ✓ |
| `ctrl-e` | `kubectl exec -it` into shell | — |
| `ctrl-d` | Delete with `y/N` confirmation | ✓ |
| `ctrl-f` | Port-forward (prompts for local/remote port) | — |
| `ctrl-r` | `kubectl rollout restart` (deploy/sts/ds) | ✓ |
| `ctrl-y` | Print YAML to stdout | ✓ |

### Preview & context

| Key | Action |
|-----|--------|
| `ctrl-p` | Cycle preview mode: **describe → yaml → logs** |
| `ctrl-x` | Open context picker — switch cluster without restarting |

---

## Preview Modes

The right-hand preview pane updates as you move the cursor. Press `ctrl-p` to cycle through three modes:

| Mode | Content |
|------|---------|
| `describe` | `kubectl describe <resource>` output |
| `yaml` | `kubectl get <resource> -o yaml` |
| `logs` | Last 100 lines of pod logs (pods only) |

---

## Multi-cluster Mode

```bash
kf --all-contexts
```

All contexts from your kubeconfig are loaded in parallel. Items are prefixed with the cluster name:

```
pod   prod-cluster/default/api-server-7d9f     Running   2d
pod   staging/default/api-server-5c2a          Pending   5m
```

Each cluster gets a distinct color so items are immediately identifiable.

### Switching contexts interactively

Press `ctrl-x` while `kf` is running to open a secondary fuzzy picker showing all your kubeconfig contexts. Selecting a context restarts the resource stream from that cluster. The selected context is saved to `~/.config/kuberift/last_context` and restored on the next launch.

---

## Status Colors

| Color | Meaning | Example statuses |
|-------|---------|-----------------|
| Red | Critical — needs attention | `CrashLoopBackOff`, `Error`, `ImagePullBackOff`, `OOMKilled`, `Failed`, `Evicted` |
| Yellow | Warning — transitional | `Pending`, `Terminating`, `Init:0/1`, `ContainerCreating` |
| Green | Healthy | `Running`, `Succeeded`, `Active`, `Bound`, `ClusterIP` |
| Gray | Gone | `[DELETED]`, `Unknown` |

Unhealthy resources (red) automatically sort to the top of the list so critical issues are visible immediately without scrolling.

---

## Demo Mode

If `kubectl` cannot connect to a cluster (no kubeconfig, invalid context, or network error), `kf` falls back to demo mode and displays 11 sample resources so you can explore the interface:

```bash
KUBECONFIG=/nonexistent kf
# [kuberift] No cluster (...). Showing demo data.
```

---

## Additional Options

```bash
kf -n production          # restrict to the 'production' namespace
kf --read-only            # disable delete, exec, port-forward, rollout-restart
kf --kubeconfig ~/alt.yaml --context staging  # use an alternate kubeconfig
```

---

## Optional Cost Awareness

Create `~/.config/kuberift/config.toml`:

```toml
[cost]
enabled = true
opencost_endpoint = ""      # optional manual override; empty = auto-discover in cluster
display_period = "daily"    # hourly | daily | monthly
highlight_threshold = 10.0  # highlight resources above $/day
```

When cost mode is enabled, `kf` will:

- detect `opencost` / `kubecost` services and query `allocation/compute`
- map pod allocations onto pods and controller-backed resources
- render a cost column without changing existing actions or output format
- disable cost for the current session if the endpoint is missing, slow, or unreachable

The namespace filter (`-n production`) also shows the namespace aggregate in the header.

---

## Config & State

| File | Purpose |
|------|---------|
| `~/.config/kuberift/config.toml` | Optional settings (`[cost]`) for OpenCost integration |
| `~/.config/kuberift/last_context` | Last-used context, restored on next launch |
| `$XDG_RUNTIME_DIR/<pid>/preview-mode` | Preview mode state (0=describe, 1=yaml, 2=logs) |
| `$XDG_RUNTIME_DIR/<pid>/preview-toggle` | Shell script installed at startup for ctrl-p |

---

## License

MIT — same as [skim](https://github.com/skim-rs/skim), the underlying fuzzy engine.
