//! Components (composition spec §7): whatever is on a set of controls, at
//! any size. A component lists the controls it covers and embeds the
//! bindings of each; one with `show` is shown on surfaces, a candidate in
//! resolution (surfaces spec §5.2).
//!
//! [`crate::surface::Surfaces`] resolves and draws shown components. Their
//! bindings run one by one, each knowing its component and with an ID of
//! `<component>/<binding>` ([`binding_id`]), so two components' bindings and
//! helper cells never meet.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use serde_json::Value as Json;

use crate::binding::{self, ControlBinding};
use crate::compose;
use crate::flow::{Code, Problem, Report};
use crate::param::ParamDef;
use crate::select::{Entities, Selector};
use crate::source::Source;
use crate::state::StatePath;
use crate::surface::{self, Cascade, ComponentId, ControlSpec, FieldContext, Level, When};
use covalence_protocol::state::PathSegment;

/// The entity type in the store.
pub const ENTITY: &str = "component";

/// A component (composition spec §7): a plain one, a template (it declares
/// `params`), or an instance of a template (composition spec §1), which has
/// only its own identity, its params' bindings and `show`.
///
/// The app keeps an instance with its template's body filled in
/// ([`Component::instance_of`]), so it's resolved, drawn and checked like
/// any other component. It's stored and sent as an instance: its template
/// gives the rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct Component {
    pub id: String,
    pub name: String,
    pub slug: String,
    /// An instance's template.
    pub template: Option<ComponentId>,
    /// A template's params: `params.<name>` in its bindings, helpers and
    /// `show` (composition spec §1). An instance has its template's.
    pub params: BTreeMap<String, ParamDef>,
    /// An instance's bindings of its template's params, as `params` on the wire.
    pub param_bindings: BTreeMap<String, Source<Json>>,
    /// The controls it covers: a shown component's are the surface's.
    pub controls: Vec<ControlSpec>,
    /// One per bound control, embedded (composition spec §7.1).
    pub bindings: Vec<ControlBinding>,
    /// Shown on surfaces, and when (composition spec §7.3). Absent, it's
    /// only used inside other components.
    pub show: Option<Show>,
}

/// A component as it's stored and sent. An instance's `params` are
/// bindings, and a template's are definitions, so both go through here.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    #[serde(default)]
    id: String,
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    template: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    params: serde_json::Map<String, Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    controls: Option<Vec<ControlSpec>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bindings: Vec<ControlBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    show: Option<Show>,
}

impl TryFrom<Wire> for Component {
    type Error = String;

    fn try_from(w: Wire) -> Result<Self, String> {
        let instance = w.template.is_some();
        if instance {
            for (field, has) in [
                ("controls", w.controls.is_some()),
                ("bindings", !w.bindings.is_empty()),
            ] {
                if has {
                    return Err(format!(
                        "an instance takes its {field} from its template; it has only its identity, `params` and `show`"
                    ));
                }
            }
        } else if w.controls.is_none() {
            return Err("missing field `controls`".into());
        }
        let mut params = BTreeMap::new();
        let mut param_bindings = BTreeMap::new();
        for (name, json) in w.params {
            if instance {
                let source = Source::from_json(json).map_err(|e| format!("params.{name}: {e}"))?;
                param_bindings.insert(name, source);
            } else {
                let def: ParamDef =
                    serde_json::from_value(json).map_err(|e| format!("params.{name}: {e}"))?;
                params.insert(name, def);
            }
        }
        Ok(Component {
            id: w.id,
            name: w.name,
            slug: w.slug,
            template: w.template,
            params,
            param_bindings,
            controls: w.controls.unwrap_or_default(),
            bindings: w.bindings,
            show: w.show,
        })
    }
}

