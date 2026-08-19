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

All figures are `trunk build --release`, which **already runs `wasm-opt -Oz`**
— `data-wasm-opt="z"` is set in `index.html` and Trunk fetches the tool
itself. There is no further optimisation pass waiting to be turned on.

| Configuration | raw `.wasm` | gzipped |
| --- | --- | --- |
| Frontend alone, engine dead-stripped | 541 KB | — |
| Design path only, `usvg` default features | 2.04 MB | 820 KB |
| Design path only, `usvg` without `text` | 1.25 MB | 509 KB |
| **Both paths live, `usvg` without `text`** | **2.15 MB** | **858 KB** |
| Both paths live, and `image` patched out of `dxf` | 2.10 MB | 835 KB |

Read that table carefully, because two of the rows are traps.

**`usvg` without `text` is a real win.** Its default features pull in
`fontdb`, `rustybuzz` and `unicode-bidi` to shape text; this project only
reads path geometry, so `default-features = false` costs nothing and saves
**790 KB raw / 311 KB gzipped**. Landed in step 1.

**The 509 KB row is not achievable.** It was measured when nothing yet called
`engine::dxf`, so the linker dropped the entire DXF reader. Once the DXF
upload path is live — which is the whole point — the honest number is
**858 KB gzipped**. Anything quoting 509 KB is quoting a build with the
feature compiled out.

**Patching `image` out of `dxf` is not worth doing.** `dxf` depends on `image`
unconditionally and only uses it in `thumbnail.rs`, so an upstream feature
gate looked like an easy win. Measured by vendoring `dxf` with the dependency
and thumbnail module surgically removed: it saves **24 KB gzipped**. The
linker was already stripping almost all of `image`, because nothing reaches
the thumbnail path. Not worth an upstream PR.

So the ~850 KB raw that the DXF path costs is almost entirely the `dxf`
crate's own code-generated entity reader — hundreds of entity structs with
their read and write implementations. That is inherent to using the crate, and
the only way around it would be writing a minimal DXF reader for the handful
of entities this project actually wants. Not recommended: the bulge, unit and
spline handling is exactly the fiddly part worth having someone else maintain.

**858 KB gzipped is the number to accept or reject.** For a niche engineering
tool loaded occasionally it is defensible — comparable to a mid-sized JS
SPA — and it buys a tool with no server, no database and no upload of the
user's geometry.

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

1. ~~**Trim `usvg`.**~~ Done. `default-features = false`; 790 KB raw saved.
2. ~~**Move zip packaging into the engine.**~~ Done. `engine::generate` now
   returns a complete `DesignResponse`, so no caller has to finish assembling
   one.
3. ~~**Call the engine directly from the frontend.**~~ Done. Both `fetch`
   calls deleted; verified against a plain static file server with the
   backend stopped. The app no longer talks to a server at all.
4. **Move the engine into a Web Worker.** The responsiveness fix. Deliberately
   *after* step 3 rather than merged into it: step 3 makes the app correct
   without a server, step 4 makes it pleasant. Splitting them keeps a
   two-binary Trunk build and worker message plumbing out of the change that
   had to be verified for numerical parity.
5. ~~**Measure.**~~ Done: **858 KB gzipped**, post-`wasm-opt -Oz`, with both
   code paths live. Accepted as the first-load cost. Gate passed.
6. **Switch to `HashRouter`** and set `--public-url`.
7. **Delete the backend.** The whole `backend/` crate, the `Dockerfile`, the
   container workflow, the `db` service and volume in `docker-compose.yml`,
   `DATABASE_URL` in `.env`/`.env.example`, and the Postgres step in the
   README quick start. This subsumes the unused-database issue entirely —
   close it as resolved rather than fixing it separately.
8. **Add the Pages workflow.** `actions/deploy-pages`, building with
   `trunk build --release --public-url /custom-heater-strip/`.
9. **Rewrite the README.** Quick start becomes `trunk serve`. Add the live URL.

Step 3 was the commitment point and it has passed. Step 4 is independent of
6–9 and can land in either order. Step 7 deletes working code, so it should
follow a Pages build that is actually serving, not precede it.

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
