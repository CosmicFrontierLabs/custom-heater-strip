use shared::{
    DesignRequest, DesignResponse, DxfPolygon, DxfUploadResponse, GeometrySpec, PolygonRole,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

/// Where the board geometry comes from.
#[derive(Clone, Copy, PartialEq)]
enum GeometryMode {
    /// Upload an SVG whose largest closed path is the outline.
    Svg,
    /// Synthesise a rectangle client-side.
    Rect,
    /// Upload a DXF and assign roles to its polygons.
    Dxf,
}

#[derive(Clone, Routable, PartialEq)]
enum Route {
    #[at("/")]
    Home,
    #[not_found]
    #[at("/404")]
    NotFound,
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! { <Designer /> },
        Route::NotFound => html! { <h1>{ "404 - Not Found" }</h1> },
    }
}

#[function_component(App)]
pub fn app() -> Html {
    // HashRouter, not BrowserRouter: this ships as static files on GitHub
    // Pages, served from /<repo>/ rather than the domain root. A path router
    // would need a basename and would 404 on any deep link, because Pages has
    // no server to rewrite unknown paths back to index.html.
    html! {
        <HashRouter>
            <Switch<Route> render={switch} />
        </HashRouter>
    }
}

/// One numeric parameter input bound to a state handle.
#[derive(Properties, PartialEq)]
struct NumFieldProps {
    label: AttrValue,
    unit: AttrValue,
    value: f64,
    step: f64,
    onchange: Callback<f64>,
}

#[function_component(NumField)]
fn num_field(props: &NumFieldProps) -> Html {
    let onchange = {
        let cb = props.onchange.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            if let Ok(v) = input.value().parse::<f64>() {
                cb.emit(v);
            }
        })
    };
    html! {
        <label class="field">
            <span class="field-label">{ props.label.clone() }</span>
            <span class="field-input">
                <input type="number" step={props.step.to_string()}
                       value={format!("{}", props.value)} onchange={onchange} />
                <span class="unit">{ props.unit.clone() }</span>
            </span>
        </label>
    }
}

/// Interactive plan view of an uploaded DXF: click a polygon to cycle what it
/// is used for.
#[derive(Properties, PartialEq)]
struct PickerProps {
    polygons: Vec<DxfPolygon>,
    roles: Vec<PolygonRole>,
    on_pick: Callback<usize>,
}

#[function_component(PolygonPicker)]
fn polygon_picker(props: &PickerProps) -> Html {
    // Frame the whole drawing with a small margin.
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in props.polygons.iter().flat_map(|p| p.points.iter()) {
        min_x = min_x.min(p[0]);
        min_y = min_y.min(p[1]);
        max_x = max_x.max(p[0]);
        max_y = max_y.max(p[1]);
    }
    if !min_x.is_finite() {
        return html! {};
    }
    let margin = ((max_x - min_x).max(max_y - min_y) * 0.03).max(1.0);
    let (vx, vy) = (min_x - margin, min_y - margin);
    let (vw, vh) = (max_x - min_x + 2.0 * margin, max_y - min_y + 2.0 * margin);
    // Keep hairlines visible whatever the board's real size.
    let stroke = (vw.max(vh) / 400.0).max(0.05);

    html! {
        <svg class="picker" viewBox={format!("{vx:.3} {vy:.3} {vw:.3} {vh:.3}")}
             xmlns="http://www.w3.org/2000/svg">
            // Polygons arrive largest-first, so painting in order puts small
            // tabs on top of the regions containing them — and therefore
            // makes them the ones that receive the click.
            { for props.polygons.iter().enumerate().map(|(i, poly)| {
                let role = props.roles.get(i).copied().unwrap_or_default();
                let on_pick = props.on_pick.clone();
                let onclick = Callback::from(move |_: MouseEvent| on_pick.emit(i));
                let used = role != PolygonRole::Unused;
                html! {
                    <path key={poly.id}
                          d={ring_d(&poly.points)}
                          fill={role.color()}
                          fill-opacity={if used { "0.35" } else { "0.08" }}
                          stroke={role.color()}
                          stroke-width={format!("{:.4}", if used { stroke * 2.0 } else { stroke })}
                          stroke-dasharray={if used { "none".to_string() } else { format!("{:.3} {:.3}", stroke * 4.0, stroke * 3.0) }}
                          onclick={onclick}>
                        <title>{ format!("{} · {} · {:.1} mm² — click to change",
                                         poly.layer, poly.kind, poly.area_mm2) }</title>
                    </path>
                }
            }) }
        </svg>
    }
}

