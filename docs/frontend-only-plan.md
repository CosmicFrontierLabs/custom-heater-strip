# Plan: make this a frontend-only app on GitHub Pages

Delete the server. Run the engine in the browser. Ship the whole thing as
static files.

This document is the plan of record for that transition. Every number in it
was measured on this repo, not estimated.

## Why it is possible

The hard prerequisite is already satisfied: **the engine compiles to
`wasm32-unknown-unknown` today, unmodified**, in both debug and release.
`usvg`, `dxf` (with its mandatory `image` dependency), and
`cavalier_contours` all cross-compile. The engine is a pure library with no
I/O and it lives up to that.

Three other things already point the same way:

- **Downloads are already client-side.** `frontend/src/main.rs` hands the
  browser `data:` URLs for the KiCad file and the base64 gerber zip. The
  server never serves a download.
- **Zip works in the browser.** `zip → flate2 → miniz_oxide` is pure Rust,
  no C shim, so gerber packaging moves client-side unchanged.
- **There is no state to lose.** Nothing is persisted (see the unused-database
  issue), so removing the server removes no capability.

What the server actually does today is: parse a DXF, run the engine, zip the
gerbers, and serve four static files. The first three are pure computation
that belongs wherever it is cheapest to run, and the fourth is what Pages is.

## Size budget

Measured with `trunk build --release`, before `wasm-opt`:

| Configuration | raw `.wasm` | gzipped |
| --- | --- | --- |
| Frontend alone (engine not linked) | 541 KB | — |
| **+ engine, `usvg` default features** | **2.04 MB** | **820 KB** |
| **+ engine, `usvg` without `text`** | **1.25 MB** | **509 KB** |

`usvg`'s default features pull in `fontdb`, `rustybuzz` and `unicode-bidi` for
text shaping. This project only reads path geometry out of an SVG, so
`default-features = false` drops **39 % of the raw bundle** and all 83 engine
tests still pass. That is a free win and should land regardless of the rest of
this plan.

509 KB gzipped is an acceptable first load for a CAD tool. Two further levers
if it needs to come down:

- **`wasm-opt -Oz`** — not yet applied to any number above. Trunk can fetch
  and run it; typically another 15–25 % off the raw size.
- **`image`, via an upstream feature gate.** `dxf` depends on `image`
  unconditionally, but only uses it in `thumbnail.rs`, for reading and writing
  the DXF preview thumbnail — a feature this project never touches. It is
  isolated to one module, so a `thumbnail` feature in `ixmilia/dxf-rs` is a
  small, well-formed upstream PR. Until then it is dead weight in the bundle.

**Gate the transition on a measured post-`wasm-opt` number.** It is the only
cost that could make this a bad trade, and it is cheap to check first.

## The responsiveness problem

This is the one piece of real engineering, and it must not be skipped.

The backend runs the engine on `tokio`'s `spawn_blocking` precisely because it
is CPU-bound. Moved naively onto the browser's main thread, a heavy fill
freezes the tab — no spinner, no input, no paint.

Measured spread on a 100 × 20 mm board:

| Pattern | Segments emitted |
| --- | --- |
| Serpentine | 89 |
| Concentric | 224 |
| Double spiral | 2 176 |
| Hilbert | 5 937 |
| **Wavy serpentine** | **21 247** |

A plain serpentine design round-trips in **24 ms** — imperceptible. But the
engine test suite takes ~45 s and is dominated by the large fills, so wavy
serpentine or Hilbert on a large board will be visibly janky on the main
thread.

**Therefore: run the engine in a Web Worker**, communicating by
`postMessage`. `gloo-worker` is already in the dependency tree.

Two constraints on the design:

- A plain worker needs **no** `COOP`/`COEP` headers, which matters because
  GitHub Pages cannot send any. Keep it to `postMessage`.
- `SharedArrayBuffer` and therefore wasm **threads are permanently
  unavailable** on Pages for the same reason. Do not design toward
  parallelising a fill with rayon; it cannot ship on this host.

The worker boundary also buys a real UX improvement over today: the design can
be cancelled, and the UI can show progress, neither of which the current
request/response shape allows.

## GitHub Pages specifics

- **Base URL.** Project sites are served from `/<repo>/`, not `/`. Trunk needs
  `--public-url /custom-heater-strip/` or every asset path 404s. This is the
  most common way a first Pages deploy fails.
- **Routing.** The app uses `yew_router`'s `BrowserRouter`, which needs a
  basename under a subpath and 404s on deep links. Switching to `HashRouter`
  fixes both at once and costs nothing here, since the app is effectively a
  single page. (Alternative: copy `index.html` to `404.html`.)
- **Compression.** Pages gzips but does **not** offer brotli, so the gzipped
  column above is the real over-the-wire number. This also makes the
  `memory-serve` recommendation in `AGENTS.md` moot — pre-compression and
  ETag negotiation are server features, and there will be no server.
- **MIME.** Pages serves `.wasm` as `application/wasm` correctly; nothing to do.

## Work breakdown

Ordered so each step is independently reviewable and leaves the app working.

1. **Trim `usvg`.** `default-features = false`. Verify the 83 engine tests and
   re-measure. Lands on its own; benefits the current architecture too.
2. **Move zip packaging into the engine.** Lift `zip_gerbers` out of
   `backend/src/handlers/design.rs` so `engine::generate` returns a complete
   `DesignResponse` with `gerber_zip_base64` populated. Removes the comment
   in `generate` that says the server fills this in. Backend keeps working.
3. **Add a wasm entry point.** `engine` as a frontend dependency, plus a thin
   `wasm-bindgen` surface for "design this request" and "parse this DXF".
4. **Build the worker.** `gloo-worker`, with the two calls from (3) behind it.
   Wire the UI to it and delete the two `fetch` calls. At the end of this step
   the app no longer talks to the server.
5. **Measure with `wasm-opt`.** Enable it in `Trunk.toml`, record the number.
   Decision point: if it is unacceptable, stop here — the app still works,
   and step 4 already removed the runtime dependency on the backend.
6. **Switch to `HashRouter`** and set `--public-url`.
7. **Delete the backend.** The whole `backend/` crate, the `Dockerfile`, the
   container workflow, the `db` service and volume in `docker-compose.yml`,
   `DATABASE_URL` in `.env`/`.env.example`, and the Postgres step in the
   README quick start. This subsumes the unused-database issue entirely —
   close it as resolved rather than fixing it separately.
8. **Add the Pages workflow.** `actions/deploy-pages`, building with
   `trunk build --release --public-url /custom-heater-strip/`.
9. **Rewrite the README.** Quick start becomes `trunk serve`. Add the live
   URL.

Steps 1–2 are safe to land immediately. Step 4 is the commitment point. Step 7
is irreversible in spirit, so it should follow a working deployed Pages build,
not precede it.

## What this gives up

Honestly: the ability to add anything server-shaped later without rebuilding
that half. Saved designs, share links, a design gallery, or a server-side CAM
check would all need a backend again. If any of those are actually wanted,
that changes the calculus — and the cheaper move is instead to keep the
backend but stop making Postgres a hard startup requirement.

If they are not wanted, this is strictly better: no service to run, no
database, no container, free hosting, and board outlines never leave the
user's machine — which is a genuine feature for proprietary geometry, not just
a cost saving.
