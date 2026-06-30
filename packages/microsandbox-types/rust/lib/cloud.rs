//! Wire types for the cloud backend's HTTP calls.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::VolumeKind;

//--------------------------------------------------------------------------------------------------
// Types: Request
//--------------------------------------------------------------------------------------------------

/// Wire shape of a cloud sandbox create request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct CloudCreateSandboxRequest {
    /// User-facing sandbox name.
    pub name: String,
    /// OCI image reference to run.
    pub image: String,
    /// Virtual CPU count.
    pub vcpus: u8,
    /// Guest memory in MiB.
    pub memory_mib: u32,
    /// Environment variables injected into the sandbox.
    pub env: HashMap<String, String>,
    /// Whether the sandbox should be removed when its allocation terminates.
    pub ephemeral: bool,

    // Optional config fields.
    /// Working directory inside the guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Default shell inside the guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// OCI entrypoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Vec<String>>,
    /// Guest hostname override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Guest user identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Runtime log verbosity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    /// Named scripts mounted into the guest.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scripts: HashMap<String, String>,
    /// Hard sandbox lifetime cap in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_secs: Option<u64>,
    /// Idle timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_secs: Option<u64>,
}

//--------------------------------------------------------------------------------------------------
// Types: Response
//--------------------------------------------------------------------------------------------------

/// Wire shape of the cloud sandbox response returned by sandbox endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudSandbox {
    /// Server-side UUID.
    pub id: String,
    /// Owning org's UUID.
    pub org_id: String,
    /// User-facing sandbox name.
    pub name: String,
    /// Current lifecycle status.
    pub status: CloudSandboxStatus,
    /// Create request stored by the cloud control plane.
    pub config: CloudCreateSandboxRequest,
    /// Whether the sandbox should be removed when its allocation terminates.
    pub ephemeral: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last start timestamp, when known.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub started_at: Option<DateTime<Utc>>,
    /// Last stop timestamp, when known.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stopped_at: Option<DateTime<Utc>>,
    /// Last failure reason, when any.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub last_error: Option<String>,
}

/// Sandbox lifecycle status returned by the cloud control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CloudSandboxStatus {
    /// Created in the database but not yet started.
    Created,
    /// Start request has been submitted.
    Starting,
    /// Sandbox is running.
    Running,
    /// Stop request has been submitted.
    Stopping,
    /// Sandbox is stopped.
    Stopped,
    /// Sandbox failed.
    Failed,
}

/// Wire shape of paginated list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudPaginated<T> {
    /// Page of response items.
    pub data: Vec<T>,
    /// Cursor for the next page, when one exists.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub next_cursor: Option<String>,
}

/// Wire shape of the message response returned by mutation endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudMessageResponse {
    /// Human-readable response message.
    pub message: String,
}

/// Wire shape of a sandbox metrics sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudSandboxMetrics {
    /// CPU usage as a percentage across all host CPUs.
    pub cpu_percent: f32,
    /// Cumulative guest vCPU execution time across all vCPUs.
    pub vcpu_time_ns: u64,
    /// Resident memory usage in bytes.
    pub memory_bytes: u64,
    /// Guest-available memory in bytes when reported by the guest.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub memory_available_bytes: Option<u64>,
    /// Host-resident guest memory in bytes for capacity diagnostics.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub memory_host_resident_bytes: Option<u64>,
    /// Configured guest memory limit in bytes.
    pub memory_limit_bytes: u64,
    /// Cumulative disk bytes read by the sandbox process.
    pub disk_read_bytes: u64,
    /// Cumulative disk bytes written by the sandbox process.
    pub disk_write_bytes: u64,
    /// Cumulative network bytes delivered from the runtime to the guest.
    pub net_rx_bytes: u64,
    /// Cumulative network bytes transmitted from the guest into the runtime.
    pub net_tx_bytes: u64,
    /// Guest-visible OCI upper filesystem used bytes when available.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub upper_used_bytes: Option<u64>,
    /// Guest-visible OCI upper filesystem free bytes when available.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub upper_free_bytes: Option<u64>,
    /// Host-allocated bytes for the writable upper image when available.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub upper_host_allocated_bytes: Option<u64>,
    /// Sandbox uptime in milliseconds.
    pub uptime_ms: u64,
    /// Timestamp of the sample.
    pub timestamp: DateTime<Utc>,
}

/// Wire shape of a guest filesystem entry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum CloudFsEntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Other filesystem node type.
    Other,
}

/// Wire shape of a guest filesystem directory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudFsEntry {
    /// Path of the entry.
    pub path: String,
    /// Kind of entry.
    pub kind: CloudFsEntryKind,
    /// Size in bytes.
    pub size: u64,
    /// Unix permission bits.
    pub mode: u32,
    /// Last modification time.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub modified: Option<DateTime<Utc>>,
}

/// Wire shape of guest filesystem metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudFsMetadata {
    /// Kind of entry.
    pub kind: CloudFsEntryKind,
    /// Size in bytes.
    pub size: u64,
    /// Unix permission bits.
    pub mode: u32,
    /// Whether the entry is read-only.
    pub readonly: bool,
    /// Last modification time.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub modified: Option<DateTime<Utc>>,
    /// Creation time.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub created: Option<DateTime<Utc>>,
}

