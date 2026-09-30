//! jobwrap-web: Axum HTTP routes, middleware, WebSockets and the browser
//! interface.

#![forbid(unsafe_code)]

pub mod assets;
pub mod error;
pub mod events;
pub mod handlers;
mod job_details;
pub mod router;
pub mod service;

pub use error::{ApiError, HttpError};
pub use events::ServerEvent;
pub use router::{build_router, RouterState};
pub use service::{JobService, LaunchCapabilities, ServerInfo};
