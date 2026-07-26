//! Host-independent network adapters shared by desktop and headless hosts.
//!
//! This crate owns wire protocols and bounded request validation. It never
//! starts an Apple-device runtime, reads process configuration, or binds a
//! listener; hosts inject an existing runtime client and explicit settings.

pub mod http;
pub mod mcp;
pub mod status;
pub mod websocket;