/// Wire shape of a guest path existence response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudFsExistsResponse {
    /// Whether the path exists.
    pub exists: bool,
}

/// Wire shape of a single guest path mutation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudFsPathRequest {
    /// Guest path.
    pub path: String,
}

/// Wire shape of a guest copy/rename request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudFsTwoPathRequest {
    /// Source guest path.
    pub from: String,
    /// Destination guest path.
    pub to: String,
}

/// Wire shape of a cloud volume response returned by volume endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudVolume {
    /// Server-side UUID.
    pub id: String,
    /// Owning org's UUID.
    pub org_id: String,
    /// User-facing volume name.
    pub name: String,
    /// Storage kind.
    pub kind: VolumeKind,
    /// Configured quota in MiB, when set.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub quota_mib: Option<u32>,
    /// Disk usage snapshot at handle-fetch time.
    pub used_bytes: u64,
    /// Disk capacity in bytes for disk volumes.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub capacity_bytes: Option<u64>,
    /// Disk image format for disk volumes.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub disk_format: Option<String>,
    /// Inner disk filesystem for disk volumes.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub disk_fstype: Option<String>,
    /// Key-value labels associated with the volume.
    pub labels: Vec<(String, String)>,
    /// Creation timestamp.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub created_at: Option<DateTime<Utc>>,
}

/// Wire shape of the typed error body returned by cloud APIs on 4xx/5xx responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudErrorBody {
    /// Flat machine-readable error code, when returned in this shape.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub code: Option<String>,
    /// Flat human-readable error message, when returned in this shape.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub message: Option<String>,
    /// Nested error object returned by the API error responder.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub error: Option<CloudErrorDetails>,
}

/// Nested cloud API error details.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CloudErrorDetails {
    /// Machine-readable error code.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub code: Option<String>,
    /// Human-readable error message.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub message: Option<String>,
}

//--------------------------------------------------------------------------------------------------
// Trait Implementations
//--------------------------------------------------------------------------------------------------

impl Default for CloudCreateSandboxRequest {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            vcpus: 1,
            memory_mib: 512,
            env: HashMap::new(),
            ephemeral: true,
            workdir: None,
            shell: None,
            entrypoint: None,
            hostname: None,
            user: None,
            log_level: None,
            scripts: HashMap::new(),
            max_duration_secs: None,
            idle_timeout_secs: None,
        }
    }
}

//--------------------------------------------------------------------------------------------------
// Tests
//--------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_serialises_minimal() {
        let req = CloudCreateSandboxRequest {
            name: "agent-1".into(),
            image: "python:3.12".into(),
            ..Default::default()
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["name"], "agent-1");
        assert_eq!(json["image"], "python:3.12");
        assert_eq!(json["vcpus"], 1);
        assert_eq!(json["memory_mib"], 512);
        assert_eq!(json["ephemeral"], true);
        assert!(json.get("workdir").is_none());
        assert!(json.get("entrypoint").is_none());
        assert!(json.get("max_duration_secs").is_none());
    }

    #[test]
    fn create_request_serialises_full_d13() {
        let mut req = CloudCreateSandboxRequest {
            name: "agent-1".into(),
            image: "python:3.12".into(),
            workdir: Some("/app".into()),
            shell: Some("/bin/bash".into()),
            entrypoint: Some(vec!["python".into(), "-u".into()]),
            hostname: Some("worker".into()),
            user: Some("appuser".into()),
            log_level: Some("info".into()),
            max_duration_secs: Some(3600),
            idle_timeout_secs: Some(600),
            ..Default::default()
        };
        req.scripts.insert("setup".into(), "echo hi".into());
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["workdir"], "/app");
        assert_eq!(json["shell"], "/bin/bash");
        assert_eq!(json["entrypoint"], serde_json::json!(["python", "-u"]));
        assert_eq!(json["max_duration_secs"], 3600);
        assert_eq!(json["scripts"]["setup"], "echo hi");
    }

    #[test]
    fn sandbox_status_round_trips() {
        for status in [
            CloudSandboxStatus::Created,
            CloudSandboxStatus::Starting,
            CloudSandboxStatus::Running,
            CloudSandboxStatus::Stopping,
            CloudSandboxStatus::Stopped,
            CloudSandboxStatus::Failed,
        ] {
            let s = serde_json::to_string(&status).unwrap();
            let parsed: CloudSandboxStatus = serde_json::from_str(&s).unwrap();
            assert_eq!(status, parsed);
        }
    }

    #[test]
    fn sandbox_status_serialises_snake_case() {
        let s = serde_json::to_string(&CloudSandboxStatus::Starting).unwrap();
        assert_eq!(s, "\"starting\"");
    }

    #[test]
    fn sandbox_response_parses_typical() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000002",
            "org_id": "00000000-0000-0000-0000-000000000001",
            "name": "agent-1",
            "status": "created",
            "config": { "name": "agent-1", "image": "python:3.12" },
            "ephemeral": true,
            "created_at": "2026-05-17T12:00:00Z"
        }"#;
        let sb: CloudSandbox = serde_json::from_str(json).unwrap();
        assert_eq!(sb.name, "agent-1");
        assert_eq!(sb.status, CloudSandboxStatus::Created);
        assert_eq!(sb.config.image, "python:3.12");
        assert!(sb.started_at.is_none());
    }
}
