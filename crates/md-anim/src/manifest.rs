//! The animation package format.
//!
//! ## Why manifests are data, not code
//!
//! An animation package is a folder with a `manifest.json` in it. The manifest declares
//! its parameters and how those parameters turn into keyframes. There is no JavaScript.
//!
//! That constraint pays for itself three times:
//!
//! * **Headless baking.** `md-cli`, the exporter and the MCP server can all apply an
//!   animation without a JavaScript runtime. An AI working through MCP with no studio
//!   open can still animate a page.
//! * **Zero editor code per animation.** The parameter list *is* the inspector: a
//!   number with a range becomes a slider, a colour becomes a swatch, an easing becomes a
//!   curve picker. Adding an animation means adding a folder — no UI work, no rebuild.
//! * **Reviewable.** A manifest can be read, diffed and reasoned about. Arbitrary code
//!   dropped into a project directory and executed at document-open time could not be.
//!
//! Genuinely procedural motion — physics, path-following, anything that needs to look at
//! geometry it cannot express arithmetically — is what [`Generator::Script`] is reserved
//! for. Nothing in the standard library needs it yet.

use crate::error::{AnimError, Result};
use md_doc::anim::{Easing, Trigger};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationManifest {
    /// Namespaced identifier, e.g. `std/stagger-fade-up`.
    pub id: String,
    /// Semantic version. Documents pin the exact version they were baked against, so
    /// installing a newer package never silently changes existing work.
    pub version: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub category: Category,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub applies_to: AppliesTo,
    /// Trigger used when the caller does not specify one.
    pub default_trigger: Trigger,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamSpec>,
    pub generator: Generator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// Bringing something onto the page.
    #[default]
    Entrance,
    Exit,
    /// Drawing attention to something already there.
    Emphasis,
    /// Continuous background motion.
    Ambient,
    /// Driven by scroll position.
    Scroll,
    /// Responding to pointer or focus.
    Interaction,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Entrance => "entrance",
            Category::Exit => "exit",
            Category::Emphasis => "emphasis",
            Category::Ambient => "ambient",
            Category::Scroll => "scroll",
            Category::Interaction => "interaction",
        }
    }
}

/// Constraints on what a package can be applied to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliesTo {
    #[serde(default = "one_usize")]
    pub min_targets: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<usize>,
    /// Node kinds this makes sense on. `["*"]` means anything.
    ///
    /// Real constraints matter here: `draw-path` animates a stroke dash offset, which
    /// does nothing at all on a text node. Catching that at apply time is much kinder
    /// than letting someone wonder why nothing moved.
    #[serde(default = "any_kind")]
    pub kinds: Vec<String>,
}

impl Default for AppliesTo {
    fn default() -> Self {
        AppliesTo { min_targets: 1, max_targets: None, kinds: any_kind() }
    }
}

impl AppliesTo {
    pub fn accepts_kind(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == "*" || k == kind)
    }
}

fn one_usize() -> usize {
    1
}
fn any_kind() -> Vec<String> {
    vec!["*".to_string()]
}

/// One tunable knob, and everything the studio needs to draw a control for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub default: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ParamKind {
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        /// Shown next to the control: `px`, `s`, `°`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        unit: String,
    },
    Color,
    Boolean,
    Select {
        options: Vec<SelectOption>,
    },
    Easing,
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

