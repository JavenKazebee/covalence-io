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
    /// Components on some of its controls, by slug (composition spec §8).
    pub children: BTreeMap<String, Child>,
    /// Shown on surfaces, and when (composition spec §7.3). Absent, it's
    /// only used inside other components.
    pub show: Option<Show>,
    /// Its children at every depth, as the app keeps them
    /// ([`Component::filled`]). Not stored.
    pub parts: Vec<Part>,
}

/// A child (composition spec §8): an instance of a component on some of
/// its container's controls, its params bound in the container's scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Child {
    /// The component it's an instance of.
    pub template: ComponentId,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Source<Json>>,
    /// Each of its template's controls, to one of its container's.
    pub controls: BTreeMap<String, String>,
}

/// A child at some depth, as it runs on its shown component (composition
/// spec §8): where it is, what it's inside, and its template's bindings on
/// the shown component's controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// `<container's ID>/<slug>`: the child's instance in journal chains.
    pub id: String,
    pub template: ComponentId,
    /// `<container's home>.parts.<slug>` (composition spec §1.1).
    pub home: StatePath,
    /// The instances it's inside, outermost first, itself last.
    pub chain: compose::InstanceChain,
    /// Its params' sources, evaluated in its container's scope.
    pub bound: Vec<(String, Source<Json>)>,
    /// Its container's `<home>.params`, which its params' sources read as
    /// `params`, when the container has params.
    pub outer: Option<StatePath>,
    /// Its own `<home>.params`, when its template has params.
    pub params_at: Option<StatePath>,
    /// Each of its template's controls, to the shown component's.
    pub controls: BTreeMap<String, String>,
    /// Its template's bindings, on its template's controls.
    pub bindings: Vec<ControlBinding>,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    children: BTreeMap<String, Child>,
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
                ("children", !w.children.is_empty()),
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
            children: w.children,
            show: w.show,
            parts: Vec::new(),
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
            children: if instance {
                BTreeMap::new()
            } else {
                c.children
            },
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
            children: template.children.clone(),
            show: self.show.clone(),
            parts: Vec::new(),
        }
    }

    /// The component as the app keeps it: an instance with its template's
    /// body (composition spec §2), and its children at every depth as
    /// [`Part`]s (composition spec §8). `components` are the stored ones;
    /// a child whose template is missing or an instance is left out, and
    /// nesting stops at [`compose::MAX_DEPTH`].
    pub fn filled(&self, components: &BTreeMap<ComponentId, Component>) -> Component {
        let mut c = match self.template.as_ref().and_then(|t| components.get(t)) {
            Some(t) if !t.is_instance() => self.instance_of(t),
            _ => self.clone(),
        };
        let chain: compose::InstanceChain = c
            .template
            .iter()
            .map(|t| compose::Link {
                template: t.clone(),
                instance: c.id.clone(),
                key: None,
            })
            .collect();
        let controls: BTreeMap<String, String> = c
            .controls
            .iter()
            .map(|s| (s.id.clone(), s.id.clone()))
            .collect();
        let mut parts = Vec::new();
        expand(
            &Container {
                id: &c.id,
                home: &c.home(),
                params: c.params_at(),
                controls: &controls,
                chain: &chain,
            },
            &c.children,
            components,
            &mut parts,
        );
        c.parts = parts;
        c
    }

    /// The instances input on `control` is made inside (composition spec
    /// §5, §7.2): the deepest child on it, or the component itself when it's
    /// an instance.
    pub fn chain_at(&self, control: &str) -> Option<compose::InstanceChain> {
        let deepest = self
            .parts
            .iter()
            .filter(|p| p.controls.values().any(|c| c == control))
            .max_by_key(|p| p.chain.len());
        match deepest {
            Some(part) => Some(part.chain.clone()),
            None => self.template.as_ref().map(|t| {
                vec![compose::Link {
                    template: t.clone(),
                    instance: self.id.clone(),
                    key: None,
                }]
            }),
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
    /// Its children's params are cells too, at `<child's home>.params`,
    /// reading their container's as `params` (composition spec §8).
    pub fn param_cells(&self) -> Vec<crate::dataflow::CellDef> {
        if self.show.is_none() || self.is_abstract() {
            return Vec::new();
        }
        let mut cells = compose::param_cells(
            &compose::param_prefix(&self.id, None),
            &self.home(),
            &compose::bound(&self.params, &self.param_bindings),
            &[],
        );
        for part in &self.parts {
            let named: Vec<(String, StatePath)> = part
                .outer
                .iter()
                .map(|p| ("params".to_owned(), p.clone()))
                .collect();
            cells.extend(
                compose::param_cells(
                    &compose::param_prefix(&part.id, None),
                    &part.home,
                    &part.bound,
                    &[],
                )
                .into_iter()
                .map(|c| crate::dataflow::CellDef {
                    named: named.clone(),
                    ..c
                }),
            );
        }
        cells
    }

    /// Its bindings as the running core has them: on this component, with
    /// IDs of `<component>/<binding>`, and its `params` in scope.
    /// Its children's bindings are on its controls, each with an ID of
    /// `<child's ID>/<binding>` and its child's `params` (composition spec §8).
    pub fn runtime_bindings(&self) -> Vec<ControlBinding> {
        let params = self.params_at();
        let own = self.bindings.iter().map(|b| ControlBinding {
            id: binding_id(&self.id, &b.id),
            component: self.id.clone(),
            params: params.clone(),
            ..b.clone()
        });
        let parts = self.parts.iter().flat_map(|part| {
            part.bindings.iter().filter_map(move |b| {
                Some(ControlBinding {
                    id: binding_id(&part.id, &b.id),
                    component: self.id.clone(),
                    params: part.params_at.clone(),
                    part: Some((part.home.clone(), b.control.clone())),
                    control: part.controls.get(&b.control)?.clone(),
                    ..b.clone()
                })
            })
        });
        own.chain(parts).collect()
    }

    pub fn control(&self, id: &str) -> Option<&ControlSpec> {
        self.controls.iter().find(|c| c.id == id)
    }
}

/// Where a child is placed: its container, as the shown component sees it.
struct Container<'a> {
    id: &'a str,
    home: &'a StatePath,
    params: Option<StatePath>,
    /// The container's controls, to the shown component's.
    controls: &'a BTreeMap<String, String>,
    chain: &'a compose::InstanceChain,
}

