use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use skim::{DisplayContext, ItemPreview, PreviewContext, SkimItem};
use std::borrow::Cow;

// ─── Name truncation helper ───────────────────────────────────────────────────

/// Truncate `name` to at most `max_chars` bytes, appending "…" if truncated.
/// Always splits on a valid UTF-8 char boundary to avoid panics on multi-byte names.
pub fn truncate_name(name: &str, max_chars: usize) -> Cow<'_, str> {
    if name.len() <= max_chars {
        return Cow::Borrowed(name);
    }
    let mut end = max_chars;
    while !name.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    Cow::Owned(format!("{}…", &name[..end]))
}

// ─── Status health classification ────────────────────────────────────────────

/// Health category for a Kubernetes resource status string.
/// Single source of truth for both color and sort-priority decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusHealth {
    /// Needs immediate attention: `CrashLoopBackOff`, Error, Failed, Evicted, etc.
    Critical,
    /// Transitional state: Pending, Terminating, Init:X/Y, etc.
    Warning,
    /// Normal operation: Running, Active, Bound, Complete, etc.
    Healthy,
    /// Deleted or unrecognized.
    Unknown,
}

impl StatusHealth {
    /// Classify a status string into a health category.
    pub fn classify(status: &str) -> Self {
        match status {
            // ── Exact critical matches ────────────────────────────────────────
            "Failed" | "Error" | "OOMKilled" | "NotReady" | "Lost" | "Evicted" | "BackOff" => {
                Self::Critical
            }
            // ── Prefix-based critical matches ─────────────────────────────────
            s if s.starts_with("CrashLoop")
                || s.starts_with("ErrImage")
                || s.starts_with("ImagePull")
                || s.starts_with("Init:Error")
                || s.starts_with("Init:ErrImage")
                || s.starts_with("Init:ImagePull")
                || s.starts_with("Failed(") =>
            {
                Self::Critical
            }
            // ── Exact warning matches ─────────────────────────────────────────
            "Pending" | "Terminating" | "ContainerCreating" | "Unknown" => Self::Warning,
            // ── Prefix-based warning matches ──────────────────────────────────
            s if s.starts_with("Init:") => Self::Warning,
            // ── Deleted ───────────────────────────────────────────────────────
            "[DELETED]" => Self::Unknown,
            // ── Exact healthy matches ─────────────────────────────────────────
            "Running" | "Active" | "Bound" | "Complete" | "Succeeded" | "Ready" | "Scheduled"
            | "ClusterIP" | "NodePort" | "LoadBalancer" => Self::Healthy,
            // ── Prefix-based healthy ──────────────────────────────────────────
            s if s.starts_with("Active(") => Self::Healthy,
            // ── Ratio: "3/3" healthy, "1/3" warning ──────────────────────────
            s if s.contains('/') => {
                let parts: Vec<&str> = s.splitn(2, '/').collect();
                if parts.len() == 2 && parts[0] == parts[1] {
                    Self::Healthy
                } else {
                    Self::Warning
                }
            }
            // ── Unknown statuses default to healthy ───────────────────────────
            _ => Self::Healthy,
        }
    }

    /// Terminal color for this health category.
    pub fn color(self) -> Color {
        match self {
            Self::Critical => Color::Red,
            Self::Warning => Color::Yellow,
            Self::Healthy => Color::Green,
            Self::Unknown => Color::DarkGray,
        }
    }

    /// Sort priority: 0 = top of list (critical), 1 = middle, 2 = bottom (healthy).
    /// Skim renders higher-indexed items at the top, so lower priority = sent last.
    pub fn priority(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::Warning | Self::Unknown => 1,
            Self::Healthy => 2,
        }
    }
}

