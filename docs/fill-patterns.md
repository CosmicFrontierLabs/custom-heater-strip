# Fill pattern design notes

Research summary (2026-07-13) on borrowing 3D-printing infill ideas for
heater traces. Klipper itself generates no infill — it's firmware; fill
patterns live in slicers. The useful prior art is PrusaSlicer/Slic3r
(`src/libslic3r/Fill/`) and CuraEngine (`src/infill/`).

## What a heater trace needs (vs. printer infill)

1. One single continuous path — no crossings, no disconnected islands
2. Uniform pitch → uniform W/cm² power density
3. Both endpoints at the terminal zone (adjacent pads)
4. Total length is the resistance target — the solver picks pitch/width

Most slicer patterns fail (1): grid/triangles/stars/cubic are
self-crossing multi-sweep families, lightning is a branching tree,
honeycomb double-traces shared walls (4× local power). The usable ideas
are rectilinear (our serpentine), the `Math::PlanePath` curve family
(Hilbert, Archimedean spiral), concentric offsets, and gyroid waves.

## The slicer architecture worth copying

Every PrusaSlicer pattern implements one hook on a `Fill` base class:
polygon region + spacing/density/angle in → open polylines out, with a
shared **generate → clip → reconnect** pipeline (generate the unbounded
pattern over the bbox, Clipper-intersect with the region, re-join the
fragments by walking short arcs of the boundary — `Fill::connect_infill`).
Our equivalent: a `FillPattern` trait in the engine,

```rust
trait FillPattern {
    fn fill(&self, outline: &Polygon, pitch_mm: f64, inset_mm: f64,
            left_reserved_mm: f64, style: CornerStyle,
            warnings: &mut Vec<String>) -> Result<Vec<PathSeg>, EngineError>;
}
```

with today's serpentine as the first implementation and a `FillKind` enum
in `shared` for the UI dropdown.

## The bifilar offset trick (key insight)

Given ANY single open path filling the region at pitch 2p, the outline of
that path inflated by p/2 is a closed loop; cut it at the path's start and
you get a single non-crossing trace at exact pitch p with **both endpoints
adjacent at the cut**, counterflowing current everywhere (non-inductive).
One polygon-offset operation converts any pattern below into an ideal
heater layout. Needs a polygon offsetting dependency
(`cavalier_contours` or `i_overlay`).

## Ranked pattern shortlist

1. **Counterflow serpentine (bifilar boustrophedon)** — serpentine at
   pitch 2p, out on even rows, hairpin, back on odd rows. Single path,
   exact pitch, endpoints adjacent (pairs perfectly with our terminal
   zone), non-inductive. The classic industrial heater layout. Can be
   built directly on the existing row scanner — no new dependencies.
2. **Hilbert / gilbert curve** — best thermal isotropy; FEA literature
   (MRS Advances 2020) shows Hilbert/Moore beat double spirals on
   temperature uniformity per unit metal. Use the generalized-rectangle
   "gilbert" algorithm (github.com/jakubcerveny/gilbert, ~100 lines,
   easy Rust port) instead of power-of-two Hilbert. Endpoints land at two
   corners of one edge — use the Moore variant (closed loop, cut
   anywhere) or the bifilar trick for adjacency. Non-rectangular
   outlines need clip-and-reconnect.
3. **Double Archimedean spiral** — two interleaved arms joined by a
   U-turn at center; both terminals exit adjacent at the outer edge.
   Ideal for round/convex heaters; weak for long skinny strips.
4. **Concentric with connectors** — repeated polygon inset at pitch,
   rings spliced through a radial slit. Best boundary following of all;
   needs polygon offsetting.
5. **Wavy serpentine (gyroid-inspired)** — sinusoidal perturbation
   (amplitude < gap/2) on serpentine rows. Same electrical behavior, but
   meandered traces have much better flex-fatigue life and kill the
   stiff straight-line axis — genuinely useful on flex.

## Prior art

- github.com/steltze/KiCad-Heater-Generator-Plugin — gilbert-curve heater
  tracks in KiCad; rectangles only, no resistance targeting
- github.com/bliepp/PCB-Heater-Generator, github.com/Trilys/PCB_Heater_KiCad
  — serpentine generators for KiCad
- "Design of Heating Coils Based on Space-Filling Fractal Curves",
  MRS Advances 2020 — FEA: Hilbert-4/Moore-4/Peano-3 beat double spiral
- Tech Explorations flex-heater guide: radius corners ≥1.5–2× trace width
  to avoid hot spots (our Smooth corner style already does this)
