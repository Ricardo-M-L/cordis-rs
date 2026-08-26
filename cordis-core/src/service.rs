//! Service trait - the base contract for every Cordis service.
//!
//! Mirrors upstream `packages/core/src/service.ts`:
//!
//! - `name` is the unique service identifier under which the service is provided.
//! - `init` is the optional one-time initialization hook (upstream `symbols.init`),
//!   invoked by the framework when the owning fiber's runtime starts.
//! - `check` is the optional readiness probe (upstream `symbols.check`) that gates
//!   access to the service's value.
//! - `invoke` is the OPTIONAL callable-service protocol (upstream `symbols.invoke`):
//!   a service that implements it can be called like a function. Unlike the
//!   previous Rust port, it is not a required method returning a generic error -
//!   only services that opt in provide it.
//!
//! There is no `start()`/`stop()` contract upstream; lifecycle is expressed by
//! registering cleanup with the owning fiber's `effect()`.

use std::any::Any;

/// Every Cordis service implements `Service`.
pub trait Service: Send + Sync {
    /// The service's unique name.
    fn name(&self) -> &str;

    /// Initialization hook. Called once during framework bootstrap, mirroring
    /// upstream `symbols.init` invoked when the fiber runtime starts.
    fn init(&self) -> Result<(), String> {
        Ok(())
    }

    /// Health / consistency check, mirroring upstream `symbols.check`.
    fn check(&self) -> Result<(), String> {
        Ok(())
    }

    /// Optional callable-service protocol, mirroring upstream `symbols.invoke`.
    /// Services that implement it can be invoked like functions; the default
    /// implementation marks the service as not callable.
    fn invoke(
        &self,
        _args: &[Box<dyn Any + Send + Sync>],
    ) -> Option<Result<Box<dyn Any + Send + Sync>, String>> {
        None // not a callable service
    }
}