impl From<Component> for Wire {
    fn from(c: Component) -> Wire {
        let instance = c.template.is_some();
        let params = if instance {
            c.param_bindings
                .iter()
                .map(|(k, s)| (k.clone(), s.to_json().expect("a source encodes as JSON")))
                .collect()
        } else {
            c.params
                .iter()
                .map(|(k, d)| {
                    (
                        k.clone(),
                        serde_json::to_value(d).expect("a param encodes as JSON"),
                    )
                })
                .collect()
        };
        Wire {
            id: c.id,
            name: c.name,
            slug: c.slug,
            template: c.template,
            params,
            controls: (!instance).then_some(c.controls),
            bindings: if instance { Vec::new() } else { c.bindings },
            show: c.show,
        }
    }
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
        binding::deserialize(json, &[]).map_err(|p| vec![p])
    }

    /// Its `show`, or `show`'s defaults when it isn't shown.
    pub fn shows(&self) -> &Show {
        static NOT_SHOWN: std::sync::LazyLock<Show> = std::sync::LazyLock::new(Show::default);
        self.show.as_ref().unwrap_or(&NOT_SHOWN)
    }

    /// An instance of a template.
    pub fn is_instance(&self) -> bool {
        self.template.is_some()
    }

    /// A template: it declares params, and isn't an instance.
    pub fn is_template(&self) -> bool {
        self.template.is_none() && !self.params.is_empty()
    }

    /// An abstract template: a required param with no default, so it's only
    /// shown through instances (composition spec §1).
    pub fn is_abstract(&self) -> bool {
        self.template.is_none() && self.params.values().any(ParamDef::needs_value)
    }

    /// An instance with its template's body (composition spec §2): the
    /// template's params, controls and bindings, with the instance's
    /// identity, params' bindings and `show`.
    pub fn instance_of(&self, template: &Component) -> Component {
        Component {
            id: self.id.clone(),
            name: self.name.clone(),
            slug: self.slug.clone(),
            template: self.template.clone(),
            params: template.params.clone(),
            param_bindings: self.param_bindings.clone(),
            controls: template.controls.clone(),
            bindings: template.bindings.clone(),
            show: self.show.clone(),
        }
    }

    /// Its home in the state tree: `component.<slug>` (composition spec §1.1).
    pub fn home(&self) -> StatePath {
        StatePath::new(
            crate::source::Root::Component.as_str(),
            vec![PathSegment::Key(self.slug.clone())],
        )
    }

    /// Where its bindings, helpers and `show` find `params`: `<home>.params`,
    /// when it has params.
    pub fn params_at(&self) -> Option<StatePath> {
        (!self.params.is_empty()).then(|| self.home().key("params"))
    }

    /// The cells its params are while it's shown (composition spec §1): one
    /// per param that's bound or has a default, at `<home>.params.<name>`.
    /// None for a component that isn't shown, or an abstract template,
    /// which is only shown through its instances.
    pub fn param_cells(&self) -> Vec<crate::dataflow::CellDef> {
        if self.show.is_none() || self.is_abstract() {
            return Vec::new();
        }
        compose::param_cells(
            &compose::param_prefix(&self.id, None),
            &self.home(),
            &compose::bound(&self.params, &self.param_bindings),
            &[],
        )
    }

    /// Its bindings as the running core has them: on this component, with
    /// IDs of `<component>/<binding>`, and its `params` in scope.
    pub fn runtime_bindings(&self) -> Vec<ControlBinding> {
        let params = self.params_at();
        self.bindings
            .iter()
            .map(|b| ControlBinding {
                id: binding_id(&self.id, &b.id),
                component: self.id.clone(),
                params: params.clone(),
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
    pub others: &'a BTreeMap<ComponentId, Component>,
    /// Every component, instances with their templates' bodies: for an
    /// instance's template, and a template's instances.
    pub components: &'a BTreeMap<ComponentId, Component>,
}

/// Checks a component: its own fields ([`surface::validate_fields`]), each
/// binding as bindings spec §2.1 checks one, under `bindings`, with unique
/// IDs and at most one per control, and its params as a template's or an
/// instance's (composition spec §5). An instance is checked with its
/// template's body filled in ([`Component::instance_of`]); its bindings
/// were checked with the template.
pub fn validate(component: &Component, ctx: &Context<'_>) -> Report {
    let none = BTreeMap::new();
    let report = surface::validate_fields(
        component,
        &FieldContext {
            taken_slugs: ctx.taken_slugs,
            surfaces: ctx.surfaces,
            evaluator: ctx.evaluator,
            others: if component.show.is_some() {
                ctx.others
            } else {
                &none
            },
        },
    );
    let mut errors = report.errors;
    let mut warnings = report.warnings;
    if component.is_instance() {
        errors.extend(instance_problems(component, ctx));
        return Report { errors, warnings };
    }
    errors.extend(template_problems(component, ctx));
    let components = BTreeMap::from([(component.id.clone(), component.clone())]);
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
        let taken: BTreeSet<(ComponentId, String)> = component
            .bindings
            .iter()
            .take(i)
            .map(|o| (component.id.clone(), o.control.clone()))
            .collect();
        let placed = ControlBinding {
            component: component.id.clone(),
            ..b.clone()
        };
        let report = binding::validate(
            &placed,
            &binding::Context {
                components: &components,
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

/// An instance (composition spec §1, §7.3): its template is a component
/// that isn't an instance, its bindings fit the template's params, and it's
/// shown, since a child is written inside its container.
fn instance_problems(component: &Component, ctx: &Context<'_>) -> Vec<Problem> {
    let mut errors = Vec::new();
    let id = component.template.as_deref().unwrap_or_default();
    let at = vec![key("template")];
    match ctx.components.get(id) {
        _ if id == component.id => errors.push(Problem::new(
            at,
            Code::RecursiveTemplate,
            "a component can't be an instance of itself",
        )),
        None => errors.push(Problem::new(
            at,
            Code::UnknownTemplate,
            format!("no component {id:?}"),
        )),
        Some(template) if template.is_instance() => errors.push(Problem::new(
            at,
            Code::TemplateIsInstance,
            format!(
                "{:?} is itself an instance; use its template, {:?}",
                template.name,
                template.template.as_deref().unwrap_or_default()
            ),
        )),
        Some(template) => {
            errors.extend(compose::check_bindings(
                &template.params,
                &component.param_bindings,
                false,
            ));
            for (name, source) in &component.param_bindings {
                let mut here = vec![key("params"), key(name)];
                source.walk(&mut here, &mut |at, part| {
                    if let crate::source::Part::Expr(e) = part
                        && let Err(e) = ctx.evaluator.check(e)
                    {
                        errors.push(Problem::new(at.clone(), Code::ExprSyntax, e.message));
                    }
                });
            }
        }
    }
    if component.show.is_none() {
        errors.push(Problem::new(
            vec![key("show")],
            Code::Invalid,
            "an instance of a component is shown: one inside another component is a child, written in its container",
        ));
    }
    errors
}

/// A template's params (composition spec §1): names, defaults that fit,
/// a shown template that can be shown by itself, and a change that every
/// instance still fits (`badInstance`).
fn template_problems(component: &Component, ctx: &Context<'_>) -> Vec<Problem> {
    let mut errors = Vec::new();
    for (name, def) in &component.params {
        let at = vec![key("params"), key(name)];
        if !crate::flow::is_identifier(name) {
            errors.push(Problem::new(
                at.clone(),
                Code::BadId,
                format!("param {name:?} isn't a name"),
            ));
        }
        if compose::ParamKind::of(def) != compose::ParamKind::Value {
            errors.push(Problem::new(
                at.clone(),
                Code::Unsupported,
                "a component's params hold values for now; ones that hold a component come with children",
            ));
            continue;
        }
        if let Some(value) = def.default.as_ref().and_then(Source::literal) {
            for m in crate::schema::check(&def.schema(), value) {
                let mut path = at.clone();
                path.push(key("default"));
                path.extend(m.path);
                errors.push(Problem::new(path, Code::BadValue, m.message));
            }
        }
    }
    if component.show.is_some()
        && let Some((name, _)) = component.params.iter().find(|(_, d)| d.needs_value())
    {
        errors.push(Problem::new(
            vec![key("show")],
            Code::UnboundParam,
            format!(
                "{name:?} is required and has no default, so this template is only shown through its instances: remove `show`, or give it a default"
            ),
        ));
    }
    if !component.id.is_empty() {
        let instances = ctx
            .components
            .values()
            .filter(|c| c.template.as_deref() == Some(component.id.as_str()));
        let broken = compose::broken_instances(
            &component.params,
            instances.map(|c| (c.name.as_str(), &c.param_bindings, false)),
        );
        if !broken.is_empty() {
            let list: Vec<String> = broken
                .iter()
                .map(|(name, why)| format!("{name:?} ({})", why.join("; ")))
                .collect();
            errors.push(Problem::new(
                vec![key("params")],
                Code::BadInstance,
                format!(
                    "this change would break {} instance{}: {}",
                    broken.len(),
                    if broken.len() == 1 { "" } else { "s" },
                    list.join(", ")
                ),
            ));
        }
    }
    errors
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
                components: &BTreeMap::new(),
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
        assert_eq!(
            (c.shows().level, c.shows().cascade),
            (Level::Over, Cascade::Through)
        );
        let runtime = c.runtime_bindings();
        assert_eq!(
            (runtime[0].id.as_str(), runtime[0].component.as_str()),
            ("C/b", "C")
        );
        assert_eq!(split_binding_id("C/b"), Some(("C", "b")));
        // Stored without the component it's in.
        assert!(
            serde_json::to_value(&c).unwrap()["bindings"][0]
                .get("component")
                .is_none()
        );
        assert!(check(&c).is_ok(), "{:?}", check(&c).errors);

        let used = Component { show: None, ..c };
        assert_eq!(used.shows(), &Show::default());
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

    #[test]
    fn an_instance_is_its_identity_params_and_show_over_its_template() {
        let template = component(json!({
            "id": "T", "name": "Camera page", "slug": "cameraPage",
            "controls": [{ "id": "k0", "outputs": ["image"] }],
            "params": { "camera": { "type": "string", "default": "Cam A" } },
            "bindings": [{ "id": "b", "control": "k0",
                "props": { "label": { "ref": { "scope": "params", "path": ["camera"] } } } }],
        }));
        assert!(template.is_template() && !template.is_abstract());
        assert!(check(&template).is_ok(), "{:?}", check(&template).errors);
        // Not shown, so it has no param cells.
        assert!(template.param_cells().is_empty());

        let stored = json!({
            "id": "B", "name": "Cam B", "slug": "camB", "template": "T",
            "params": { "camera": "Cam B" }, "show": {},
        });
        let instance = component(stored.clone()).instance_of(&template);
        assert_eq!(instance.controls, template.controls);
        assert_eq!(serde_json::to_value(&instance).unwrap(), stored);
        let runtime = instance.runtime_bindings();
        assert_eq!(runtime[0].id, "B/b");
        assert_eq!(
            runtime[0].named(),
            [("params".to_owned(), instance.home().key("params"))]
        );
        let cells = instance.param_cells();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].home, instance.home().key("params").key("camera"));

        let bad = Component::parse(&json!({
            "name": "x", "template": "T", "controls": [], "show": {},
        }));
        assert!(bad.is_err());
    }

    #[test]
    fn show_names_its_own_params() {
        let c = component(json!({
            "id": "C", "name": "Cam", "slug": "cam", "controls": [],
            "params": { "camera": { "type": "string", "default": "Cam A" } },
            "show": { "when": { "while": { "ref": { "scope": "params", "path": ["cam"] } } } },
        }));
        assert_eq!(
            codes(&check(&c)),
            [(
                json!(["show", "when", "while", "ref"]),
                "unknownParam".into()
            )]
        );
        let plain = component(json!({
            "id": "P", "name": "Plain", "slug": "plain", "controls": [],
            "show": { "rank": { "ref": { "scope": "params", "path": ["n"] } } },
        }));
        assert_eq!(
            codes(&check(&plain)),
            [(json!(["show", "rank", "ref"]), "notInScope".into())]
        );
    }
}
