//! Cordis Core — the foundational module of the cordis framework.

pub mod context;
pub mod events;
pub mod fiber;
pub mod logger;
pub mod registry;
pub mod reflect;
pub mod service;
pub mod utils;

// Re-export key types for convenience
pub use context::CordisContext;
pub use events::EventsService;
pub use fiber::{EffectorHandle, Fiber, FiberState};
pub use logger::{Exporter, LoggerLevel, LoggerService, Message};
pub use registry::{Inject, Plugin, RegistryService};
pub use reflect::Reflect;
pub use service::Service;
pub use utils::{DisposableList, Tracker};

#[cfg(test)]
mod tests;
