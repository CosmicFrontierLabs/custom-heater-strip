//! The Web Worker binary: registers the engine task and then just serves
//! requests. Trunk builds this as a separate wasm module via
//! `<link data-trunk rel="rust" data-bin="worker" data-type="worker" />`.

use gloo_worker::Registrable;

fn main() {
    frontend::EngineTask::registrar().register();
}
