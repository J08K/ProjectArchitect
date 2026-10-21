//! The MCP tool table as a library, shared by the `pa-mcp` server and the `pa-cli` command line (D-47).

pub mod tools;

use pa_core::Store;
use std::path::PathBuf;

/// What the tools run on: the store opened once at start, and the data folder (for backups).
pub struct Server {
    pub store: Store,
    pub data_dir: PathBuf,
}
