//! Marketplace manager — stub.
//!
//! Plan 16 wires the marketplace fetch loop (HTTP listing, signed manifest
//! verification, download, `.mcpb` unpacking). M1.21 ships the contract
//! shape only.

/// Placeholder for the marketplace fetch coordinator.
///
/// Plan 16 grows this into a struct holding the registry endpoints,
/// the install directory, and the http transport.
pub struct MarketplaceManager;
