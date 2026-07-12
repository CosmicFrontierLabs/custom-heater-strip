use gloo_net::http::Request;
use shared::{DesignError, DesignRequest, DesignResponse};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

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
    html! {
        <BrowserRouter>
            <Switch<Route> render={switch} />
        </BrowserRouter>
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

    let on_generate = {
        let svg_text = svg_text.clone();
        let (voltage, watts, max_current) = (voltage.clone(), watts.clone(), max_current.clone());
        let (copper_oz, min_trace, min_gap, edge_margin) = (
            copper_oz.clone(),
            min_trace.clone(),
            min_gap.clone(),
            edge_margin.clone(),
        );
        let result = result.clone();
        let error = error.clone();
        let busy = busy.clone();
        Callback::from(move |_: MouseEvent| {
            let Some((_, svg)) = (*svg_text).clone() else {
                error.set(Some("Upload an SVG outline first.".into()));
                return;
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
            };
            let result = result.clone();
            let error = error.clone();
            let busy = busy.clone();
            busy.set(true);
            spawn_local(async move {
                let resp = Request::post("/api/design")
                    .json(&req)
                    .expect("serialize request")
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.ok() => match r.json::<DesignResponse>().await {
                        Ok(d) => {
                            error.set(None);
                            result.set(Some(d));
                        }
                        Err(e) => error.set(Some(format!("Bad response: {e}"))),
                    },
                    Ok(r) => {
                        let msg = r
                            .json::<DesignError>()
                            .await
                            .map(|e| e.message)
                            .unwrap_or_else(|_| format!("HTTP {}", r.status()));
                        error.set(Some(msg));
                    }
                    Err(e) => error.set(Some(format!("Request failed: {e}"))),
                }
                busy.set(false);
            });
        })
    };

    let on_preset = {
        let (copper_oz, min_trace, min_gap) =
            (copper_oz.clone(), min_trace.clone(), min_gap.clone());
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            if let Some(p) = shared::FAB_PRESETS
                .iter()
                .find(|p| p.name == select.value())
            {
                copper_oz.set(p.copper_oz);
                min_trace.set(p.min_trace_mm);
                min_gap.set(p.min_gap_mm);
            }
        })
    };

    html! {
        <div class="designer">
            <h1>{ "Custom Heater Strip" }</h1>
            <p class="tagline">{ "Upload a flex outline, set your electrical budget, get a fab-ready serpentine heater." }</p>

            <div class="panel">
                <h2>{ "1 · Outline" }</h2>
                <input type="file" accept=".svg,image/svg+xml" onchange={on_file} />
                { match (*svg_text).as_ref() {
                    Some((name, text)) => html! {
                        <span class="file-ok">{ format!("{name} ({} bytes)", text.len()) }</span>
                    },
                    None => html! { <span class="file-hint">{ "SVG with a closed path, sized in mm" }</span> },
                }}
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
                    <NumField label="Min trace" unit="mm" value={*min_trace} step={0.05}
                        onchange={let v = min_trace.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Min gap" unit="mm" value={*min_gap} step={0.05}
                        onchange={let v = min_gap.clone(); Callback::from(move |x| v.set(x))} />
                    <NumField label="Edge margin" unit="mm" value={*edge_margin} step={0.1}
                        onchange={let v = edge_margin.clone(); Callback::from(move |x| v.set(x))} />
                </div>
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

    let mut downloads: Vec<(String, String)> =
        vec![("heater.kicad_pcb".to_string(), data_url(&resp.kicad_pcb))];
    downloads.extend(
        resp.gerbers
            .iter()
            .map(|(name, body)| (name.clone(), data_url(body))),
    );

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
                { for downloads.iter().map(|(name, url)| html! {
                    <li><a href={url.clone()} download={name.clone()}>{ name }</a></li>
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
