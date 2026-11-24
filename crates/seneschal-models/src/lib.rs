//! Central model registry, downloader and NeMo service status (issue #35).
//!
//! The catalog replaces the ad-hoc model URL lists spread across `install.sh`,
//! the docker weight scripts and the kokorox patch. The desktop model manager
//! consumes it through the Control API; downloads land in the standard per-OS
//! data directory and are checksum-verified before they count as installed.

pub mod catalog;
pub mod download;
pub mod manager;
pub mod nemo;
pub mod paths;

pub use catalog::{CATALOG, ModelBackend, ModelFile, ModelPurpose, ModelSpec};
pub use manager::{
    DownloadPhase, ManagerError, ModelCatalogWire, ModelEvent, ModelManager, ModelStateWire,
};
pub use paths::{legacy_roots, models_root, same_path};
