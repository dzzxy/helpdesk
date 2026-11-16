pub mod policy;
pub mod store;
pub mod identity;
pub mod pipe;
pub mod service;
pub mod management;

pub const SERVICE:&str="ApprovedLauncherRustBroker";
pub const PIPE:&str="ApprovedLauncherRust.v1";
pub const MAX_FRAME:usize=256*1024;
// Increment when the broker policy or service behavior changes. Manager-only UI
// changes must not restart an otherwise current backend while applications run.
pub const BACKEND_REVISION:u32=2;
