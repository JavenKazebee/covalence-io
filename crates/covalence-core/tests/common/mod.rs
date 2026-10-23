//! Helpers shared by the core's integration tests.

use covalence_core::surface::{InputCapKind, Layer, LayerControl, OutputCapKind, Placement};

/// A layer of `cols × rows` keys, `k0` … in row-major order, each
/// `momentary` with an `image`: what the virtual deck has.
pub fn deck_layer(id: &str, slug: &str, cols: u32, rows: u32) -> Layer {
    Layer {
        id: id.into(),
        name: slug.into(),
        slug: slug.into(),
        controls: (0..cols * rows)
            .map(|i| LayerControl {
                id: format!("k{i}"),
                at: Some(Placement::at(i % cols, i / cols)),
                inputs: vec![InputCapKind::Momentary],
                outputs: vec![OutputCapKind::Image],
            })
            .collect(),
    }
}