/// Adds `children` of `container`, and theirs, to `out`, depth first.
fn expand(
    container: &Container<'_>,
    children: &BTreeMap<String, Child>,
    components: &BTreeMap<ComponentId, Component>,
    out: &mut Vec<Part>,
) {
    if container.chain.len() >= compose::MAX_DEPTH {
        return;
    }
    for (slug, child) in children {
        let Some(template) = components.get(&child.template).filter(|t| !t.is_instance()) else {
            continue;
        };
        let id = format!("{}/{slug}", container.id);
        let home = container.home.key("parts").key(slug);
        let mut chain = container.chain.clone();
        chain.push(compose::Link {
            template: template.id.clone(),
            instance: id.clone(),
            key: None,
        });
        let controls: BTreeMap<String, String> = child
            .controls
            .iter()
            .filter_map(|(its, to)| Some((its.clone(), container.controls.get(to)?.clone())))
            .collect();
        let params_at = (!template.params.is_empty()).then(|| home.key("params"));
        out.push(Part {
            id: id.clone(),
            template: template.id.clone(),
            home: home.clone(),
            chain: chain.clone(),
            bound: compose::bound(&template.params, &child.params),
            outer: container.params.clone(),
            params_at: params_at.clone(),
            controls: controls.clone(),
            bindings: template.bindings.clone(),
        });
        expand(
            &Container {
                id: &id,
                home: &home,
                params: params_at,
                controls: &controls,
                chain: &chain,
            },
            &template.children,
            components,
            out,
        );
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
    errors.extend(children_problems(component, ctx));
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

/// Its children (composition spec §8): each names a component that isn't
/// an instance and fits where it's placed ([`child_problems`]), each
/// control is covered once, by a binding or a child (`duplicateControl`),
/// and the component doesn't contain itself (`recursiveTemplate`) or nest
/// more than 16 deep (`tooDeep`).
fn children_problems(component: &Component, ctx: &Context<'_>) -> Vec<Problem> {
    let mut errors = Vec::new();
    for (slug, child) in &component.children {
        let at = [key("children"), key(slug)];
        if !crate::flow::is_identifier(slug) {
            errors.push(Problem::new(
                at.to_vec(),
                Code::BadId,
                format!("child {slug:?} isn't a name"),
            ));
        }
        let id = child.template.as_str();
        let here = [key("children"), key(slug), key("template")];
        match ctx.components.get(id) {
            _ if id == component.id => errors.push(Problem::new(
                here.to_vec(),
                Code::RecursiveTemplate,
                "a component can't contain itself",
            )),
            None => errors.push(Problem::new(
                here.to_vec(),
                Code::UnknownTemplate,
                format!("no component {id:?}"),
            )),
            Some(t) if t.is_instance() => errors.push(Problem::new(
                here.to_vec(),
                Code::TemplateIsInstance,
                format!(
                    "{:?} is itself an instance; use its template, {:?}",
                    t.name,
                    t.template.as_deref().unwrap_or_default()
                ),
            )),
            Some(t) => errors.extend(
                child_problems(child, t, component, ctx.evaluator)
                    .into_iter()
                    .map(|p| prefixed(p, &at)),
            ),
        }
    }
    // Each control once (composition spec §7.1).
    let mut covered: BTreeMap<&str, String> = component
        .bindings
        .iter()
        .map(|b| (b.control.as_str(), format!("binding {:?}", b.id)))
        .collect();
    for (slug, child) in &component.children {
        for (its, to) in &child.controls {
            if let Some(by) = covered.get(to.as_str()) {
                errors.push(Problem::new(
                    vec![key("children"), key(slug), key("controls"), key(its)],
                    Code::DuplicateControl,
                    format!("{to} is already covered, by {by}"),
                ));
            } else {
                covered.insert(to, format!("child {slug:?}"));
            }
        }
    }
    if !component.id.is_empty() && !component.children.is_empty() {
        let edges = |id: &str| -> Vec<String> {
            let c = if id == component.id {
                Some(component)
            } else {
                ctx.components.get(id)
            };
            c.map(|c| c.children.values().map(|ch| ch.template.clone()).collect())
                .unwrap_or_default()
        };
        if let Err((code, trail)) = compose::check_nesting(&component.id, &edges) {
            let names: Vec<String> = trail
                .iter()
                .map(|id| {
                    let name = if id == &component.id {
                        &component.name
                    } else {
                        ctx.components.get(id).map_or(id, |c| &c.name)
                    };
                    format!("{name:?}")
                })
                .collect();
            let message = if code == Code::TooDeep {
                format!(
                    "children nest at most {} deep: {}",
                    compose::MAX_DEPTH,
                    names.join(" → ")
                )
            } else {
                format!("it would contain itself: {}", names.join(" → "))
            };
            errors.push(Problem::new(vec![key("children")], code, message));
        }
    }
    errors
}

/// Whether a child fits where it's placed (composition spec §8), with
/// problems relative to the child: its params fit its template's, read in
/// its container's scope, and each of its template's controls is mapped
/// to one of the container's with the capabilities it needs.
pub fn child_problems(
    child: &Child,
    template: &Component,
    container: &Component,
    evaluator: &dyn crate::expr::Evaluator,
) -> Vec<Problem> {
    let mut errors = compose::check_bindings_in(
        &template.params,
        &child.params,
        false,
        Some(&container.params),
    );
    for (name, source) in &child.params {
        let mut here = vec![key("params"), key(name)];
        source.walk(&mut here, &mut |at, part| {
            if let crate::source::Part::Expr(e) = part
                && let Err(e) = evaluator.check(e)
            {
                errors.push(Problem::new(at.clone(), Code::ExprSyntax, e.message));
            }
        });
    }
    for spec in &template.controls {
        let Some(to) = child.controls.get(&spec.id) else {
            errors.push(Problem::new(
                vec![key("controls")],
                Code::UnmappedControl,
                format!("{:?}'s control {:?} isn't mapped", template.name, spec.id),
            ));
            continue;
        };
        let at = vec![key("controls"), key(&spec.id)];
        let Some(theirs) = container.control(to) else {
            errors.push(Problem::new(
                at,
                Code::UnknownControl,
                format!("the component doesn't list a control {to:?}"),
            ));
            continue;
        };
        let inputs = spec.inputs.iter().filter(|k| !theirs.inputs.contains(k));
        let outputs = spec.outputs.iter().filter(|k| !theirs.outputs.contains(k));
        let missing: Vec<String> = inputs
            .map(|k| format!("{k:?}").to_lowercase())
            .chain(outputs.map(|k| format!("{k:?}").to_lowercase()))
            .collect();
        if !missing.is_empty() {
            errors.push(Problem::new(
                at,
                Code::DoesNotFit,
                format!(
                    "{:?}'s {:?} needs {}, and {to} doesn't give {}",
                    template.name,
                    spec.id,
                    missing.join(", "),
                    if missing.len() == 1 { "it" } else { "them" }
                ),
            ));
        }
    }
    for its in child.controls.keys() {
        if template.control(its).is_none() {
            errors.push(Problem::new(
                vec![key("controls"), key(its)],
                Code::UnknownControl,
                format!("{:?} has no control {its:?}", template.name),
            ));
        }
    }
    errors
}

/// Every component with a child of `template`, by name, with the child's
/// slug: they're its instances too (composition spec §5).
pub fn containers_of<'a>(
    template: &str,
    components: &'a BTreeMap<ComponentId, Component>,
) -> impl Iterator<Item = (&'a Component, &'a str, &'a Child)> {
    components
        .values()
        .filter(|c| !c.is_instance())
        .flat_map(move |c| {
            c.children
                .iter()
                .filter(move |(_, ch)| ch.template == template)
                .map(move |(slug, ch)| (c, slug.as_str(), ch))
        })
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
                "a component's params hold values for now; ones that hold a component aren't built yet",
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
        let mut broken = compose::broken_instances(
            &component.params,
            instances.map(|c| (c.name.as_str(), &c.param_bindings, false)),
        );
        // Children are instances too, and must still fit (composition spec §8).
        for (container, slug, child) in containers_of(&component.id, ctx.components) {
            if container.id == component.id {
                continue;
            }
            let why: Vec<String> = child_problems(child, component, container, ctx.evaluator)
                .into_iter()
                .map(|p| p.message)
                .collect();
            if !why.is_empty() {
                broken.push((format!("{slug}, in {}", container.name), why));
            }
        }
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
