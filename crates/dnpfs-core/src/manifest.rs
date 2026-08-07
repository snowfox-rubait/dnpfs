use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationType {
    Write,
    Delete,
    Copy,
    Rename,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ManifestStatus {
    Pending,
    Confirmed,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtentTarget {
    pub index: u32,
    pub start_block: u64,
    pub block_count: u64,
    pub group_checksum: u64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpec {
    pub path: String,
    pub size_bytes: u64,
    pub checksum_xxhash3: u64,
    pub cascade_delete_on_confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationSpec {
    pub device_uuid: String,
    pub extent_map: Vec<ExtentTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreOperationSnapshot {
    pub inode_id: u64,
    pub old_size: u64,
    pub old_block_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationSpec {
    pub bad_sectors_in_range: Vec<u64>,
    pub estimated_write_time_ms: u64,
    pub space_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationSpec {
    pub status: ManifestStatus,
    pub confirmed_at: Option<u64>,
    pub blocks_written: u64,
    pub blocks_verified: u64,
}

/// The `allocation.dry` transaction manifest structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocationManifest {
    pub manifest_version: u32,
    pub manifest_id: Uuid,
    pub operation_type: OperationType,
    pub created: u64,
    pub reservation_id: Uuid,
    pub source: Option<SourceSpec>,
    pub destination: DestinationSpec,
    pub pre_operation_snapshot: Option<PreOperationSnapshot>,
    pub verification: VerificationSpec,
    pub confirmation: ConfirmationSpec,
}

impl AllocationManifest {
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }
}
