//! Cordis Core — the foundational module of the cordis framework.

pub mod context;
pub mod events;
pub mod fiber;
pub mod logger;
pub mod reflect;
pub mod registry;
pub mod service;
pub mod utils;

// Re-export key types for convenience
pub use context::CordisContext;
pub use events::{EventArgs, EventHandle, EventValue, EventsService};
pub use fiber::{disposer, Disposer, EffectorHandle, Fiber, FiberState};
pub use logger::{Exporter, LoggerLevel, LoggerService, Message};
pub use reflect::Reflect;
pub use registry::{Inject, Plugin, RegistryService};
pub use service::Service;
pub use utils::{DisposableList, Tracker};

#[cfg(test)]
mod tests;
