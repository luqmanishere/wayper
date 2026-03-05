//! Library for common code
// TODO: actually only common code, so don't bloat up unrelated binaries
pub mod config;
pub mod event_source;
#[cfg(target_os = "linux")]
pub mod socket;