/// Closed SVG `d` string for a ring.
fn ring_d(points: &[[f64; 2]]) -> String {
    let mut d = String::with_capacity(points.len() * 18);
    for (i, p) in points.iter().enumerate() {
        d.push(if i == 0 { 'M' } else { 'L' });
        d.push_str(&format!("{:.4} {:.4}", p[0], p[1]));
    }
    d.push('Z');
    d
}

/// Roles that only one polygon may hold at a time.
fn is_singular(role: PolygonRole) -> bool {
    matches!(
        role,
        PolygonRole::TabIn | PolygonRole::TabOut | PolygonRole::Outline
    )
}

/// Turn the role assignment into engine geometry, or explain what is missing.
fn build_geometry(polygons: &[DxfPolygon], roles: &[PolygonRole]) -> Result<GeometrySpec, String> {
    let mut spec = GeometrySpec::default();
    for (poly, role) in polygons.iter().zip(roles.iter()) {
        let ring = poly.points.clone();
        match role {
            PolygonRole::Heater => spec.heaters.push(ring),
            PolygonRole::TabIn => spec.tab_in = Some(ring),
            PolygonRole::TabOut => spec.tab_out = Some(ring),
            PolygonRole::Outline => spec.outline = Some(ring),
            PolygonRole::Unused => {}
        }
    }
    if spec.heaters.is_empty() {
        return Err("Click at least one polygon to mark it as a heater region.".into());
    }
    match (&spec.tab_in, &spec.tab_out) {
        (Some(_), Some(_)) => Ok(spec),
        (None, Some(_)) => Err("Mark a polygon as the input solder tab.".into()),
        (Some(_), None) => Err("Mark a polygon as the output solder tab.".into()),
        (None, None) => Err("Mark two polygons as the input and output solder tabs.".into()),
    }
}

