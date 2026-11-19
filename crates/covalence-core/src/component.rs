//! Components (composition spec §7): whatever is on a set of controls, at
//! any size. A component lists the controls it covers and embeds the
//! bindings of each; one with `show` is shown on surfaces, a candidate in
//! resolution (surfaces spec §5.2).
//!
//! The running core still resolves and draws **layers**: a shown component
//! becomes a [`Layer`] (its controls and `show`), and its bindings are set
//! one by one, each with the component as its layer and an ID of
//! `<component>/<binding>` ([`binding_id`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::binding::{self, ControlBinding};
use crate::flow::{Code, Problem, Report};
use crate::select::{Entities, Selector};
use crate::source::Source;
use crate::surface::{
    self, Cascade, ControlSpec, Layer, LayerContext, LayerId, Level, OutputCapKind, When,
};
use covalence_protocol::state::PathSegment;

/// The entity type in the store.
pub const ENTITY: &str = "component";

/// A component (composition spec §7).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    /// The controls it covers: a shown component's are the surface's.
    pub controls: Vec<ControlSpec>,
    /// One per bound control, embedded (composition spec §7.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ControlBinding>,
    /// Shown on surfaces, and when (composition spec §7.3). Absent, it's
    /// only used inside other components.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<Show>,
}

/// Where and when a component is shown (surfaces spec §5.2, §5.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Show {
    /// The surfaces it may appear on; every surface when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Selector<Entities>>,
    /// When it's active; `{nav: "main"}` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
    #[serde(default, skip_serializing_if = "Level::is_normal")]
    pub level: Level,
    /// Its order within its level, 0 to 99; 0 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<Source<f64>>,
    /// Whether it sets the whole look of its controls, or only what it
    /// binds (surfaces spec §5.2.1).
    #[serde(default, skip_serializing_if = "Cascade::is_replace")]
    pub cascade: Cascade,
}

/// A binding's ID in the running core: `<component>/<binding>`, so two
/// components' bindings, and their helpers' cells, never meet.
pub fn binding_id(component: &str, binding: &str) -> String {
    format!("{component}/{binding}")
}

/// The component and binding of a binding ID in the running core.
pub fn split_binding_id(id: &str) -> Option<(&str, &str)> {
    id.split_once('/')
}

impl Component {
    /// Reads a component from JSON. A binding's problems name it under
    /// `bindings`.
    pub fn parse(json: &serde_json::Value) -> Result<Component, Vec<Problem>> {
        if let Some(bindings) = json.get("bindings").and_then(serde_json::Value::as_array) {
            for (i, b) in bindings.iter().enumerate() {
                ControlBinding::parse(b).map_err(|problems| {
                    problems
                        .into_iter()
                        .map(|p| prefixed(p, &[key("bindings"), PathSegment::Index(i as i64)]))
                        .collect::<Vec<_>>()
                })?;
            }
        }
        let mut component: Component = binding::deserialize(json, &[]).map_err(|p| vec![p])?;
        for b in &mut component.bindings {
            b.layer.clear();
        }
        Ok(component)
    }

    /// What resolution reads: its controls and `show`, with `show`'s
    /// defaults when it has none.
    pub fn layer(&self) -> Layer {
        let show = self.show.clone().unwrap_or_default();
        Layer {
            id: self.id.clone(),
            name: self.name.clone(),
            slug: self.slug.clone(),
            controls: self.controls.clone(),
            on: show.on,
            when: show.when,
            level: show.level,
            rank: show.rank,
            cascade: show.cascade,
        }
    }

    /// Its layer, if it's shown.
    pub fn shown(&self) -> Option<Layer> {
        self.show.as_ref().map(|_| self.layer())
    }

    /// Its bindings as the running core has them: on this component, with
    /// IDs of `<component>/<binding>`.
    pub fn runtime_bindings(&self) -> Vec<ControlBinding> {
        self.bindings
            .iter()
            .map(|b| ControlBinding {
                id: binding_id(&self.id, &b.id),
                layer: self.id.clone(),
                ..b.clone()
            })
            .collect()
    }

    pub fn control(&self, id: &str) -> Option<&ControlSpec> {
        self.controls.iter().find(|c| c.id == id)
    }
}

/// What a component is checked against.
pub struct Context<'a> {
    /// Slugs other components have.
    pub taken_slugs: &'a BTreeSet<String>,
    /// Every surface's ID, for `show.on`.
    pub surfaces: &'a BTreeSet<String>,
    /// Every connection's ID, for its bindings.
    pub conns: &'a BTreeSet<crate::state::ConnId>,
    /// Compiles expressions.
    pub evaluator: &'a dyn crate::expr::Evaluator,
    /// The other shown components, for `ambiguousOrder`.
    pub others: &'a BTreeMap<LayerId, Layer>,
}

