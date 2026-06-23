//! Shared pure-Rust HTTP transport (`reqwest` + `rustls-tls`).
//!
//! Single source of truth for the production [`traits::http::HttpTransport`]
//! used by every host (desktop, windows, mobile). Replaces the formerly
//! duplicated `platforms/{common,windows,posix}` copies.

pub mod reqwest_http;

pub use reqwest_http::ReqwestHttp;
