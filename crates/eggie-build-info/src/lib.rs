//! One build identity shared by GUI, CLI and daemon handshake.
pub const BUILD_ID: &str = env!("EGGIE_BUILD_ID");
pub const BUILD_DATE: &str = env!("EGGIE_BUILD_DATE");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