/// Checks a component: what surfaces spec §9 checks of a layer, under
/// `show` for its fields, and each binding as bindings spec §2.1 checks one,
/// under `bindings`, with unique IDs and at most one per control.
pub fn validate(component: &Component, ctx: &Context<'_>) -> Report {
    // Its bindings are checked against its controls below, where a problem
    // can name the binding; the layer check needn't repeat it.
    let bound: BTreeMap<String, BTreeSet<OutputCapKind>> = BTreeMap::new();
    let none = BTreeMap::new();
    let layer = component.layer();
    let report = surface::validate_layer(
        &layer,
        &LayerContext {
            taken_slugs: ctx.taken_slugs,
            bound: &bound,
            surfaces: ctx.surfaces,
            evaluator: ctx.evaluator,
            others: if component.show.is_some() {
                ctx.others
            } else {
                &none
            },
        },
    );
    let under_show = |p: Problem| match p.path.first() {
        Some(PathSegment::Key(k))
            if ["on", "when", "level", "rank", "cascade"].contains(&k.as_str()) =>
        {
            prefixed(p, &[key("show")])
        }
        _ => p,
    };
    let mut errors: Vec<Problem> = report.errors.into_iter().map(under_show).collect();
    let mut warnings: Vec<Problem> = report.warnings.into_iter().map(under_show).collect();

    let layers = BTreeMap::from([(component.id.clone(), layer)]);
    let mut ids = BTreeSet::new();
    for (i, b) in component.bindings.iter().enumerate() {
        let at = [key("bindings"), PathSegment::Index(i as i64)];
        if b.id.is_empty() {
            errors.push(prefixed(
                Problem::new(vec![key("id")], Code::Invalid, "a binding needs an id"),
                &at,
            ));
        } else if !ids.insert(b.id.as_str()) {
            errors.push(prefixed(
                Problem::new(
                    vec![key("id")],
                    Code::Invalid,
                    format!("{:?} is the ID of another binding", b.id),
                ),
                &at,
            ));
        }
        let taken: BTreeSet<(LayerId, String)> = component
            .bindings
            .iter()
            .take(i)
            .map(|o| (component.id.clone(), o.control.clone()))
            .collect();
        let placed = ControlBinding {
            layer: component.id.clone(),
            ..b.clone()
        };
        let report = binding::validate(
            &placed,
            &binding::Context {
                layers: &layers,
                conns: ctx.conns,
                taken: &taken,
                evaluator: ctx.evaluator,
            },
        );
        errors.extend(report.errors.into_iter().map(|p| prefixed(p, &at)));
        warnings.extend(report.warnings.into_iter().map(|p| prefixed(p, &at)));
    }
    Report { errors, warnings }
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

fn prefixed(mut p: Problem, base: &[PathSegment]) -> Problem {
    let mut path = base.to_vec();
    path.append(&mut p.path);
    p.path = path;
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::QuickJs;
    use serde_json::json;

    fn component(json: serde_json::Value) -> Component {
        Component::parse(&json).unwrap()
    }

    fn codes(report: &Report) -> Vec<(serde_json::Value, String)> {
        report
            .errors
            .iter()
            .map(|p| {
                (
                    serde_json::to_value(&p.path).unwrap(),
                    serde_json::to_value(p.code)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned(),
                )
            })
            .collect()
    }

    fn check(c: &Component) -> Report {
        let evaluator = QuickJs::default();
        validate(
            c,
            &Context {
                taken_slugs: &BTreeSet::new(),
                surfaces: &BTreeSet::new(),
                conns: &BTreeSet::new(),
                evaluator: &evaluator,
                others: &BTreeMap::new(),
            },
        )
    }

    #[test]
    fn a_component_holds_its_bindings_and_shows_as_a_layer() {
        let c = component(json!({
            "id": "C", "name": "Cameras", "slug": "cams",
            "controls": [{ "id": "k0", "outputs": ["image"] }],
            "bindings": [{ "id": "b", "control": "k0", "props": { "label": "Hi" } }],
            "show": { "level": "over", "cascade": "through" }
        }));
        let layer = c.shown().unwrap();
        assert_eq!(
            (layer.level, layer.cascade),
            (Level::Over, Cascade::Through)
        );
        let runtime = c.runtime_bindings();
        assert_eq!(
            (runtime[0].id.as_str(), runtime[0].layer.as_str()),
            ("C/b", "C")
        );
        assert_eq!(split_binding_id("C/b"), Some(("C", "b")));
        // Stored without the component it's in.
        assert!(
            serde_json::to_value(&c).unwrap()["bindings"][0]
                .get("layer")
                .is_none()
        );
        assert!(check(&c).is_ok(), "{:?}", check(&c).errors);

        let used = Component { show: None, ..c };
        assert!(used.shown().is_none());
    }

    #[test]
    fn problems_name_show_and_bindings() {
        let c = component(json!({
            "id": "C", "name": "Cameras", "slug": "cams",
            "controls": [{ "id": "k0" }],
            "bindings": [
                { "id": "b", "control": "k0", "props": { "label": "Hi" } },
                { "id": "b", "control": "k9", "props": {} }
            ],
            "show": { "when": { "nav": "pop over" } }
        }));
        assert_eq!(
            codes(&check(&c)),
            [
                (json!(["show", "when", "nav"]), "invalid".into()),
                (json!(["bindings", 0, "props", "label"]), "noOutput".into()),
                (json!(["bindings", 1, "id"]), "invalid".into()),
                (json!(["bindings", 1, "control"]), "unknownControl".into()),
            ]
        );
        let bad = Component::parse(&json!({ "name": "x", "controls": [],
            "bindings": [{ "id": "b", "control": "k0", "props": { "glow": 1 } }] }));
        assert_eq!(
            serde_json::to_value(&bad.unwrap_err()[0].path).unwrap(),
            json!(["bindings", 0, "props", "glow"])
        );
    }
}
