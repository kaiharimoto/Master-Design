//! WebAssembly surface.
//!
//! The studio calls these directly from the webview rather than over Tauri IPC. That
//! matters most for [`hit_test_fill`] and [`nearest_point_on_path`], which run on every
//! pointer move — a serialization round-trip per move would be felt as lag on a phone.
//!
//! Everything here is a thin wrapper: errors become JS exceptions, structs cross as
//! plain objects. Keep the logic in the pure modules so the native builds
//! (`md-cli`, `md-mcp`, `md-emit`) exercise exactly the same code.

use crate::{
    boolean::{boolean_op, BoolOp},
    hit::FillRule,
    stroke::StrokeStyle,
    DEFAULT_TOLERANCE,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(v).map_err(err)
}

#[wasm_bindgen(js_name = normalizePath)]
pub fn normalize_path(d: &str) -> Result<String, JsValue> {
    crate::normalize(d).map_err(err)
}

#[wasm_bindgen(js_name = pathBounds)]
pub fn path_bounds(d: &str) -> Result<JsValue, JsValue> {
    to_js(&crate::bounds(d).map_err(err)?)
}

#[wasm_bindgen(js_name = transformPath)]
pub fn transform_path(d: &str, matrix: Vec<f64>) -> Result<String, JsValue> {
    if matrix.len() != 6 {
        return Err(JsValue::from_str("matrix must have exactly 6 components"));
    }
    let m = [
        matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5],
    ];
    crate::transform_path(d, m).map_err(err)
}

#[wasm_bindgen(js_name = hitTestFill)]
pub fn hit_test_fill(d: &str, x: f64, y: f64, even_odd: bool) -> Result<bool, JsValue> {
    let rule = if even_odd {
        FillRule::EvenOdd
    } else {
        FillRule::NonZero
    };
    crate::hit_test_fill(d, x, y, rule).map_err(err)
}

#[wasm_bindgen(js_name = hitTestStroke)]
pub fn hit_test_stroke(
    d: &str,
    x: f64,
    y: f64,
    width: f64,
    tolerance: f64,
) -> Result<bool, JsValue> {
    crate::hit_test_stroke(d, x, y, width, tolerance).map_err(err)
}

#[wasm_bindgen(js_name = nearestPointOnPath)]
pub fn nearest_point_on_path(d: &str, x: f64, y: f64) -> Result<JsValue, JsValue> {
    to_js(&crate::nearest_point_on_path(d, x, y).map_err(err)?)
}

#[wasm_bindgen(js_name = booleanOp)]
pub fn boolean_op_js(paths: Vec<String>, op: &str, refit: bool) -> Result<String, JsValue> {
    let op = match op {
        "union" => BoolOp::Union,
        "subtract" => BoolOp::Subtract,
        "intersect" => BoolOp::Intersect,
        "exclude" => BoolOp::Exclude,
        other => return Err(JsValue::from_str(&format!("unknown boolean op: {other}"))),
    };
    boolean_op(&paths, op, DEFAULT_TOLERANCE, refit).map_err(err)
}

#[wasm_bindgen(js_name = outlineStroke)]
pub fn outline_stroke_js(d: &str, style: JsValue) -> Result<String, JsValue> {
    let style: StrokeStyle = serde_wasm_bindgen::from_value(style).map_err(err)?;
    crate::outline_stroke(d, &style).map_err(err)
}

/// `radii` is a CSS `border-radius` shorthand of one to four values; see
/// [`crate::expand_radii`] for how a short list spreads across the corners.
#[wasm_bindgen(js_name = rectPath)]
pub fn rect_path(x: f64, y: f64, w: f64, h: f64, radii: Vec<f64>) -> String {
    crate::rect_path(x, y, w, h, crate::expand_radii(&radii))
}

#[wasm_bindgen(js_name = ellipsePath)]
pub fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64) -> String {
    crate::ellipse_path(cx, cy, rx, ry)
}

#[wasm_bindgen(js_name = polygonPath)]
pub fn polygon_path(cx: f64, cy: f64, radius: f64, sides: u32, rotation: f64) -> String {
    crate::polygon_path(cx, cy, radius, sides, rotation)
}

#[wasm_bindgen(js_name = starPath)]
pub fn star_path(cx: f64, cy: f64, outer: f64, inner: f64, points: u32, rotation: f64) -> String {
    crate::star_path(cx, cy, outer, inner, points, rotation)
}

#[wasm_bindgen(js_name = roundCorners)]
pub fn round_corners(d: &str, radius: f64) -> Result<String, JsValue> {
    crate::round_corners(d, radius).map_err(err)
}

#[wasm_bindgen(js_name = pathLength)]
pub fn path_length(d: &str) -> Result<f64, JsValue> {
    crate::path_length(d).map_err(err)
}

/// Returns `{ point: [x, y], tangent: [dx, dy] }`.
#[wasm_bindgen(js_name = pointAtLength)]
pub fn point_at_length(d: &str, distance: f64) -> Result<JsValue, JsValue> {
    let (point, tangent) = crate::point_at_length(d, distance).map_err(err)?;
    to_js(&serde_json::json!({ "point": point, "tangent": tangent }))
}

/// Simplify then fit — the freehand/pencil pipeline, exposed as one call so the studio
/// does not have to pick tolerances twice.
#[wasm_bindgen(js_name = fitFreehand)]
pub fn fit_freehand(points: Vec<f64>, tolerance: f64) -> Result<String, JsValue> {
    if points.len() % 2 != 0 {
        return Err(JsValue::from_str(
            "points must be a flat [x, y, x, y, ...] array",
        ));
    }
    let pts: Vec<[f64; 2]> = points.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
    let tolerance = if tolerance > 0.0 {
        tolerance
    } else {
        DEFAULT_TOLERANCE
    };
    let simplified = crate::simplify_rdp(&pts, tolerance);
    Ok(crate::to_svg(&crate::fit_cubics(
        &simplified,
        tolerance * 2.0,
    )))
}
