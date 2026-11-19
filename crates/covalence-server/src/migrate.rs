//! Migrates stored projects to the shapes the core reads now, before any
//! entity is loaded.
//!
//! Layers became components (composition spec §7.4): the `layer` state
//! root is `component`, a surface's resolution results are `shown` and
//! `.controls.<id>.component`, an event source's `layer` is `component`,
//! `trigger.layer` is `trigger.component`, a surface's `startLayer` is
//! `startComponent`, and a helper's `scope: "layer"` is `"component"`.
//!
//! Each stored layer, with its control bindings, becomes a component: its
//! `on`, `when`, `level`, `rank` and `cascade` move under `show`, and its
//! bindings are embedded without their `layer`.
//!
//! Stored entities are rewritten as JSON, by shape, and written back in one
//! transaction. Expressions are rewritten where they name one of those by a
//! dotted name; a path built at run time isn't found, so each rewritten
//! expression is logged as a warning, to be checked.

use covalence_store::Store;
use serde_json::{Map, Value};

/// The entity types whose JSON can hold what's renamed.
const KINDS: &[&str] = &["flow", "surface", "layer", "control_binding", "variable"];

/// Keys whose string values are expressions, wherever they appear.
const EXPR_KEYS: &[&str] = &[
    "expr", "when", "until", "filter", "value", "error", "source", "key",
];

/// Rewrites every stored entity that needs it, in one transaction, and
/// returns a warning per rewritten expression or dropped binding.
pub(crate) fn migrate(store: &Store) -> Result<Vec<String>, covalence_store::StoreError> {
    let mut warnings = Vec::new();
    // (type, ID) → its new body, or `None` to delete it.
    let mut writes: Vec<(&str, String, Option<String>)> = Vec::new();
    let mut layers: Vec<(String, Value)> = Vec::new();
    let mut bindings: Vec<(String, Value)> = Vec::new();
    for &kind in KINDS {
        for row in store.entities(kind)? {
            let Ok(mut json) = serde_json::from_str::<Value>(&row.body) else {
                continue;
            };
            let mut rewritten = Vec::new();
            let changed = rewrite(&mut json, &mut Vec::new(), &mut rewritten);
            for (at, before, after) in rewritten {
                warnings.push(format!(
                    "{kind} {} at {at}: rewrote {before:?} as {after:?}",
                    row.id
                ));
            }
            match kind {
                LAYER => layers.push((row.id, json)),
                BINDING => bindings.push((row.id, json)),
                _ if changed => writes.push((kind, row.id, Some(json.to_string()))),
                _ => {}
            }
        }
    }
    for (id, body) in components(&layers, &bindings, &mut warnings) {
        writes.push((COMPONENT, id, Some(body.to_string())));
    }
    for (id, _) in &layers {
        writes.push((LAYER, id.clone(), None));
    }
    for (id, _) in &bindings {
        writes.push((BINDING, id.clone(), None));
    }
    if !writes.is_empty() {
        let writes: Vec<(&str, &str, Option<&str>)> = writes
            .iter()
            .map(|(kind, id, body)| (*kind, id.as_str(), body.as_deref()))
            .collect();
        store.write_entities(&writes)?;
    }
    Ok(warnings)
}

const LAYER: &str = "layer";
const BINDING: &str = "control_binding";
const COMPONENT: &str = "component";

/// Each layer as a component, with its bindings embedded in ID order. A
/// binding whose layer is gone is dropped, with a warning.
fn components(
    layers: &[(String, Value)],
    bindings: &[(String, Value)],
    warnings: &mut Vec<String>,
) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = layers
        .iter()
        .map(|(id, layer)| {
            let mut component = Map::new();
            let mut show = Map::new();
            if let Value::Object(fields) = layer {
                for (k, v) in fields {
                    if ["on", "when", "level", "rank", "cascade"].contains(&k.as_str()) {
                        show.insert(k.clone(), v.clone());
                    } else {
                        component.insert(k.clone(), v.clone());
                    }
                }
            }
            component.insert("bindings".into(), Value::Array(Vec::new()));
            component.insert("show".into(), Value::Object(show));
            (id.clone(), Value::Object(component))
        })
        .collect();
    let mut sorted: Vec<&(String, Value)> = bindings.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (id, binding) in sorted {
        let layer = binding.get("layer").and_then(Value::as_str).unwrap_or("");
        match out.iter_mut().find(|(l, _)| l == layer) {
            Some((_, component)) => {
                let mut b = binding.clone();
                if let Value::Object(fields) = &mut b {
                    fields.remove("layer");
                }
                if let Some(Value::Array(list)) = component.get_mut("bindings") {
                    list.push(b);
                }
            }
            None => warnings.push(format!(
                "control_binding {id}: dropped, since its layer {layer:?} is gone"
            )),
        }
    }
    for (_, component) in &mut out {
        if let Value::Object(fields) = component
            && fields
                .get("bindings")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        {
            fields.remove("bindings");
        }
    }
    out
}

