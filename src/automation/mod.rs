//! automation: other programs driving a running kuterm. the unix socket answers json requests,
//! `kuterm msg` is its command line client, and the event bus streams what happens in panes to
//! subscribed clients

pub mod events;
pub mod msg;
pub mod protocol;
pub mod socket;

pub use protocol::{BlockInfo, Reply, Request, TabInfo};
