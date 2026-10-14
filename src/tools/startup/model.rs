use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a startup entry is registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SourceKind {
    RegistryRun,
    StartupFolder,
    Service,
    ScheduledTask,
    Winlogon,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::RegistryRun => "Registry",
            SourceKind::StartupFolder => "Startup folder",
            SourceKind::Service => "Service",
            SourceKind::ScheduledTask => "Scheduled task",
            SourceKind::Winlogon => "Winlogon",
        }
    }
}

/// Estimated effect on boot / sign-in time and resource use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Impact {
    Low,
    Medium,
    High,
}

impl Impact {
    pub fn label(self) -> &'static str {
        match self {
            Impact::Low => "Low",
            Impact::Medium => "Medium",
            Impact::High => "High",
        }
    }
    pub fn lower(self) -> Self {
        match self {
            Impact::High => Impact::Medium,
            _ => Impact::Low,
        }
    }
}

/// How dangerous it is to switch an entry off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Criticality {
    /// Safe to disable; nothing in Windows depends on it.
    Optional,
    /// Disabling turns off a feature, a driver helper or updates. Proceed knowingly.
    Important,
    /// Windows itself (sign-in, networking, security, input…) depends on it.
    Critical,
}

impl Criticality {
    pub fn label(self) -> &'static str {
        match self {
            Criticality::Optional => "Safe to disable",
            Criticality::Important => "Use caution",
            Criticality::Critical => "Critical",
        }
    }
}

/// Everything needed to find an entry again in order to change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locator {
    /// A value under a `Run` key, toggled through Explorer's `StartupApproved` key.
    RunValue {
        machine: bool,
        /// Name of the `StartupApproved` subkey: `Run` or `Run32`.
        approved_subkey: String,
        value_name: String,
    },
    /// A file in a Startup folder, toggled through `StartupApproved\StartupFolder`.
    StartupFile { machine: bool, file_name: String },
    Service { name: String },
    Task { path: String },
    /// Read-only entries (RunOnce, policy Run keys, Winlogon).
    ReadOnly,
}

#[derive(Clone, Debug)]
pub struct ServiceDetails {
    pub name: String,
    pub start_type: u32,
    pub delayed: bool,
    pub shared_process: bool,
}

#[derive(Clone, Debug)]
pub struct StartupItem {
    pub id: String,
    pub name: String,
    pub command: String,
    pub target: Option<PathBuf>,
    pub target_exists: bool,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub source: SourceKind,
    /// Human readable location (registry key, folder, task path, service name).
    pub location: String,
    pub locator: Locator,
    pub enabled: bool,
    /// e.g. "Automatic (delayed start)" or "Enabled".
    pub status_text: String,
    /// When false, the reason is in `readonly_reason`.
    pub can_toggle: bool,
    pub readonly_reason: Option<String>,
    /// Changing it needs administrator rights.
    pub needs_admin: bool,
    pub is_microsoft: bool,
    pub file_size: Option<u64>,
    pub memory: Option<u64>,
    pub service: Option<ServiceDetails>,
    /// Extra facts for the details pane (label, value).
    pub facts: Vec<(String, String)>,
    pub impact: Impact,
    pub impact_reason: String,
    pub criticality: Criticality,
    pub criticality_reason: String,
}

impl StartupItem {
    pub fn new(id: String, name: String, command: String, source: SourceKind, location: String, locator: Locator) -> Self {
        Self {
            id,
            name,
            command,
            target: None,
            target_exists: false,
            publisher: None,
            description: None,
            source,
            location,
            locator,
            enabled: true,
            status_text: "Enabled".into(),
            can_toggle: true,
            readonly_reason: None,
            needs_admin: false,
            is_microsoft: false,
            file_size: None,
            memory: None,
            service: None,
            facts: Vec::new(),
            impact: Impact::Low,
            impact_reason: String::new(),
            criticality: Criticality::Optional,
            criticality_reason: String::new(),
        }
    }

    pub fn read_only(mut self, reason: &str) -> Self {
        self.can_toggle = false;
        self.locator = Locator::ReadOnly;
        self.readonly_reason = Some(reason.to_string());
        self
    }

    /// Lowercased text used for search and keyword matching.
    pub fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.name,
            self.command,
            self.publisher.as_deref().unwrap_or(""),
            self.description.as_deref().unwrap_or(""),
            self.location
        )
        .to_lowercase()
    }
}