type Rewritten = Vec<(String, String, String)>;

/// Rewrites one value in place; whether anything changed. Expressions that
/// changed are added to `rewritten` with where they are.
fn rewrite(value: &mut Value, at: &mut Vec<String>, rewritten: &mut Rewritten) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            changed |= rename_fields(map);
            for (k, v) in map.iter_mut() {
                at.push(k.clone());
                if EXPR_KEYS.contains(&k.as_str()) {
                    changed |= rewrite_expr_value(v, at, rewritten);
                } else if k == "conditions" {
                    if let Value::Array(items) = v {
                        for (i, item) in items.iter_mut().enumerate() {
                            at.push(i.to_string());
                            changed |= rewrite_expr_value(item, at, rewritten);
                            at.pop();
                        }
                    }
                } else if k == "match" {
                    if let Value::String(pattern) = v
                        && let Some(new) = rename_pattern(pattern)
                    {
                        rewritten.push((at.join("."), pattern.clone(), new.clone()));
                        *pattern = new;
                        changed = true;
                    }
                } else {
                    changed |= rewrite(v, at, rewritten);
                }
                at.pop();
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                at.push(i.to_string());
                changed |= rewrite(item, at, rewritten);
                at.pop();
            }
        }
        _ => {}
    }
    changed
}

/// An expression string, or an object holding more.
fn rewrite_expr_value(value: &mut Value, at: &mut Vec<String>, rewritten: &mut Rewritten) -> bool {
    match value {
        Value::String(e) => match rename_expr(e) {
            Some(new) => {
                rewritten.push((at.join("."), e.clone(), new.clone()));
                *e = new;
                true
            }
            None => false,
        },
        other => rewrite(other, at, rewritten),
    }
}

/// Renamed fields in one object, by its shape.
fn rename_fields(map: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    // An event source on a layer (dataflow spec §3.1): `{layer, controls?}`.
    if map.contains_key("layer") && map.keys().all(|k| k == "layer" || k == "controls") {
        let layer = map.remove("layer").expect("checked");
        map.insert("component".into(), layer);
        changed = true;
    }
    // The core's `navigate` action, in a step or a sink (core actions §3.1).
    if map.get("action") == Some(&Value::String("navigate".into()))
        && let Some(Value::Object(params)) = map.get_mut("params")
        && let Some(layer) = params.remove("layer")
    {
        params.insert("component".into(), layer);
        changed = true;
    }
    // A surface's start.
    if let Some(start) = map.remove("startLayer") {
        map.insert("startComponent".into(), start);
        changed = true;
    }
    // A helper's scope.
    if map.contains_key("use") && map.get("scope") == Some(&Value::String("layer".into())) {
        map.insert("scope".into(), "component".into());
        changed = true;
    }
    // A ref: `{root: "layer"}`, and resolution results under `surface` or `self`.
    if map.get("root") == Some(&Value::String("layer".into())) {
        map.insert("root".into(), "component".into());
        changed = true;
    }
    let skip = match map.get("root").or_else(|| map.get("scope")) {
        Some(Value::String(r)) if r == "surface" => Some(1),
        Some(Value::String(r)) if r == "self" => Some(0),
        _ => None,
    };
    if let Some(skip) = skip
        && let Some(Value::Array(path)) = map.get_mut("path")
    {
        let mut segs: Vec<String> = path
            .iter()
            .map(|s| s.as_str().unwrap_or("*").to_owned())
            .collect();
        if rename_resolution(&mut segs, skip) {
            for (seg, new) in path.iter_mut().zip(segs) {
                if seg.is_string() {
                    *seg = Value::String(new);
                }
            }
            changed = true;
        }
    }
    changed
}

/// Renames a resolution result in a path, after `skip` leading segments:
/// `layers` as `shown`, `controls.<id>.layer` as `controls.<id>.component`.
fn rename_resolution(segs: &mut [String], skip: usize) -> bool {
    match segs.get(skip).map(String::as_str) {
        Some("layers") => {
            segs[skip] = "shown".into();
            true
        }
        Some("controls") if segs.get(skip + 2).map(String::as_str) == Some("layer") => {
            segs[skip + 2] = "component".into();
            true
        }
        _ => false,
    }
}

/// A text path pattern (`surface.*.controls.*.layer`) with what was renamed.
fn rename_pattern(pattern: &str) -> Option<String> {
    let mut segs: Vec<String> = pattern.split('.').map(str::to_owned).collect();
    let changed = match segs.first().map(String::as_str) {
        Some("layer") => {
            segs[0] = "component".into();
            true
        }
        Some("surface") => rename_resolution(&mut segs, 2),
        _ => false,
    };
    changed.then(|| segs.join("."))
}