impl ParamSpec {
    /// Check a supplied value against this parameter's type and range.
    pub fn validate(&self, value: &Value) -> Result<()> {
        let wrong = |expected: &str| {
            Err(AnimError::BadParam {
                key: self.key.clone(),
                reason: format!("expected {expected}, got {value}"),
            })
        };

        match &self.kind {
            ParamKind::Number { min, max, .. } => {
                let n = match value.as_f64() {
                    Some(n) => n,
                    None => return wrong("a number"),
                };
                if let Some(lo) = min {
                    if n < *lo {
                        return Err(AnimError::BadParam {
                            key: self.key.clone(),
                            reason: format!("{n} is below the minimum of {lo}"),
                        });
                    }
                }
                if let Some(hi) = max {
                    if n > *hi {
                        return Err(AnimError::BadParam {
                            key: self.key.clone(),
                            reason: format!("{n} is above the maximum of {hi}"),
                        });
                    }
                }
                Ok(())
            }
            ParamKind::Color => match value.as_str() {
                Some(s) => md_doc::Color::parse(s)
                    .map(|_| ())
                    .map_err(|e| AnimError::BadParam { key: self.key.clone(), reason: e.to_string() }),
                None => wrong("a colour string"),
            },
            ParamKind::Boolean => {
                if value.is_boolean() {
                    Ok(())
                } else {
                    wrong("true or false")
                }
            }
            ParamKind::Select { options } => match value.as_str() {
                Some(s) if options.iter().any(|o| o.value == s) => Ok(()),
                Some(s) => Err(AnimError::BadParam {
                    key: self.key.clone(),
                    reason: format!(
                        "'{s}' is not one of: {}",
                        options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                }),
                None => wrong("one of the listed options"),
            },
            ParamKind::Easing => {
                if serde_json::from_value::<Easing>(value.clone()).is_ok() {
                    Ok(())
                } else {
                    wrong("an easing name or cubicBezier")
                }
            }
            ParamKind::Text => {
                if value.is_string() {
                    Ok(())
                } else {
                    wrong("a string")
                }
            }
        }
    }
}

/// How a package turns parameters into keyframes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Generator {
    Declarative(DeclarativeGenerator),
    /// Reserved for procedural motion that arithmetic cannot express.
    ///
    /// Packages using this bake only inside the studio, where a JavaScript runtime
    /// exists. Nothing in the standard library needs it, and anything that can be
    /// declarative should be.
    Script { entry: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarativeGenerator {
    /// Total timeline length in seconds. An [`Expr`] over the parameters and `count`.
    pub duration: Expr,
    pub per_target: PerTarget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerTarget {
    /// When this target's motion begins, in seconds. Usually where stagger lives.
    #[serde(default)]
    pub start_at: Expr,
    /// When it finishes, in seconds.
    pub end_at: Expr,
    pub tracks: Vec<TrackTemplate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackTemplate {
    /// Property name — see `md_doc::anim::properties`.
    pub property: String,
    pub from: Value,
    pub to: Value,
    /// Overrides the timeline's easing for this track. `"$key"` reads a parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easing: Option<Value>,
}

/// A numeric value that may be written as a literal or as an expression.
///
/// Numbers stay numbers. Strings are evaluated as arithmetic, so
/// `"stagger * index"` works where a bare number would not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expr {
    Literal(f64),
    Formula(String),
}

impl Default for Expr {
    fn default() -> Self {
        Expr::Literal(0.0)
    }
}

impl Expr {
    pub fn eval(&self, scope: &crate::expr::Scope) -> Result<f64> {
        match self {
            Expr::Literal(v) => Ok(*v),
            Expr::Formula(src) => crate::expr::eval(src, scope),
        }
    }
}

impl AnimationManifest {
    pub fn param(&self, key: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.key == key)
    }

    /// Namespace half of the id — `std` for `std/stagger-fade-up`.
    pub fn namespace(&self) -> &str {
        self.id.split('/').next().unwrap_or("")
    }

    /// Structural checks that catch a broken package at load time rather than at the
    /// moment a designer tries to use it.
    pub fn validate(&self) -> Result<()> {
        if !self.id.contains('/') {
            return Err(AnimError::BadManifest {
                id: self.id.clone(),
                reason: "id must be namespaced, e.g. 'std/fade-in'".into(),
            });
        }
        if self.version.is_empty() {
            return Err(AnimError::BadManifest {
                id: self.id.clone(),
                reason: "version is required".into(),
            });
        }

        let mut keys: Vec<&str> = self.params.iter().map(|p| p.key.as_str()).collect();
        keys.sort();
        let count = keys.len();
        keys.dedup();
        if keys.len() != count {
            return Err(AnimError::BadManifest {
                id: self.id.clone(),
                reason: "duplicate parameter keys".into(),
            });
        }

        // Reserved names would be shadowed by the baker's own variables, and the
        // resulting animation would quietly ignore the designer's value.
        for reserved in ["index", "count"] {
            if self.params.iter().any(|p| p.key == reserved) {
                return Err(AnimError::BadManifest {
                    id: self.id.clone(),
                    reason: format!("'{reserved}' is a reserved variable name"),
                });
            }
        }

        for p in &self.params {
            p.validate(&p.default).map_err(|e| AnimError::BadManifest {
                id: self.id.clone(),
                reason: format!("default for '{}' is invalid: {e}", p.key),
            })?;
        }

        if let Generator::Declarative(g) = &self.generator {
            if g.per_target.tracks.is_empty() {
                return Err(AnimError::BadManifest {
                    id: self.id.clone(),
                    reason: "generator has no tracks".into(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn number_param() -> ParamSpec {
        ParamSpec {
            key: "distance".into(),
            label: "Distance".into(),
            kind: ParamKind::Number {
                min: Some(0.0),
                max: Some(400.0),
                step: Some(1.0),
                unit: "px".into(),
            },
            default: json!(32),
            description: String::new(),
        }
    }

    #[test]
    fn number_params_enforce_their_range() {
        let p = number_param();
        assert!(p.validate(&json!(50)).is_ok());
        assert!(p.validate(&json!(-1)).is_err());
        assert!(p.validate(&json!(500)).is_err());
        assert!(p.validate(&json!("nope")).is_err());
    }

    #[test]
    fn colour_params_reject_non_colours() {
        let p = ParamSpec {
            key: "tint".into(),
            label: "Tint".into(),
            kind: ParamKind::Color,
            default: json!("#ff0055"),
            description: String::new(),
        };
        assert!(p.validate(&json!("#00ff00")).is_ok());
        assert!(p.validate(&json!("chartreuse")).is_err());
    }

    #[test]
    fn select_params_list_valid_options_on_failure() {
        let p = ParamSpec {
            key: "axis".into(),
            label: "Axis".into(),
            kind: ParamKind::Select {
                options: vec![
                    SelectOption { value: "x".into(), label: "Horizontal".into() },
                    SelectOption { value: "y".into(), label: "Vertical".into() },
                ],
            },
            default: json!("y"),
            description: String::new(),
        };
        assert!(p.validate(&json!("x")).is_ok());
        let err = p.validate(&json!("z")).unwrap_err().to_string();
        assert!(err.contains("x, y"), "got {err}");
    }

    #[test]
    fn easing_params_accept_names_and_custom_curves() {
        let p = ParamSpec {
            key: "easing".into(),
            label: "Easing".into(),
            kind: ParamKind::Easing,
            default: json!("easeOut"),
            description: String::new(),
        };
        assert!(p.validate(&json!("easeInOut")).is_ok());
        assert!(p.validate(&json!({ "cubicBezier": [0.2, 0.0, 0.1, 1.0] })).is_ok());
        assert!(p.validate(&json!("bouncy")).is_err());
    }

    fn manifest() -> AnimationManifest {
        AnimationManifest {
            id: "std/test".into(),
            version: "1.0.0".into(),
            title: "Test".into(),
            description: String::new(),
            category: Category::Entrance,
            tags: vec![],
            applies_to: AppliesTo::default(),
            default_trigger: Trigger::Load { delay: 0.0 },
            params: vec![number_param()],
            generator: Generator::Declarative(DeclarativeGenerator {
                duration: Expr::Literal(1.0),
                per_target: PerTarget {
                    start_at: Expr::Literal(0.0),
                    end_at: Expr::Literal(1.0),
                    tracks: vec![TrackTemplate {
                        property: "opacity".into(),
                        from: json!(0),
                        to: json!(1),
                        easing: None,
                    }],
                },
            }),
        }
    }

    #[test]
    fn a_well_formed_manifest_validates() {
        manifest().validate().unwrap();
    }

    #[test]
    fn an_unnamespaced_id_is_rejected() {
        let mut m = manifest();
        m.id = "fade".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn reserved_variable_names_cannot_be_parameters() {
        let mut m = manifest();
        m.params[0].key = "index".into();
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("reserved"), "got {err}");
    }

    #[test]
    fn a_default_outside_its_own_range_is_caught_at_load() {
        let mut m = manifest();
        m.params[0].default = json!(9999);
        assert!(m.validate().is_err());
    }

    #[test]
    fn expressions_accept_both_numbers_and_formulas() {
        let m: DeclarativeGenerator = serde_json::from_value(json!({
            "duration": "stagger * count",
            "perTarget": { "endAt": 1.5, "tracks": [{ "property": "opacity", "from": 0, "to": 1 }] }
        }))
        .unwrap();
        assert_eq!(m.duration, Expr::Formula("stagger * count".into()));
        assert_eq!(m.per_target.end_at, Expr::Literal(1.5));
        assert_eq!(m.per_target.start_at, Expr::Literal(0.0));
    }

    #[test]
    fn a_manifest_round_trips() {
        let m = manifest();
        let json = serde_json::to_string(&m).unwrap();
        let back: AnimationManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn applies_to_filters_by_node_kind() {
        let a = AppliesTo { min_targets: 1, max_targets: None, kinds: vec!["path".into()] };
        assert!(a.accepts_kind("path"));
        assert!(!a.accepts_kind("text"));
        assert!(AppliesTo::default().accepts_kind("text"));
    }
}