/// The kind of Kubernetes resource this item represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceKind {
    Pod,
    Service,
    Deployment,
    StatefulSet,
    DaemonSet,
    ConfigMap,
    Secret,
    Ingress,
    Node,
    Namespace,
    PersistentVolume,
    PersistentVolumeClaim,
    Job,
    CronJob,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pod => "pod",
            Self::Service => "svc",
            Self::Deployment => "deploy",
            Self::StatefulSet => "sts",
            Self::DaemonSet => "ds",
            Self::ConfigMap => "cm",
            Self::Secret => "secret",
            Self::Ingress => "ing",
            Self::Node => "node",
            Self::Namespace => "ns",
            Self::PersistentVolume => "pv",
            Self::PersistentVolumeClaim => "pvc",
            Self::Job => "job",
            Self::CronJob => "cronjob",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Pod => Color::Green,
            Self::Service => Color::Blue,
            Self::Deployment | Self::StatefulSet | Self::DaemonSet => Color::Yellow,
            Self::ConfigMap | Self::Secret => Color::Magenta,
            Self::Ingress => Color::Cyan,
            Self::Node | Self::Namespace => Color::White,
            Self::PersistentVolume => Color::LightCyan,
            Self::PersistentVolumeClaim => Color::LightMagenta,
            Self::Job | Self::CronJob => Color::LightBlue,
        }
    }
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A Kubernetes resource item displayed in the skim TUI.
#[derive(Debug, Clone)]
pub struct K8sItem {
    kind: ResourceKind,
    namespace: String,
    name: String,
    status: String,
    age: String,
    /// The cluster context this resource belongs to (empty in single-cluster mode).
    context: String,
    cost_label: String,
    cost_highlighted: bool,
}

impl K8sItem {
    pub fn new(
        kind: ResourceKind,
        namespace: impl Into<String>,
        name: impl Into<String>,
        status: impl Into<String>,
        age: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            namespace: namespace.into(),
            name: name.into(),
            status: status.into(),
            age: age.into(),
            context: context.into(),
            cost_label: String::new(),
            cost_highlighted: false,
        }
    }

    pub fn kind(&self) -> ResourceKind {
        self.kind
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn set_cost(&mut self, label: impl Into<String>, highlighted: bool) {
        self.cost_label = label.into();
        self.cost_highlighted = highlighted;
    }

    pub fn clear_cost(&mut self) {
        self.cost_label.clear();
        self.cost_highlighted = false;
    }

    pub fn cost_label(&self) -> Option<&str> {
        (!self.cost_label.is_empty()).then_some(self.cost_label.as_str())
    }

    /// Color the status string based on health — delegates to `StatusHealth`.
    pub fn status_color(&self) -> Color {
        StatusHealth::classify(&self.status).color()
    }

    /// Machine-parseable output string for piping.
    /// In multi-cluster mode, prefixed with the context: "ctx:kind/ns/name"
    pub fn output_str(&self) -> String {
        let loc = if self.namespace.is_empty() {
            format!("{}/{}", self.kind.as_str(), self.name)
        } else {
            format!("{}/{}/{}", self.kind.as_str(), self.namespace, self.name)
        };
        if self.context.is_empty() {
            loc
        } else {
            format!("{}:{}", self.context, loc)
        }
    }
}

/// Pick a consistent color for a cluster context name based on a hash of the name.
/// Ensures the same context always gets the same color across all items.
pub fn context_color(ctx: &str) -> Color {
    const PALETTE: &[Color] = &[
        Color::Cyan,
        Color::Magenta,
        Color::Yellow,
        Color::LightGreen,
        Color::LightBlue,
        Color::LightRed,
        Color::LightCyan,
        Color::LightMagenta,
    ];
    let hash: usize = ctx
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_add(b as usize));
    PALETTE[hash % PALETTE.len()]
}

