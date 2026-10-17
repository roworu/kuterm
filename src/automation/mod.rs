//! automation: other programs driving a running kuterm. the unix socket answers json requests,
//! `kuterm msg` is its command line client, `kuterm mcp` serves the same requests to ai agents
//! over the model context protocol, and the event bus streams what happens in panes to both

pub mod events;
pub mod mcp;
pub mod msg;
pub mod protocol;
pub mod socket;

pub use protocol::{BlockInfo, Reply, Request, TabInfo};
