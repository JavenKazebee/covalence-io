//! Helpers shared by the core's integration tests.

#![allow(dead_code)] // Each test file uses a different part.

use covalence_core::binding::ControlBinding;
use covalence_core::component::{Component, Show};
use covalence_core::flow::Problem;
use covalence_core::surface::{ControlSpec, InputCapKind, OutputCapKind, Placement};

/// A binding from JSON that names its component as `component`, as a component
/// would place it (composition spec §7.1).
pub fn placed(json: &serde_json::Value) -> Result<ControlBinding, Vec<Problem>> {
    let mut json = json.clone();
    let component = json
        .as_object_mut()
        .and_then(|o| o.remove("component"))
        .and_then(|l| l.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut b = ControlBinding::parse(&json)?;
    b.component = component;
    Ok(b)
}

/// A page of `cols × rows` keys, `k0` … in row-major order, each
/// `momentary` with an `image`: what the virtual deck has.
pub fn deck_page(id: &str, slug: &str, cols: u32, rows: u32) -> Component {
    Component {
        id: id.into(),
        name: slug.into(),
        slug: slug.into(),
        controls: (0..cols * rows)
            .map(|i| ControlSpec {
                id: format!("k{i}"),
                at: Some(Placement::at(i % cols, i / cols)),
                inputs: vec![InputCapKind::Momentary],
                outputs: vec![OutputCapKind::Image],
            })
            .collect(),
        show: Some(Show::default()),
        ..Component::default()
    }
}