impl SkimItem for K8sItem {
    /// The text skim fuzzy-matches against — plain, no color.
    /// In multi-cluster mode the context name is included so users can search by cluster.
    fn text(&self) -> Cow<'_, str> {
        let ctx_prefix = if self.context.is_empty() {
            String::new()
        } else {
            format!("{}/", self.context)
        };
        let ns_prefix = if self.namespace.is_empty() {
            String::new()
        } else {
            format!("{}/", self.namespace)
        };
        let name_truncated = truncate_name(&self.name, 31);
        Cow::Owned(format!(
            "{:<8} {}{}{} {} {} {}",
            self.kind.as_str(),
            ctx_prefix,
            ns_prefix,
            name_truncated,
            self.status,
            self.age,
            self.cost_label,
        ))
    }

    /// Colored display shown in the skim list.
    /// In multi-cluster mode a context prefix is shown before the namespace/name,
    /// colored distinctly per cluster.
    fn display(&self, _context: DisplayContext) -> Line<'_> {
        let ns_prefix = if self.namespace.is_empty() {
            String::new()
        } else {
            format!("{}/", self.namespace)
        };

        let mut spans = vec![Span::styled(
            format!("{:<8} ", self.kind.as_str()),
            Style::default().fg(self.kind.color()),
        )];

        // Context prefix — only shown in multi-cluster mode
        if !self.context.is_empty() {
            spans.push(Span::styled(
                format!("{}/", self.context),
                Style::default().fg(context_color(&self.context)),
            ));
        }

        spans.push(Span::styled(ns_prefix, Style::default().fg(Color::Cyan)));
        let name_col = {
            let t = truncate_name(&self.name, 31);
            if t.len() < 32 {
                format!("{t:<32} ")
            } else {
                format!("{t} ")
            }
        };
        spans.push(Span::styled(name_col, Style::default().fg(Color::White)));
        spans.push(Span::styled(
            format!("{:<17} ", self.status),
            Style::default().fg(self.status_color()),
        ));
        spans.push(Span::styled(
            self.age.clone(),
            Style::default().fg(Color::DarkGray),
        ));
        let cost_col = self.cost_label().map_or_else(
            || "          ".to_string(),
            |label| format!(" {:>10}", label),
        );
        let cost_color = if self.cost_highlighted {
            Color::LightRed
        } else {
            Color::LightGreen
        };
        spans.push(Span::styled(cost_col, Style::default().fg(cost_color)));

        Line::from(spans)
    }

    /// Preview pane content — mode cycles via ctrl-p (describe → yaml → logs).
    /// Passes --context when the item belongs to a non-default cluster.
    /// Skim calls this from a background thread; blocking is fine here.
    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        let mode = crate::actions::current_preview_mode();

        // Build the kubectl argument list for the current preview mode.
        // Namespace (-n) and --context must come BEFORE the `--` end-of-flags
        // separator; anything after `--` is treated as a resource name by kubectl.
        let mut args: Vec<&str> = if mode == 2 && matches!(self.kind, ResourceKind::Pod) {
            vec!["logs", "--tail=100"]
        } else {
            match mode {
                1 => vec!["get", self.kind.as_str(), "-o", "yaml"],
                _ => vec!["describe", self.kind.as_str()],
            }
        };

        if !self.namespace.is_empty() {
            args.push("-n");
            args.push(&self.namespace);
        }

        // Target the correct cluster in multi-context mode
        if !self.context.is_empty() {
            args.push("--context");
            args.push(&self.context);
        }

        args.push("--");
        args.push(&self.name);

        match std::process::Command::new("kubectl").args(&args).output() {
            Ok(out) => {
                let header = match mode {
                    1 => format!("── YAML: {}/{} ──\n", self.kind.as_str(), self.name),
                    2 => format!("── LOGS: {} (last 100) ──\n", self.name),
                    _ => format!("── DESCRIBE: {}/{} ──\n", self.kind.as_str(), self.name),
                };
                let body = if out.status.success() {
                    String::from_utf8_lossy(&out.stdout).to_string()
                } else {
                    format!("[kubectl error]\n{}", String::from_utf8_lossy(&out.stderr))
                };
                ItemPreview::AnsiText(format!("{header}{body}"))
            }
            Err(e) => ItemPreview::Text(format!(
                "[Error running kubectl]\n{e}\n\nIs kubectl in your PATH?"
            )),
        }
    }

    /// What gets written to stdout when this item is selected
    fn output(&self) -> Cow<'_, str> {
        Cow::Owned(self.output_str())
    }
}