#[function_component(Designer)]
fn designer() -> Html {
    let svg_text = use_state(|| None::<(String, String)>); // (filename, contents)
    let voltage = use_state(|| 12.0);
    let watts = use_state(|| 10.0);
    let max_current = use_state(|| 2.0);
    let copper_oz = use_state(|| 0.5);
    let min_trace = use_state(|| 0.15);
    let min_gap = use_state(|| 0.15);
    let edge_margin = use_state(|| 0.5);
    let pad_diameter = use_state(|| 2.5);
    let corner_style = use_state(shared::CornerStyle::default);
    let fill_kind = use_state(shared::FillKind::default);
    // Fab process floor for the trace-width slider; set by the preset picker.
    let fab_floor = use_state(|| 0.05_f64);
    // Where the geometry comes from: SVG upload, parametric rectangle, or a
    // DXF whose polygons the user tags by role.
    let mode = use_state(|| GeometryMode::Svg);
    let rect_w = use_state(|| 100.0_f64);
    let rect_h = use_state(|| 20.0_f64);
    let dxf = use_state(|| None::<DxfUploadResponse>);
    let roles = use_state(Vec::<PolygonRole>::new);
    let result = use_state(|| None::<DesignResponse>);
    let error = use_state(|| None::<String>);
    let busy = use_state(|| false);

    let on_file = {
        let svg_text = svg_text.clone();
        let error = error.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            let Some(file) = input.files().and_then(|fs| fs.get(0)) else {
                return;
            };
            let name = file.name();
            let svg_text = svg_text.clone();
            let error = error.clone();
            spawn_local(async move {
                match gloo_file::futures::read_as_text(&gloo_file::File::from(file)).await {
                    Ok(text) => svg_text.set(Some((name, text))),
                    Err(e) => error.set(Some(format!("Could not read file: {e}"))),
                }
            });
        })
    };

    // DXF upload: parse server-side, then seed each polygon's role from the
    // layer-name guess so a conventionally-named file arrives ready to go.
    let on_dxf_file = {
        let (dxf, roles, error, busy) = (dxf.clone(), roles.clone(), error.clone(), busy.clone());
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            let Some(file) = input.files().and_then(|fs| fs.get(0)) else {
                return;
            };
            let (dxf, roles, error, busy) =
                (dxf.clone(), roles.clone(), error.clone(), busy.clone());
            busy.set(true);
            spawn_local(async move {
                match gloo_file::futures::read_as_bytes(&gloo_file::File::from(file)).await {
                    // The engine runs here in the browser; the bytes never
                    // leave the machine.
                    Ok(bytes) => match engine::dxf::extract(&bytes) {
                        Ok(d) => {
                            roles.set(d.polygons.iter().map(|p| p.suggested_role).collect());
                            dxf.set(Some(d));
                            error.set(None);
                        }
                        Err(e) => error.set(Some(e.to_string())),
                    },
                    Err(e) => error.set(Some(format!("Could not read file: {e}"))),
                }
                busy.set(false);
            });
        })
    };

    // Clicking a polygon advances its role. The tab and outline roles are
    // singular, so taking one clears whoever held it before — that way the
    // selection is always valid by construction.
    let on_pick = {
        let roles = roles.clone();
        Callback::from(move |i: usize| {
            let mut next = (*roles).clone();
            if i >= next.len() {
                return;
            }
            let assigned = next[i].next();
            if is_singular(assigned) {
                for (j, r) in next.iter_mut().enumerate() {
                    if j != i && *r == assigned {
                        *r = PolygonRole::Unused;
                    }
                }
            }
            next[i] = assigned;
            roles.set(next);
        })
    };

    let on_generate = {
        let svg_text = svg_text.clone();
        let (voltage, watts, max_current) = (voltage.clone(), watts.clone(), max_current.clone());
        let (copper_oz, min_trace, min_gap, edge_margin) = (
            copper_oz.clone(),
            min_trace.clone(),
            min_gap.clone(),
            edge_margin.clone(),
        );
        let (pad_diameter, corner_style, fill_kind) = (
            pad_diameter.clone(),
            corner_style.clone(),
            fill_kind.clone(),
        );
        let (mode, rect_w, rect_h) = (mode.clone(), rect_w.clone(), rect_h.clone());
        let (dxf, roles) = (dxf.clone(), roles.clone());
        let result = result.clone();
        let error = error.clone();
        let busy = busy.clone();
        Callback::from(move |_: MouseEvent| {
            // In DXF mode the polygon roles carry the geometry and `svg` goes
            // unused; the other two modes supply an outline the engine parses.
            let mut geometry = None;
            let svg = match *mode {
                GeometryMode::Rect => {
                    let (w, h) = (*rect_w, *rect_h);
                    format!(
                        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}mm" height="{h}mm" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}"/></svg>"##
                    )
                }
                GeometryMode::Svg => match (*svg_text).clone() {
                    Some((_, svg)) => svg,
                    None => {
                        error.set(Some("Upload an SVG outline first.".into()));
                        return;
                    }
                },
                GeometryMode::Dxf => {
                    let Some(parsed) = (*dxf).clone() else {
                        error.set(Some("Upload a DXF first.".into()));
                        return;
                    };
                    match build_geometry(&parsed.polygons, &roles) {
                        Ok(spec) => geometry = Some(spec),
                        Err(msg) => {
                            error.set(Some(msg));
                            return;
                        }
                    }
                    String::new()
                }
            };
            let req = DesignRequest {
                svg,
                voltage: *voltage,
                watts: *watts,
                max_current: *max_current,
                copper_oz: *copper_oz,
                min_trace_mm: *min_trace,
                min_gap_mm: *min_gap,
                edge_margin_mm: *edge_margin,
                pad_diameter_mm: *pad_diameter,
                corner_style: *corner_style,
                fill_kind: *fill_kind,
                geometry,
            };
            let result = result.clone();
            let error = error.clone();
            let busy = busy.clone();
            busy.set(true);
            // Yield once so the "Generating…" state paints before the engine
            // takes the thread. Heavy fills still block it; that is what the
            // Web Worker step of docs/frontend-only-plan.md fixes.
            spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(0).await;
                match engine::generate(&req) {
                    Ok(d) => {
                        error.set(None);
                        result.set(Some(d));
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        })
    };

    let on_preset = {
        let (copper_oz, min_trace, min_gap, fab_floor) = (
            copper_oz.clone(),
            min_trace.clone(),
            min_gap.clone(),
            fab_floor.clone(),
        );
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            if let Some(p) = shared::FAB_PRESETS
                .iter()
                .find(|p| p.name == select.value())
            {
                copper_oz.set(p.copper_oz);
                min_trace.set(p.min_trace_mm);
                min_gap.set(p.min_gap_mm);
                fab_floor.set(p.min_trace_mm);
            } else {
                fab_floor.set(0.05);
            }
        })
    };

    let on_trace_slider = {
        let (min_trace, fab_floor) = (min_trace.clone(), fab_floor.clone());
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            if let Ok(v) = input.value().parse::<f64>() {
                min_trace.set(v.max(*fab_floor));
            }
        })
    };

    let on_fill_kind = {
        let fill_kind = fill_kind.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            if let Some(k) = shared::FillKind::ALL
                .iter()
                .find(|k| k.label() == select.value())
            {
                fill_kind.set(*k);
            }
        })
    };

    let on_corner = {
        let corner_style = corner_style.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            if let Some(c) = shared::CornerStyle::ALL
                .iter()
                .find(|c| c.label() == select.value())
            {
                corner_style.set(*c);
            }
        })
    };

    let set_mode = |to: GeometryMode| {
        let mode = mode.clone();
        Callback::from(move |_: Event| mode.set(to))
    };

    html! {
        <div class="designer">
            <h1>{ "Custom Heater Strip" }</h1>
            <p class="tagline">{ "Upload a flex outline, set your electrical budget, get a fab-ready serpentine heater." }</p>

            <div class="panel">
                <h2>{ "1 · Geometry" }</h2>
                <div class="mode-row">
                    { for [
                        (GeometryMode::Svg, "Upload SVG"),
                        (GeometryMode::Rect, "Rectangle"),
                        (GeometryMode::Dxf, "Upload DXF"),
                    ].into_iter().map(|(m, label)| html! {
                        <label>
                            <input type="radio" name="geometry-mode" checked={*mode == m}
                                   onchange={set_mode(m)} />
                            { format!(" {label}") }
                        </label>
                    }) }
                </div>
                { match *mode {
                    GeometryMode::Rect => html! {
                        <div class="fields">
                            <NumField label="Width" unit="mm" value={*rect_w} step={1.0}
                                onchange={let v = rect_w.clone(); Callback::from(move |x| v.set(x))} />
                            <NumField label="Height" unit="mm" value={*rect_h} step={1.0}
                                onchange={let v = rect_h.clone(); Callback::from(move |x| v.set(x))} />
                        </div>
                    },
                    GeometryMode::Svg => html! {
                        <>
                            <input type="file" accept=".svg,image/svg+xml" onchange={on_file} />
                            { match (*svg_text).as_ref() {
                                Some((name, text)) => html! {
                                    <span class="file-ok">{ format!("{name} ({} bytes)", text.len()) }</span>
                                },
                                None => html! { <span class="file-hint">{ "SVG with a closed path, sized in mm" }</span> },
                            }}
                        </>
                    },
                    GeometryMode::Dxf => html! {
                        <>
                            <input type="file" accept=".dxf" onchange={on_dxf_file} />
                            { match (*dxf).as_ref() {
                                None => html! {
                                    <span class="file-hint">
                                        { "DXF with closed outlines — polylines, circles, ellipses" }
                                    </span>
                                },
                                Some(parsed) => html! {
                                    <>
                                        <span class="file-ok">
                                            { format!("{} polygon{} · units: {}",
                                                      parsed.polygons.len(),
                                                      if parsed.polygons.len() == 1 { "" } else { "s" },
                                                      parsed.units) }
                                        </span>
                                        { for parsed.warnings.iter().map(|w| html! {
                                            <div class="warning">{ w }</div>
                                        }) }
                                        <p class="field-label">
                                            { "Click a polygon to change what it is used for." }
                                        </p>
                                        <div class="legend">
                                            { for PolygonRole::ALL.iter().map(|r| {
                                                let n = roles.iter().filter(|x| *x == r).count();
                                                html! {
                                                    <span class="legend-item">
                                                        <span class="swatch"
                                                              style={format!("background:{}", r.color())} />
                                                        { format!("{} ({n})", r.label()) }
                                                    </span>
                                                }
                                            }) }
                                        </div>
                                        <PolygonPicker
                                            polygons={parsed.polygons.clone()}
                                            roles={(*roles).clone()}
                                            on_pick={on_pick.clone()} />
                                        { match build_geometry(&parsed.polygons, &roles) {
                                            Ok(_) => html!{},
                                            Err(msg) => html! { <div class="warning">{ msg }</div> },
                                        }}
                                    </>
                                },
                            }}
                        </>
                    },
                } }
            </div>

            <div class="panel">
                <h2>{ "2 · Electrical" }</h2>
                <div class="fields">
                    <NumField label="Supply voltage" unit="V" value={*voltage} step={0.1}
                        onchange={let v = voltage.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Target power" unit="W" value={*watts} step={0.5}
                        onchange={let v = watts.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Current ceiling" unit="A" value={*max_current} step={0.1}
                        onchange={let v = max_current.clone(); Callback::from(move |x| v.set(x))} />
                </div>
                <h2>{ "3 · Fab limits" }</h2>
                <label class="field">
                    <span class="field-label">{ "Fab preset" }</span>
                    <select onchange={on_preset}>
                        <option selected=true>{ "Custom" }</option>
                        { for shared::FAB_PRESETS.iter().map(|p| html! {
                            <option>{ p.name }</option>
                        }) }
                    </select>
                </label>
                <div class="fields">
                    <NumField label="Copper weight" unit="oz" value={*copper_oz} step={0.5}
                        onchange={let v = copper_oz.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Min gap" unit="mm" value={*min_gap} step={0.05}
                        onchange={let v = min_gap.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Edge margin" unit="mm" value={*edge_margin} step={0.1}
                        onchange={let v = edge_margin.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Solder pad" unit="mm" value={*pad_diameter} step={0.5}
                        onchange={let v = pad_diameter.clone(); Callback::from(move |x| v.set(x))} />
                    <label class="field">
                        <span class="field-label">{ "Fill pattern" }</span>
                        <select onchange={on_fill_kind}>
                            { for shared::FillKind::ALL.iter().map(|k| html! {
                                <option selected={*k == *fill_kind}>{ k.label() }</option>
                            }) }
                        </select>
                    </label>
                    <label class="field">
                        <span class="field-label">{ "Corners" }</span>
                        <select onchange={on_corner}>
                            { for shared::CornerStyle::ALL.iter().map(|c| html! {
                                <option selected={*c == *corner_style}>{ c.label() }</option>
                            }) }
                        </select>
                    </label>
                </div>
                <label class="field slider-field">
                    <span class="field-label">
                        { format!("Min trace width: {:.2} mm (fab floor {:.2} mm)", *min_trace, *fab_floor) }
                    </span>
                    <input type="range"
                           min={format!("{:.2}", *fab_floor)}
                           max="1.00"
                           step="0.01"
                           value={format!("{}", *min_trace)}
                           oninput={on_trace_slider} />
                </label>
                <button class="generate" onclick={on_generate} disabled={*busy}>
                    { if *busy { "Generating…" } else { "Generate heater" } }
                </button>
            </div>

            { if let Some(msg) = (*error).as_ref() {
                html! { <div class="error">{ msg }</div> }
            } else { html!{} } }

            { if let Some(resp) = (*result).as_ref() {
                render_result(resp)
            } else { html!{} } }
        </div>
    }
}

fn render_result(resp: &DesignResponse) -> Html {
    let r = &resp.report;
    let preview = Html::from_html_unchecked(AttrValue::from(resp.preview_svg.clone()));

    let downloads: Vec<(String, String, String)> = vec![
        (
            "heater.kicad_pcb".to_string(),
            "KiCad board".to_string(),
            data_url(&resp.kicad_pcb),
        ),
        (
            "heater-gerbers.zip".to_string(),
            format!("Gerber set, {} layers", resp.gerbers.len()),
            format!("data:application/zip;base64,{}", resp.gerber_zip_base64),
        ),
    ];

    html! {
        <>
        <div class="panel preview-panel">
            <h2>{ "Preview" }</h2>
            <div class="preview">{ preview }</div>
        </div>
        <div class="panel">
            <h2>{ "Design report" }</h2>
            { for r.warnings.iter().map(|w| html! { <div class="warning">{ w }</div> }) }
            <table class="report">
                <tbody>
                    <tr><td>{ "Resistance (target → achieved)" }</td>
                        <td>{ format!("{:.2} Ω → {:.2} Ω", r.target_resistance_ohms, r.achieved_resistance_ohms) }</td></tr>
                    <tr><td>{ "Power at supply" }</td>
                        <td>{ format!("{:.2} W", r.achieved_watts) }</td></tr>
                    <tr><td>{ "Operating current" }</td>
                        <td>{ format!("{:.2} A ({:.0}% of ceiling)", r.operating_current_amps, r.current_headroom_frac * 100.0) }</td></tr>
                    <tr><td>{ "Power density" }</td>
                        <td>{ format!("{:.2} W/cm² over {:.1} cm²", r.power_density_w_cm2, r.outline_area_cm2) }</td></tr>
                    <tr><td>{ "Trace" }</td>
                        <td>{ format!("{:.3} mm wide, {:.3} mm gap, {:.0} mm long", r.trace_width_mm, r.trace_gap_mm, r.trace_length_mm) }</td></tr>
                    <tr><td>{ "Copper" }</td>
                        <td>{ format!("{:.1} µm ({:.1} oz)", r.copper_thickness_um, r.copper_thickness_um / 34.8) }</td></tr>
                </tbody>
            </table>
        </div>
        <div class="panel">
            <h2>{ "Downloads" }</h2>
            <ul class="downloads">
                { for downloads.iter().map(|(filename, label, url)| html! {
                    <li>
                        <a href={url.clone()} download={filename.clone()}>{ filename }</a>
                        <span class="dl-label">{ format!(" — {label}") }</span>
                    </li>
                }) }
            </ul>
        </div>
        </>
    }
}

fn data_url(body: &str) -> String {
    format!(
        "data:application/octet-stream,{}",
        js_sys::encode_uri_component(body)
    )
}

fn main() {
    yew::Renderer::<App>::new().render();
}
