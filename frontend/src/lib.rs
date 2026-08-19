//! The engine task that runs off the main thread.
//!
//! Designing a heater is CPU-bound: a plain serpentine takes about 24 ms, but
//! the wavy serpentine emits over twenty thousand segments and Hilbert on a
//! large board is worse. Run on the main thread that freezes the tab — no
//! spinner, no input, no paint. So the engine lives in a Web Worker and the UI
//! talks to it by message.
//!
//! Deliberately **one** worker with a task enum rather than one worker per
//! call: a worker binary registers a single entry point, and one wasm module
//! shared by both operations keeps the second copy of the engine out of the
//! bundle.
//!
//! Plain `postMessage` only. GitHub Pages cannot send `COOP`/`COEP`, so
//! `SharedArrayBuffer` — and therefore wasm threads — are unavailable there;
//! see `docs/frontend-only-plan.md`.

use gloo_worker::oneshot::oneshot;
use serde::{Deserialize, Serialize};
use shared::{DesignRequest, DesignResponse, DxfUploadResponse};

/// Work the UI can hand to the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Task {
    /// Route a heater and produce every fab output.
    Design(Box<DesignRequest>),
    /// Pull the closed rings out of an uploaded DXF.
    ParseDxf(Vec<u8>),
}

/// What comes back. Engine errors cross the boundary as their display string,
/// since `EngineError` is not serialisable and the UI only shows the text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskDone {
    Design(Box<DesignResponse>),
    Dxf(Box<DxfUploadResponse>),
    Failed(String),
}

/// The worker entry point. `#[oneshot]` generates an `EngineTask` type with
/// `spawner()` for the app side and `registrar()` for the worker side.
#[oneshot]
pub async fn EngineTask(task: Task) -> TaskDone {
    match task {
        Task::Design(req) => match engine::generate(&req) {
            Ok(resp) => TaskDone::Design(Box::new(resp)),
            Err(e) => TaskDone::Failed(e.to_string()),
        },
        Task::ParseDxf(bytes) => match engine::dxf::extract(&bytes) {
            Ok(parsed) => TaskDone::Dxf(Box::new(parsed)),
            Err(e) => TaskDone::Failed(e.to_string()),
        },
    }
}

/// Where Trunk publishes the worker's loader shim.
///
/// The shim is two lines that `importScripts` the bindgen glue and point it at
/// the wasm; both of *its* paths are relative to itself, so the whole thing
/// keeps working under the `/<repo>/` prefix Pages serves from.
///
/// Trunk deliberately does not content-hash worker output — the name has to be
/// known at runtime to be passed to `new Worker` — so this is just the cargo
/// bin name with the shim suffix. Kept relative so it resolves against the
/// document base URL.
pub const WORKER_URL: &str = "./worker_loader.js";
