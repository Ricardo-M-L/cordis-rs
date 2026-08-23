//! Service trait — the base contract for every Cordis service.

use std::any::Any;

/// Every Cordis service implements `Service`.
///
/// - `name()` — unique service identifier.
/// - `init()` — one-time initialisation (called by the framework).
/// - `check()` — health / liveness check (called periodically or on demand).
/// - `invoke()` — optional runtime call; default is `Err("not implemented")`.
pub trait Service: Send + Sync {
    /// The service's unique name.
    fn name(&self) -> &str;

    /// Initialisation hook. Called once during framework bootstrap.
    fn init(&self) -> Result<(), String>;

    /// Health / consistency check.
    fn check(&self) -> Result<(), String>;

    /// Invoke the service with a variadic argument list.
    /// Override this method in concrete services to provide custom behaviour.
    /// Default returns an error indicating the service does not support invocation.
    fn invoke(&self, _args: &[Box<dyn Any>]) -> Result<Box<dyn Any>, String> {
        Err("invoke not implemented".to_string())
    }
}
