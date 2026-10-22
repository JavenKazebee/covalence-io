//! Key image snapshots (ADR 0023). Each look is drawn and compared pixel for
//! pixel with `tests/snapshots/keys/<name>.png`. Run with
//! `UPDATE_SNAPSHOTS=1` to write them again, then look at the images.

use std::path::PathBuf;

use covalence_core::surface::render::{Look, Renderer, Rgb};
use tiny_skia::Pixmap;

fn look(label: &str) -> Look {
    Look {
        label: label.into(),
        ..Look::default()
    }
}

fn cases() -> Vec<(&'static str, Look, u32)> {
    vec![
        ("blank", Look::default(), 72),
        ("short", look("CAM 1"), 72),
        ("wraps", look("Program return feed"), 72),
        (
            "too-long",
            look("A label far too long to fit on one small key at all"),
            72,
        ),
        ("one-long-word", look("Teleprompter"), 72),
        (
            "colours",
            Look {
                label: "LIVE".into(),
                text_color: Rgb::BLACK,
                background: Rgb(0xe0, 0x20, 0x20),
                stale: false,
            },
            72,
        ),
        (
            "stale",
            Look {
                label: "04:59".into(),
                text_color: Rgb::WHITE,
                background: Rgb(0x20, 0x60, 0xc0),
                stale: true,
            },
            72,
        ),
        ("virtual-deck", look("Out 1: 3"), 144),
        ("non-latin", look("Cámara ñ — Ωμέγα"), 72),
    ]
}

#[test]
fn keys_match_their_snapshots() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/keys");
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let mut renderer = Renderer::default();
    let mut failed = Vec::new();
    for (name, look, size) in cases() {
        let png = renderer.render(&look, size, size, "png", 0).unwrap();
        let path = dir.join(format!("{name}.png"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &png).unwrap();
            continue;
        }
        let expected = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_SNAPSHOTS=1", path.display()));
        let (got, want) = (
            Pixmap::decode_png(&png).unwrap(),
            Pixmap::decode_png(&expected).unwrap(),
        );
        if got.width() != want.width() || got.data() != want.data() {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "differ from their snapshots: {failed:?}");
}

#[test]
fn the_same_look_draws_the_same_bytes() {
    let mut renderer = Renderer::default();
    let a = renderer.render(&look("CAM 1"), 72, 72, "png", 0).unwrap();
    let b = renderer.render(&look("CAM 1"), 72, 72, "png", 0).unwrap();
    assert_eq!(a, b);
    assert_ne!(
        a,
        renderer.render(&look("CAM 2"), 72, 72, "png", 0).unwrap()
    );
}
