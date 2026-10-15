mod auth;
mod base;
mod compose;
mod error;
mod events;
mod facts;
mod http;
mod output;
mod packages;
mod pass;
mod route;
mod scopes;
mod tables;

pub use error::{LinkError, LinkResult};

pub const LINK_VERSION: u32 = 8;
pub const ATTR_CLIENT_CALL: &str = "client_call";
pub const ATTR_COMPOSED_FROM: &str = "composed_from";
pub use pass::{LinkOutput, PASS_NAME, link};
