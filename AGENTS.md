# AGENTS.md

## This project has no server

The heater engine is compiled to WebAssembly and runs in the browser; the site
is static files on GitHub Pages. There is no axum backend, no database, and no
container. Do not reintroduce one to solve a problem that can be solved in the
client — see `docs/frontend-only-plan.md` for why, and for what it would cost
to reverse.

Consequences worth knowing before you reach for a crate:

- **No static-asset-serving crate is needed.** Earlier guidance here
  recommended `memory-serve` for embedding assets in an axum server. That is
  obsolete: pre-compression, ETag negotiation and cache-control headers are
  server features, and Pages provides its own (gzip only, no brotli).
- **No response headers can be set.** GitHub Pages cannot send `COOP`/`COEP`,
  so `SharedArrayBuffer` and wasm threads are permanently unavailable. Web
  Workers communicating by `postMessage` are fine; anything needing
  `rayon`-style parallelism in wasm is not.
- **Everything must cross-compile to `wasm32-unknown-unknown`.** Check a new
  dependency with `cargo build -p engine --target wasm32-unknown-unknown`
  before designing around it, and weigh its bundle cost — first load is
  currently ~858 KB gzipped.

## The deployed URL is inherited, not chosen

The site lives at `https://www.cosmicfrontier.org/custom-heater-strip/`. That
is not configured here and cannot be changed here.

`cosmicfrontierlabs.github.io` — the org's *user site* repo — has
`cname = www.cosmicfrontier.org`. Once a user or org site has a custom domain,
GitHub `301`s **every** path under `<org>.github.io` to it, project pages
included, and there is no per-repo opt-out:

```
GET https://cosmicfrontierlabs.github.io/custom-heater-strip/
  -> 301 https://www.cosmicfrontier.org/custom-heater-strip/
```

This repo's own Pages config has `cname: null`. It is a fully independent
Pages site — own workflow, own artifact, own deployment — that merely serves
under a shared hostname.

Consequences that matter for the build:

- The site is served from a **subpath**, so the Pages workflow builds with
  `--public-url "/<repo>/"`. Assets built for `/` all 404.
- Routing uses `HashRouter` for the same reason (see `frontend/src/lib.rs` and
  `frontend/src/main.rs`).

Changing the hostname would take one of: giving *this* repo its own custom
domain (a DNS record plus `gh api ... /pages` with a `cname`), or moving the
repo to an account whose user site has no custom domain. Removing the org's
custom domain would take the main website down — not an option.

## Bundle size

Measure with `cd frontend && trunk build --release`, then gzip the `.wasm`.
`wasm-opt -Oz` already runs (via `data-wasm-opt="z"` in `index.html`), so the
built artifact is the real number. Beware measuring a size while the code path
you care about is dead — the linker will strip it and flatter you.

## Testing

`cargo test --workspace` covers the engine. For anything that only manifests in
a browser — wasm failing to initialise, an exception during mount, a click
handler that does the wrong thing — use `scripts/browser_drive.py`, which
drives the app over CDP and reports console errors and page exceptions that a
screenshot would hide.
