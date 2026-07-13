# custom-heater-strip

Design custom resistive heater strips as flex PCBs. Upload an SVG outline of
the strip you want, specify your electrical budget (supply voltage, target
wattage, current ceiling), and get back a fab-ready serpentine copper trace:

- **SVG preview** rendered in the browser
- **KiCad board file** (`.kicad_pcb`) to open and tweak
- **Gerber X2 set** (top copper, top soldermask terminal openings, silkscreen
  legend with the design's voltage/resistance/power and copper stackup, outline)
- **Design report** — target vs. achieved resistance, operating current and
  headroom, trace width/gap/length, power density

Built on the [meawoppl-rust-skeleton](https://github.com/meawoppl/meawoppl-rust-skeleton)
template: Yew WASM frontend embedded into an Axum backend as a single binary.

## Architecture

```
shared/      Serde types shared by both sides (DesignRequest/Response/Report)
engine/      The heater design engine (pure library, no I/O):
  outline.rs    SVG → outline polygon in mm (usvg, 96 dpi convention)
  solver.rs     electrical solve: R = V²/P → trace width/pitch quadratic,
                current-ceiling feasibility, post-route width refinement
  serpentine.rs boustrophedon scanline fill; rectangular, mitered (45°), or
                smooth (true-arc) turnarounds — arcs emit G02/G03 in Gerber
                and (arc …) tracks in KiCad
  preview.rs    SVG rendering of the routed design
  silk.rs       stroke-font silkscreen legend (specs printed on the board)
  kicad.rs      minimal .kicad_pcb writer (F.Cu segments + Edge.Cuts + F.SilkS)
  gerber.rs     RS-274X / X2 writer (X4.6 mm format)
frontend/    Yew app: upload, parameter form, preview, downloads
backend/     Axum server; POST /api/design runs the engine on spawn_blocking
```

### Physics

A serpentine of pitch `p = w + g` filling outline area `A` has length
`L ≈ A/p`, so its resistance is `R = ρA / ((w+g)·w·t)`. Given the target
`R = V²/P` this solves as a quadratic in `w`. After the actual path is routed
(edge margins, connectors, skipped rows change `L`), the width is re-solved
against the real length so the achieved resistance lands on target. Copper:
ρ = 1.724×10⁻⁸ Ω·m at 20 °C; 1 oz/ft² = 34.8 µm.

### Reuse from [pastebom.com](https://github.com/meawoppl/pastebom.com)

pastebom is a PCB *reader* (parse → neutral model → view), so its Gerber and
KiCad code is read-only — no writers to lift. What this project borrows:

- SVG emission conventions from `pcb-extract/src/svg.rs` (`M/L` d-strings,
  4-decimal coords, viewBox helpers)
- KiCad s-expression grammar/tag knowledge from `parsers/kicad{,_sexpr}.rs`
- Gerber X2 `%TF.FileFunction` vocabulary and X4.6 coordinate format from
  `parsers/gerber/` — our output round-trips through pastebom's reader, so
  generated boards can be uploaded there for visual checking
- Server patterns (size limits, `spawn_blocking` for CPU work)

## Quick start

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

docker compose up db -d          # Postgres (host port 5433)
cp .env.example .env
cd frontend && trunk build && cd ..
cargo run -p backend -- --dev-mode
# -> http://localhost:3000
```

## Outline SVG conventions

The UI can also synthesize a rectangle (width × height × corner radius)
client-side, so no SVG is needed for simple strips. For uploads:

- One closed `<path>` (or `<rect>`); the largest closed subpath is used as
  the outline
- Size the document in physical units (`width="100mm"`) — unitless SVGs are
  interpreted at 96 dpi with a warning
- Concave outlines route only the widest section per row (a warning tells you
  when this happens); holes/cutouts are not yet supported

## Current limitations / roadmap

- Single-span serpentine: complex concave shapes lose coverage
- No connection footprints in the KiCad output yet (terminal pads exist in
  the gerbers; endpoints are marked in the preview)
- No temperature-rise model (copper tempco means R climbs ~0.39%/°C as the
  heater warms — power at temperature will be below the 20 °C figure)
- Gerber output is not yet validated against a fab's CAM

## Fill patterns

Six patterns, all single continuous non-crossing paths at uniform pitch with
both ends at the terminal zone (see docs/fill-patterns.md for the research):

![fill pattern montage](docs/fill-patterns-montage.png)

- **Serpentine** — the classic; **Wavy** — sinusoidal rows for flex-fatigue
  life; **Counterflow** — bifilar out-and-back (non-inductive), built by
  offsetting a double-pitch serpentine ±p/2
- **Hilbert** — generalized (gilbert) space-filling curve, best thermal
  isotropy; rectangles only
- **Double spiral** — interleaved Archimedean arms, best for round boards
  (fills the inscribed circle)
- **Concentric** — outline insets (cavalier_contours) spliced through a
  channel at the left; best coverage of irregular outlines

## Example

100×20 mm strip, 12 V / 10 W / 2 A max, 0.5 oz copper →
14.40 Ω achieved, 0.83 A (42 % of ceiling), 0.286 mm trace × 4.16 m,
0.50 W/cm².

![example preview](docs/example-preview.svg)