/// One name in a dotted chain, with where it is in the expression.
struct Seg {
    start: usize,
    end: usize,
    name: String,
}

/// An expression with renamed roots and resolution results, or `None` if it
/// names none. Strings aren't told apart from code, so a string literal
/// that looks like one is renamed too: that's why each is a warning.
fn rename_expr(e: &str) -> Option<String> {
    let chars: Vec<char> = e.chars().collect();
    let mut edits: Vec<(usize, usize, &str)> = Vec::new();
    for chain in chains(&chars) {
        let name = |i: usize| chain.get(i).map(|s| s.name.as_str());
        match name(0) {
            Some("layer") if chain.len() > 1 => {
                edits.push((chain[0].start, chain[0].end, "component"));
            }
            Some("trigger") if name(1) == Some("layer") => {
                edits.push((chain[1].start, chain[1].end, "component"));
            }
            Some("self" | "surface") => {
                let skip = if name(0) == Some("self") { 1 } else { 2 };
                match name(skip) {
                    Some("layers") => edits.push((chain[skip].start, chain[skip].end, "shown")),
                    Some("controls") if name(skip + 2) == Some("layer") => {
                        let s = &chain[skip + 2];
                        edits.push((s.start, s.end, "component"));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out: Vec<char> = chars.clone();
    for (start, end, new) in edits.into_iter().rev() {
        out.splice(start..end, new.chars());
    }
    Some(out.into_iter().collect())
}

/// The dotted chains in an expression, `a.b[c].d` as `[a, b, *, d]`, with
/// each name's position. A bracketed segment is `*`, with no position to
/// rewrite.
fn chains(chars: &[char]) -> Vec<Vec<Seg>> {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts = ident(chars[i])
            && !chars[i].is_ascii_digit()
            && (i == 0 || !(ident(chars[i - 1]) || chars[i - 1] == '.'));
        if !starts {
            i += 1;
            continue;
        }
        let mut chain = Vec::new();
        loop {
            let start = i;
            while i < chars.len() && ident(chars[i]) {
                i += 1;
            }
            chain.push(Seg {
                start,
                end: i,
                name: chars[start..i].iter().collect(),
            });
            while i < chars.len() && chars[i] == '[' {
                let mut depth = 0;
                while i < chars.len() {
                    match chars[i] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                chain.push(Seg {
                    start: i,
                    end: i,
                    name: "*".into(),
                });
            }
            let dot = if chars.get(i) == Some(&'.') {
                1
            } else if chars.get(i) == Some(&'?') && chars.get(i + 1) == Some(&'.') {
                2
            } else {
                0
            };
            if dot > 0 && chars.get(i + dot).is_some_and(|&c| ident(c)) {
                i += dot;
                continue;
            }
            break;
        }
        out.push(chain);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renames_roots_and_results_in_expressions() {
        let cases = [
            (
                "layer.cams.controls.dial1.position > 0.5",
                Some("component.cams.controls.dial1.position > 0.5"),
            ),
            ("trigger.layer == 'x'", Some("trigger.component == 'x'")),
            ("self.layers.length", Some("self.shown.length")),
            ("self.controls.k3.layer", Some("self.controls.k3.component")),
            (
                "surface.deck.controls['k3'].layer == 'cams'",
                Some("surface.deck.controls['k3'].component == 'cams'"),
            ),
            ("surface.deck?.layers[0]", Some("surface.deck?.shown[0]")),
            ("vars.layer + layer", None),
            ("x.layer.y", None),
            ("surface.deck.controls.k3.pressed", None),
        ];
        for (before, after) in cases {
            assert_eq!(rename_expr(before).as_deref(), after, "{before}");
        }
    }

    #[test]
    fn renames_shapes() {
        let mut flow = json!({
            "triggers": [
                { "kind": "event", "from": { "layer": "L", "controls": "k0" }, "event": "press",
                  "filter": "trigger.layer == 'L'" },
                { "kind": "state", "match": "surface.*.controls.*.layer", "edge": "change" },
                { "kind": "state", "when": { "ref": { "root": "surface", "path": ["deck", "layers"] } }, "edge": "change" }
            ],
            "conditions": ["layer.cams.params.x"],
            "body": [{ "id": "s", "kind": "set", "path": "vars.x", "value": "layer.cams.controls.k0.toggle" }]
        });
        let mut rewritten = Vec::new();
        assert!(rewrite(&mut flow, &mut Vec::new(), &mut rewritten));
        assert_eq!(
            flow["triggers"][0]["from"],
            json!({ "component": "L", "controls": "k0" })
        );
        assert_eq!(flow["triggers"][0]["filter"], "trigger.component == 'L'");
        assert_eq!(
            flow["triggers"][1]["match"],
            "surface.*.controls.*.component"
        );
        assert_eq!(
            flow["triggers"][2]["when"]["ref"]["path"],
            json!(["deck", "shown"])
        );
        assert_eq!(flow["conditions"][0], "component.cams.params.x");
        assert_eq!(
            flow["body"][0]["value"],
            "component.cams.controls.k0.toggle"
        );
        assert_eq!(
            rewritten.len(),
            4,
            "refs are renamed by shape, not as expressions: {rewritten:?}"
        );

        let mut step = json!({ "kind": "action", "target": "core", "action": "navigate",
            "params": { "surface": "S", "layer": "L" } });
        assert!(rewrite(&mut step, &mut Vec::new(), &mut Vec::new()));
        assert_eq!(step["params"], json!({ "surface": "S", "component": "L" }));

        let mut surface = json!({ "id": "s", "startLayer": "L" });
        assert!(rewrite(&mut surface, &mut Vec::new(), &mut Vec::new()));
        assert_eq!(surface, json!({ "id": "s", "startComponent": "L" }));

        let mut binding = json!({ "layer": "L", "control": "k0", "props": {},
            "helpers": { "t": { "use": "toggle", "scope": "layer" } } });
        assert!(rewrite(&mut binding, &mut Vec::new(), &mut Vec::new()));
        assert_eq!(binding["layer"], "L", "a binding's own layer stays");
        assert_eq!(binding["helpers"]["t"]["scope"], "component");

        let mut again = flow.clone();
        assert!(
            !rewrite(&mut again, &mut Vec::new(), &mut Vec::new()),
            "idempotent"
        );
    }

    #[test]
    fn layers_and_their_bindings_become_components() {
        let store = Store::open_in_memory().unwrap();
        store
            .put_entity(
                "layer",
                "L",
                &json!({ "id": "L", "name": "Shift", "slug": "shift", "controls": [{ "id": "k0" }],
                    "level": "over", "cascade": "through", "when": { "while": { "expr": "layer.cams.controls.k0.on" } } })
                .to_string(),
            )
            .unwrap();
        store
            .put_entity(
                "layer",
                "P",
                &json!({ "id": "P", "name": "Page", "slug": "page", "controls": [] }).to_string(),
            )
            .unwrap();
        for (id, layer) in [("B2", "L"), ("B1", "L"), ("B9", "gone")] {
            let body = json!({ "id": id, "layer": layer, "control": "k0", "props": { "label": id },
                "helpers": { "t": { "use": "toggle", "scope": "layer" } } });
            store
                .put_entity("control_binding", id, &body.to_string())
                .unwrap();
        }
        let warnings = migrate(&store).unwrap();
        assert_eq!(
            warnings.len(),
            2,
            "an expression and a dropped binding: {warnings:?}"
        );
        let component = |id: &str| -> Value {
            serde_json::from_str(&store.entity("component", id).unwrap().unwrap().body).unwrap()
        };
        assert_eq!(
            component("L"),
            json!({ "id": "L", "name": "Shift", "slug": "shift", "controls": [{ "id": "k0" }],
                "bindings": [
                    { "id": "B1", "control": "k0", "props": { "label": "B1" }, "helpers": { "t": { "use": "toggle", "scope": "component" } } },
                    { "id": "B2", "control": "k0", "props": { "label": "B2" }, "helpers": { "t": { "use": "toggle", "scope": "component" } } }
                ],
                "show": { "level": "over", "cascade": "through", "when": { "while": { "expr": "component.cams.controls.k0.on" } } } })
        );
        assert_eq!(
            component("P"),
            json!({ "id": "P", "name": "Page", "slug": "page", "controls": [], "show": {} })
        );
        assert!(store.entities("layer").unwrap().is_empty());
        assert!(store.entities("control_binding").unwrap().is_empty());
        // Each parses as the core reads it.
        covalence_core::component::Component::parse(&component("L")).unwrap();
        assert!(migrate(&store).unwrap().is_empty(), "nothing left to do");
    }

    #[test]
    fn migrates_a_store_in_place() {
        let store = Store::open_in_memory().unwrap();
        store
            .put_entity("surface", "s", r#"{"id":"s","startLayer":"L"}"#)
            .unwrap();
        store
            .put_entity(
                "variable",
                "v",
                r#"{"id":"v","value":{"expr":"layer.a.b"}}"#,
            )
            .unwrap();
        let warnings = migrate(&store).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            store
                .entity("surface", "s")
                .unwrap()
                .unwrap()
                .body
                .contains("startComponent")
        );
        assert!(migrate(&store).unwrap().is_empty());
    }
}
