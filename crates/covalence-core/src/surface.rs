//! Surfaces and layers (surfaces spec, ADR 0022).
//!
//! [`Surfaces`] holds the `surface` and `layer` entities and the surfaces
//! attached right now. It doesn't know how a surface is connected: a
//! transport (the web surface's WebSocket, later the plugin host) attaches
//! one with its controls, feeds it [`Input`] and sends on the [`Draw`]s it
//! gets back.
//!
//! What a surface shows is drawn by the binding worker
//! ([`crate::binding::worker`]), which evaluates the bindings of the layer it
//! shows.
//!
//! Each attached surface has a task that applies its input to the controls'
//! input state, `surface.<slug>.controls.<id>.*`, as `state.patch`es with the
//! surface as origin (surfaces spec §4.1). Each change gives its edge events
//! ([`gesture`]), journaled as `input.event` with `cause` → the patch. Flows
//! match `gesture` triggers on those entries. Key, touch and encoder changes
//! each get a scheduler turn of their own: the task waits for the state's
//! consumers to see one before it applies the next. The rest of the
//! surface's state, `surface.<slug>.*`, is written through the journal as
//! `state.patch` too.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use covalence_protocol::state::PathSegment;
use covalence_protocol::{True, Value};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::journal::{
    ControlEvent, InputEventRecord, InputKind as SentKind, JournalError, JournalOp, NewEntry,
    Origin, Record, Seq, StatePatchRecord,
};
use crate::state::{Meta, Quality, Source, StatePath};
use crate::{Shared, clock};

pub mod gesture;
pub mod render;

use crate::binding::ControlBinding;
use crate::binding::worker::{Bindings, Problems};
use crate::flow::{Code, Problem, Report, is_identifier};
use gesture::{Change, Detected, Recogniser, Timing};

/// Entity types in the store.
pub const SURFACE_ENTITY: &str = "surface";
pub const LAYER_ENTITY: &str = "layer";
/// The state root for surfaces: `{conn: "surface", path: [slug, ...]}`.
pub const STATE_ROOT: &str = "surface";
/// How many draws wait for a slow transport before more are dropped.
const DRAW_BACKLOG: usize = 256;
/// How much input waits for a busy core.
const INPUT_BACKLOG: usize = 256;
/// The most controls a surface can declare.
const MAX_CONTROLS: usize = 1024;
/// The largest image output, in pixels per side.
const MAX_IMAGE: u32 = 1024;
/// How long an input edge waits for the state's consumers to see the one
/// before it, so that a stuck consumer can't stop a surface.
const TURN_LIMIT: Duration = Duration::from_millis(50);
/// How often dropped input is logged.
const DROP_LOG_EVERY: Duration = Duration::from_secs(1);

pub type SurfaceId = String;
pub type LayerId = String;

/// A control device (surfaces spec §1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Surface {
    /// A ULID, assigned by the server.
    #[serde(default)]
    pub id: SurfaceId,
    pub name: String,
    /// For state paths: `surface.<slug>.connected`.
    #[serde(default)]
    pub slug: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Shown when the surface first attaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_layer: Option<LayerId>,
}

/// A set of control bindings on the controls it covers (surfaces spec §1).
/// Every layer is in the `main` navigation group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layer {
    #[serde(default)]
    pub id: LayerId,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    pub controls: Vec<LayerControl>,
}

/// A control a layer covers, and what it needs that control to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerControl {
    /// Matches a [`ControlDef::id`] on the surfaces it fits.
    pub id: String,
    /// Where the editor draws it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Placement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputCapKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputCapKind>,
}

impl Layer {
    pub fn control(&self, id: &str) -> Option<&LayerControl> {
        self.controls.iter().find(|c| c.id == id)
    }
}

/// An [`InputCap`]'s kind: what a layer can ask a control for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputCapKind {
    Momentary,
    Relative,
    Absolute,
    Touch,
}

/// An [`OutputCap`]'s kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputCapKind {
    Image,
    Led,
    Level,
    Text,
}

impl InputCapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InputCapKind::Momentary => "momentary",
            InputCapKind::Relative => "relative",
            InputCapKind::Absolute => "absolute",
            InputCapKind::Touch => "touch",
        }
    }
}

impl OutputCapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OutputCapKind::Image => "image",
            OutputCapKind::Led => "led",
            OutputCapKind::Level => "level",
            OutputCapKind::Text => "text",
        }
    }
}

/// One control on a surface, typed by what it can do (surfaces spec §2,
/// plugin protocol §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlDef {
    /// Unique on the surface: `k0`, `dial1`, `pad.36`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub at: Placement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputCap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputCap>,
}

/// Where a control is, in grid cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
}

impl Placement {
    pub fn at(x: u32, y: u32) -> Self {
        Placement {
            x,
            y,
            w: None,
            h: None,
        }
    }

    fn w(&self) -> u32 {
        self.w.unwrap_or(1)
    }

    fn h(&self) -> u32 {
        self.h.unwrap_or(1)
    }

    fn overlaps(&self, other: &Placement) -> bool {
        let (ax, ay, bx, by) = (
            u64::from(self.x),
            u64::from(self.y),
            u64::from(other.x),
            u64::from(other.y),
        );
        ax < bx + u64::from(other.w())
            && bx < ax + u64::from(self.w())
            && ay < by + u64::from(other.h())
            && by < ay + u64::from(self.h())
    }
}

/// What a control can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum InputCap {
    /// `down` and `up`.
    Momentary {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        velocity: bool,
    },
    /// `turn`: signed detents.
    Relative,
    /// `value`: 0 to 1, or `[x, y]` with two axes.
    Absolute {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axes: Option<u8>,
    },
    /// `touch`: 1 or 0.
    Touch,
}

impl InputCap {
    pub fn kind(&self) -> InputCapKind {
        match self {
            InputCap::Momentary { .. } => InputCapKind::Momentary,
            InputCap::Relative => InputCapKind::Relative,
            InputCap::Absolute { .. } => InputCapKind::Absolute,
            InputCap::Touch => InputCapKind::Touch,
        }
    }
}

/// What the core can set on a control. Only `image` does anything yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum OutputCap {
    Image(ImageDef),
    Led {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rgb: bool,
    },
    Level {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        motor: bool,
    },
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chars: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
    },
}

impl OutputCap {
    pub fn kind(&self) -> OutputCapKind {
        match self {
            OutputCap::Image(_) => OutputCapKind::Image,
            OutputCap::Led { .. } => OutputCapKind::Led,
            OutputCap::Level { .. } => OutputCapKind::Level,
            OutputCap::Text { .. } => OutputCapKind::Text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageDef {
    pub w: u32,
    pub h: u32,
    pub format: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: u16,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

impl ControlDef {
    /// The image the control shows, if it shows one.
    pub fn image(&self) -> Option<&ImageDef> {
        self.outputs.iter().find_map(|o| match o {
            OutputCap::Image(image) => Some(image),
            _ => None,
        })
    }

    fn input(&self, kind: InputCapKind) -> Option<&InputCap> {
        self.inputs.iter().find(|i| i.kind() == kind)
    }

    /// What a layer that lists this control copies from it.
    pub fn to_layer_control(&self) -> LayerControl {
        LayerControl {
            id: self.id.clone(),
            at: Some(self.at),
            inputs: self.inputs.iter().map(InputCap::kind).collect(),
            outputs: self.outputs.iter().map(OutputCap::kind).collect(),
        }
    }
}

/// `surface.attach`, without the surface: the transport knows which it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attach {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub name: String,
    pub controls: Vec<ControlDef>,
}

/// `surface.input`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub control: String,
    pub kind: SentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<InputValue>,
    /// The host's timestamp, in milliseconds on its own clock.
    pub ts: f64,
}

/// An input's `value`: velocity, detents, a position or a touch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    One(f64),
    Two([f64; 2]),
}

impl From<f64> for InputValue {
    fn from(v: f64) -> Self {
        InputValue::One(v)
    }
}

/// `surface.draw`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draw {
    pub control: String,
    pub format: String,
    pub image: Vec<u8>,
}

/// An attached surface's half of the connection to the core.
pub struct Link {
    /// Input from the device. Dropping it detaches the surface.
    pub input: mpsc::Sender<Input>,
    /// Images to show. Closes when the surface is detached by the core:
    /// replaced by a newer attach, or deleted.
    pub draws: mpsc::Receiver<Draw>,
}

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("no surface {0}")]
    UnknownSurface(SurfaceId),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Debug, thiserror::Error)]
pub enum ShowError {
    #[error("no surface {0}")]
    UnknownSurface(SurfaceId),
    #[error("no layer {0}")]
    UnknownLayer(LayerId),
    #[error("layer {layer} doesn't fit: the surface has no key for {}", missing.join(", "))]
    DoesNotFit {
        layer: LayerId,
        missing: Vec<String>,
    },
    #[error(transparent)]
    Journal(#[from] JournalError),
}

pub struct Surfaces {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Shared,
    timing: Timing,
    book: Mutex<Book>,
    bindings: Bindings,
    sessions: AtomicU64,
}

#[derive(Default)]
struct Book {
    surfaces: BTreeMap<SurfaceId, Surface>,
    layers: BTreeMap<LayerId, Layer>,
    attached: BTreeMap<SurfaceId, Attached>,
    /// The layer each surface last showed, kept while it's detached.
    shown: BTreeMap<SurfaceId, Option<LayerId>>,
    /// The controls each surface last attached with.
    controls: BTreeMap<SurfaceId, Vec<ControlDef>>,
}

struct Attached {
    /// Tells a replaced attach's task apart from the current one.
    session: u64,
    attach: Attach,
    draws: mpsc::Sender<Draw>,
    /// Dropped to stop the task when the attach is replaced or deleted.
    _stop: oneshot::Sender<()>,
}

impl Surfaces {
    pub fn new(shared: Shared, timing: Timing) -> Self {
        Surfaces {
            inner: Arc::new(Inner {
                bindings: Bindings::start(shared.state.clone()),
                shared,
                timing,
                book: Mutex::new(Book::default()),
                sessions: AtomicU64::new(0),
            }),
        }
    }

    /// Adds or replaces a surface entity. A new slug moves its state.
    pub fn set_surface(&self, surface: Surface) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        let old = book.surfaces.insert(surface.id.clone(), surface.clone());
        match old {
            Some(old) if old.slug != surface.slug => {
                let mut ops = vec![(StatePath::new(STATE_ROOT, vec![key(&old.slug)]), None)];
                ops.extend(inner.surface_state(&book, &surface.id));
                inner.patch(Origin::system(), None, ops)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Removes a surface entity, detaching it and deleting its state.
    pub fn remove_surface(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        let Some(surface) = book.surfaces.remove(id) else {
            return Ok(());
        };
        book.attached.remove(id);
        book.shown.remove(id);
        book.controls.remove(id);
        inner.bindings.hide(id);
        let path = StatePath::new(STATE_ROOT, vec![key(&surface.slug)]);
        inner.patch(Origin::system(), None, vec![(path, None)])?;
        Ok(())
    }

    /// Adds or replaces a layer. Surfaces showing it whose layout no longer
    /// fits stop showing it.
    pub fn set_layer(&self, layer: Layer) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        book.layers.insert(layer.id.clone(), layer.clone());
        let showing: Vec<SurfaceId> = book
            .shown
            .iter()
            .filter(|(_, p)| p.as_deref() == Some(layer.id.as_str()))
            .map(|(s, _)| s.clone())
            .collect();
        for surface in showing {
            let fits = book
                .attached
                .get(&surface)
                .is_none_or(|a| missing(&layer, &a.attach.controls).is_empty());
            if !fits {
                book.shown.insert(surface.clone(), None);
            }
            inner.publish_current(&book, &surface, Origin::system())?;
            inner.show_on(&book, &surface);
        }
        Ok(())
    }

    /// Removes a layer from the surfaces showing it.
    pub fn remove_layer(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        book.layers.remove(id);
        let showing: Vec<SurfaceId> = book
            .shown
            .iter()
            .filter(|(_, p)| p.as_deref() == Some(id))
            .map(|(s, _)| s.clone())
            .collect();
        for surface in showing {
            book.shown.insert(surface.clone(), None);
            inner.publish_current(&book, &surface, Origin::system())?;
            inner.show_on(&book, &surface);
        }
        Ok(())
    }

    /// Every layer, by ID, for validating triggers and bindings.
    pub fn layers(&self) -> BTreeMap<LayerId, Layer> {
        self.inner.book().layers.clone()
    }

    /// The controls a surface declared when it last attached, while this
    /// core has been running.
    pub fn controls(&self, surface: &str) -> Option<Vec<ControlDef>> {
        self.inner.book().controls.get(surface).cloned()
    }

    /// The layer a surface shows, or last showed while detached.
    pub fn shown(&self, surface: &str) -> Option<LayerId> {
        self.inner.book().shown.get(surface).cloned().flatten()
    }

    /// Adds or replaces a control binding.
    pub fn set_binding(&self, binding: ControlBinding) {
        self.inner.bindings.set(binding);
    }

    pub fn remove_binding(&self, id: &str) {
        self.inner.bindings.remove(id);
    }

    /// The connection aliases `expr` bindings read, by alias.
    pub fn set_aliases(&self, aliases: BTreeMap<String, crate::state::ConnId>) {
        self.inner.bindings.set_aliases(aliases);
    }

    /// The last error of each failing binding property.
    pub fn binding_problems(&self) -> Problems {
        self.inner.bindings.problems()
    }

    /// Returns once bindings have caught up with every change made before:
    /// layers evaluated and drawn.
    pub async fn flush(&self) {
        self.inner.bindings.flush().await;
    }

    pub fn is_attached(&self, surface: &str) -> bool {
        self.inner.book().attached.contains_key(surface)
    }

    /// Shows a layer on a surface, or nothing (surfaces spec §5). It must fit
    /// an attached surface (§5.4).
    pub fn show(
        &self,
        surface: &str,
        layer: Option<&str>,
        origin: Origin,
    ) -> Result<(), ShowError> {
        let inner = &self.inner;
        let mut book = inner.book();
        if !book.surfaces.contains_key(surface) {
            return Err(ShowError::UnknownSurface(surface.to_owned()));
        }
        if let Some(id) = layer {
            let layer = book
                .layers
                .get(id)
                .ok_or_else(|| ShowError::UnknownLayer(id.to_owned()))?;
            if let Some(attached) = book.attached.get(surface) {
                let missing = missing(layer, &attached.attach.controls);
                if !missing.is_empty() {
                    return Err(ShowError::DoesNotFit {
                        layer: id.to_owned(),
                        missing,
                    });
                }
            }
        }
        book.shown
            .insert(surface.to_owned(), layer.map(str::to_owned));
        inner.publish_current(&book, surface, origin)?;
        inner.show_on(&book, surface);
        Ok(())
    }

    /// Attaches a surface. A surface already attached is replaced: its
    /// [`Link::draws`] closes. Must be called inside a tokio runtime.
    pub fn attach(&self, surface: &str, attach: Attach) -> Result<Link, AttachError> {
        check_attach(&attach)?;
        let inner = &self.inner;
        let session = inner.sessions.fetch_add(1, Ordering::Relaxed);
        let (input_tx, input_rx) = mpsc::channel(INPUT_BACKLOG);
        let (draws_tx, draws_rx) = mpsc::channel(DRAW_BACKLOG);
        let (stop_tx, stop_rx) = oneshot::channel();

        let mut book = inner.book();
        let entity = book
            .surfaces
            .get(surface)
            .ok_or_else(|| AttachError::UnknownSurface(surface.to_owned()))?;
        // The layer it showed last, or its start layer the first time.
        let layer = match book.shown.get(surface) {
            Some(layer) => layer.clone(),
            None => entity.start_layer.clone(),
        }
        .filter(|p| {
            book.layers
                .get(p)
                .is_some_and(|p| missing(p, &attach.controls).is_empty())
        });
        book.shown.insert(surface.to_owned(), layer);
        book.controls
            .insert(surface.to_owned(), attach.controls.clone());
        book.attached.insert(
            surface.to_owned(),
            Attached {
                session,
                attach,
                draws: draws_tx,
                _stop: stop_tx,
            },
        );
        let mut ops = inner.surface_state(&book, surface);
        ops.extend(Inner::input_reset(&book, surface));
        inner.patch(surface_origin(surface), None, ops)?;
        inner.show_on(&book, surface);
        drop(book);

        tokio::spawn(run(
            inner.clone(),
            surface.to_owned(),
            session,
            input_rx,
            stop_rx,
        ));
        Ok(Link {
            input: input_tx,
            draws: draws_rx,
        })
    }
}

/// What makes an attach unusable (surfaces spec §2): too many controls, a
/// duplicate ID, a control that's empty, overlaps another or can do
/// nothing, or an image the renderer can't draw.
fn check_attach(attach: &Attach) -> Result<(), AttachError> {
    let invalid = |m: String| Err(AttachError::Invalid(m));
    if attach.controls.len() > MAX_CONTROLS {
        return invalid(format!(
            "a surface can have at most {MAX_CONTROLS} controls"
        ));
    }
    let mut seen = BTreeSet::new();
    for (i, control) in attach.controls.iter().enumerate() {
        let id = &control.id;
        if id.is_empty() {
            return invalid("a control has no id".into());
        }
        if !seen.insert(id) {
            return invalid(format!("control {id:?} appears twice"));
        }
        if control.at.w() == 0 || control.at.h() == 0 {
            return invalid(format!("control {id:?} has no size"));
        }
        if control.inputs.is_empty() && control.outputs.is_empty() {
            return invalid(format!("control {id:?} has no inputs or outputs"));
        }
        let mut kinds = BTreeSet::new();
        for kind in control.inputs.iter().map(InputCap::kind) {
            if !kinds.insert(kind) {
                return invalid(format!("control {id:?} has two {} inputs", kind.as_str()));
            }
        }
        if let Some(InputCap::Absolute { axes: Some(axes) }) = control.input(InputCapKind::Absolute)
            && !(1..=2).contains(axes)
        {
            return invalid(format!("control {id:?} has {axes} axes: use 1 or 2"));
        }
        let mut kinds = BTreeSet::new();
        for output in &control.outputs {
            if !kinds.insert(output.kind()) {
                return invalid(format!(
                    "control {id:?} has two {} outputs",
                    output.kind().as_str()
                ));
            }
            if let OutputCap::Image(image) = output
                && !((1..=MAX_IMAGE).contains(&image.w) && (1..=MAX_IMAGE).contains(&image.h))
            {
                return invalid(format!(
                    "control {id:?} has a {}×{} image: use 1 to {MAX_IMAGE} pixels a side",
                    image.w, image.h
                ));
            }
        }
        if let Some(other) = attach.controls[..i]
            .iter()
            .find(|o| o.at.overlaps(&control.at))
        {
            return invalid(format!("controls {:?} and {id:?} overlap", other.id));
        }
    }
    Ok(())
}

/// The layer's controls the surface lacks or can't support (surfaces spec
/// §5.4): each must be there with at least the inputs and outputs the layer
/// lists for it.
fn missing(layer: &Layer, controls: &[ControlDef]) -> Vec<String> {
    layer
        .controls
        .iter()
        .filter(|needed| {
            !controls.iter().any(|c| {
                c.id == needed.id
                    && needed.inputs.iter().all(|&k| c.input(k).is_some())
                    && needed
                        .outputs
                        .iter()
                        .all(|&k| c.outputs.iter().any(|o| o.kind() == k))
            })
        })
        .map(|c| c.id.clone())
        .collect()
}

impl Surface {
    /// Reads a surface from JSON, or one `invalid` problem.
    pub fn parse(json: &serde_json::Value) -> Result<Surface, Vec<Problem>> {
        crate::binding::deserialize(json, &[]).map_err(|p| vec![p])
    }
}

impl Layer {
    /// Reads a layer from JSON, or one `invalid` problem.
    pub fn parse(json: &serde_json::Value) -> Result<Layer, Vec<Problem>> {
        crate::binding::deserialize(json, &[]).map_err(|p| vec![p])
    }
}

/// Checks a surface: a name, a slug no other surface has, a model, and a
/// start layer that exists.
pub fn validate_surface(
    surface: &Surface,
    taken_slugs: &BTreeSet<String>,
    layers: &BTreeMap<LayerId, Layer>,
) -> Report {
    let mut errors = named("surface", &surface.name, &surface.slug, taken_slugs);
    if surface.model.trim().is_empty() {
        errors.push(Problem::new(
            vec![key("model")],
            Code::Invalid,
            "model is required",
        ));
    }
    if let Some(layer) = &surface.start_layer
        && !layers.contains_key(layer)
    {
        errors.push(Problem::new(
            vec![key("startLayer")],
            Code::UnknownLayer,
            format!("no layer {layer:?}"),
        ));
    }
    Report {
        errors,
        warnings: Vec::new(),
    }
}

/// The most controls a layer can list.
pub const MAX_LAYER_CONTROLS: usize = 256;

/// Checks a layer (surfaces spec §9): a name, a slug no other layer has, at
/// most 256 controls with unique IDs, and every control bound on it still
/// listed with the outputs its bindings drive. `bound` has those outputs by
/// control.
pub fn validate_layer(
    layer: &Layer,
    taken_slugs: &BTreeSet<String>,
    bound: &BTreeMap<String, BTreeSet<OutputCapKind>>,
) -> Report {
    let mut errors = named("layer", &layer.name, &layer.slug, taken_slugs);
    if layer.controls.len() > MAX_LAYER_CONTROLS {
        errors.push(Problem::new(
            vec![key("controls")],
            Code::Invalid,
            format!("a layer can have at most {MAX_LAYER_CONTROLS} controls"),
        ));
    }
    let mut seen = BTreeSet::new();
    for (i, control) in layer.controls.iter().enumerate() {
        let at = vec![key("controls"), PathSegment::Index(i as i64), key("id")];
        if control.id.is_empty() {
            errors.push(Problem::new(at, Code::Invalid, "a control needs an id"));
        } else if !seen.insert(&control.id) {
            errors.push(Problem::new(
                at,
                Code::DuplicateControl,
                format!("{:?} is listed twice", control.id),
            ));
        }
    }
    for (id, outputs) in bound {
        match layer.control(id) {
            None => errors.push(Problem::new(
                vec![key("controls")],
                Code::UnknownControl,
                format!("{id} is bound, but the layer doesn't list it"),
            )),
            Some(control) => {
                for output in outputs.iter().filter(|o| !control.outputs.contains(o)) {
                    errors.push(Problem::new(
                        vec![key("controls")],
                        Code::NoOutput,
                        format!(
                            "{id} has a binding that sets its {}, but the layer doesn't give it that output",
                            output.as_str()
                        ),
                    ));
                }
            }
        }
    }
    Report {
        errors,
        warnings: Vec::new(),
    }
}

/// The name and slug problems of a surface or layer.
fn named(kind: &str, name: &str, slug: &str, taken: &BTreeSet<String>) -> Vec<Problem> {
    let mut errors = Vec::new();
    if name.trim().is_empty() {
        errors.push(Problem::new(
            vec![key("name")],
            Code::Invalid,
            "name is required",
        ));
    }
    if !is_identifier(slug) {
        errors.push(Problem::new(
            vec![key("slug")],
            Code::BadSlug,
            format!(
                "slug {slug:?} must start with a lowercase letter and use only letters, digits and _"
            ),
        ));
    } else if taken.contains(slug) {
        errors.push(Problem::new(
            vec![key("slug")],
            Code::DuplicateSlug,
            format!("another {kind} already has slug {slug:?}"),
        ));
    }
    errors
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

fn surface_origin(surface: &str) -> Origin {
    Origin::Surface {
        surface: surface.to_owned(),
        orbital: None,
    }
}

/// An attached surface's task: input, the gesture timer, and the detach.
async fn run(
    inner: Arc<Inner>,
    surface: SurfaceId,
    session: u64,
    mut input: mpsc::Receiver<Input>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut task = Task {
        recogniser: Recogniser::new(inner.timing),
        inner,
        surface,
        session,
        controls: BTreeMap::new(),
        downs: BTreeMap::new(),
        drops: Drops::default(),
    };
    loop {
        let deadline = task.recogniser.deadline();
        tokio::select! {
            _ = &mut stop => {
                // Replaced or deleted: whoever did that owns the state now.
                let released = task.recogniser.release_all();
                task.journal_events(None, released, None);
                return;
            }
            next = input.recv() => match next {
                Some(i) => task.input(i).await,
                None => {
                    task.detach();
                    return;
                }
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                for found in task.recogniser.tick(Instant::now()) {
                    let (cause, layer) = task.downs.get(&found.control).cloned().unzip();
                    task.journal_events(layer.flatten().as_deref(), vec![found], cause);
                }
            }
        }
    }
}

/// An attached surface's input, as its task sees it.
struct Task {
    inner: Arc<Inner>,
    surface: SurfaceId,
    session: u64,
    recogniser: Recogniser,
    /// Each control's input state, as last written.
    controls: BTreeMap<String, InputState>,
    /// The patch of each pressed control's press, and the layer shown then:
    /// the cause and layer of its `long`, `hold` and `release`.
    downs: BTreeMap<String, (Seq, Option<LayerId>)>,
    drops: Drops,
}

#[derive(Default)]
struct InputState {
    pressed: bool,
    detents: i64,
    value: Option<Value>,
    touched: bool,
}

/// Dropped input, logged at most once every [`DROP_LOG_EVERY`].
#[derive(Default)]
struct Drops {
    logged: Option<Instant>,
    since: u32,
}

impl Drops {
    fn drop(&mut self, surface: &str, input: &Input, why: &str) {
        let now = Instant::now();
        if self.logged.is_some_and(|t| now < t + DROP_LOG_EVERY) {
            self.since += 1;
            return;
        }
        let also = std::mem::take(&mut self.since);
        tracing::warn!(%surface, control = %input.control, kind = ?input.kind, also, "dropped input: {why}");
        self.logged = Some(now);
    }
}

/// One change to a control's input state, ready to write.
struct Write {
    change: Change,
    ops: Vec<(String, Value)>,
    /// Whether it needs a scheduler turn of its own (surfaces spec §4.1).
    edge: bool,
}

impl Task {
    /// Applies one input to its control's input state, a turn at a time,
    /// and journals the events each change gives.
    async fn input(&mut self, input: Input) {
        let control = {
            let book = self.inner.book();
            let Some(attached) = book
                .attached
                .get(&self.surface)
                .filter(|a| a.session == self.session)
            else {
                return;
            };
            match attached
                .attach
                .controls
                .iter()
                .find(|c| c.id == input.control)
            {
                Some(control) => control.clone(),
                None => {
                    self.drops.drop(&self.surface, &input, "no such control");
                    return;
                }
            }
        };
        let writes = match self.writes(&control, &input) {
            Ok(writes) => writes,
            Err(why) => {
                self.drops.drop(&self.surface, &input, &why);
                return;
            }
        };
        for write in writes {
            let Some(turn) = self.write(&input, write) else {
                return;
            };
            if let Some(turn) = turn
                && !self.inner.shared.state.seen(turn, TURN_LIMIT).await
            {
                tracing::debug!(surface = %self.surface, "input went ahead of a slow state consumer");
            }
        }
    }

    /// What an input changes, or why it doesn't fit the control (plugin
    /// protocol §8).
    fn writes(&self, control: &ControlDef, input: &Input) -> Result<Vec<Write>, String> {
        let state = self.controls.get(&control.id);
        let needs = |kind: InputCapKind| {
            control
                .input(kind)
                .ok_or_else(|| format!("{:?} has no {} input", control.id, kind.as_str()))
        };
        let one = || match input.value {
            Some(InputValue::One(v)) if v.is_finite() => Ok(v),
            _ => Err("value must be a number".to_owned()),
        };
        let pressed = |down: bool, velocity: Option<f64>| {
            let mut ops = vec![("pressed".to_owned(), down.into())];
            if let Some(v) = velocity {
                ops.push(("velocity".to_owned(), v.clamp(0.0, 1.0).into()));
            }
            Write {
                change: Change::Pressed(down),
                ops,
                edge: true,
            }
        };
        let was_pressed = state.is_some_and(|s| s.pressed);
        Ok(match input.kind {
            SentKind::Down => {
                let InputCap::Momentary { velocity } = needs(InputCapKind::Momentary)? else {
                    unreachable!()
                };
                let velocity = if *velocity { Some(one()?) } else { None };
                let mut writes = Vec::new();
                // A missed `up`: release before pressing again.
                if was_pressed {
                    writes.push(pressed(false, None));
                }
                writes.push(pressed(true, velocity));
                writes
            }
            SentKind::Up => {
                needs(InputCapKind::Momentary)?;
                if was_pressed {
                    vec![pressed(false, None)]
                } else {
                    Vec::new()
                }
            }
            SentKind::Turn => {
                needs(InputCapKind::Relative)?;
                let n = one()?;
                if n.fract() != 0.0 || n.abs() > 1e9 {
                    return Err("a turn must be a whole number of detents".into());
                }
                let n = n as i64;
                if n == 0 {
                    return Ok(Vec::new());
                }
                let detents = state.map_or(0, |s| s.detents) + n;
                vec![Write {
                    change: Change::Turned(n),
                    ops: vec![("detents".to_owned(), detents.into())],
                    edge: true,
                }]
            }
            SentKind::Value => {
                let InputCap::Absolute { axes } = needs(InputCapKind::Absolute)? else {
                    unreachable!()
                };
                let unit = |v: f64| v.is_finite().then(|| v.clamp(0.0, 1.0));
                let value: Value = match (axes.unwrap_or(1), input.value) {
                    (1, Some(InputValue::One(v))) => unit(v).map(Value::from),
                    (2, Some(InputValue::Two([x, y]))) => unit(x)
                        .zip(unit(y))
                        .map(|(x, y)| Value::Array(vec![x.into(), y.into()])),
                    _ => None,
                }
                .ok_or_else(|| match axes {
                    Some(2) => "value must be [x, y], each 0 to 1".to_owned(),
                    _ => "value must be a number from 0 to 1".to_owned(),
                })?;
                if state.and_then(|s| s.value.as_ref()) == Some(&value) {
                    return Ok(Vec::new());
                }
                vec![Write {
                    change: Change::Value(value.clone()),
                    ops: vec![("value".to_owned(), value)],
                    edge: false,
                }]
            }
            SentKind::Touch => {
                needs(InputCapKind::Touch)?;
                let touched = one()? != 0.0;
                if state.is_some_and(|s| s.touched) == touched {
                    return Ok(Vec::new());
                }
                vec![Write {
                    change: Change::Touched(touched),
                    ops: vec![("touched".to_owned(), touched.into())],
                    edge: true,
                }]
            }
        })
    }

    /// Writes one change and journals its events. `None` if the surface is
    /// no longer this task's, else the turn to wait for, if it's an edge.
    fn write(&mut self, input: &Input, write: Write) -> Option<Option<u64>> {
        let (slug, layer) = {
            let book = self.inner.book();
            book.attached
                .get(&self.surface)
                .filter(|a| a.session == self.session)?;
            (
                book.surfaces[&self.surface].slug.clone(),
                book.shown.get(&self.surface).cloned().flatten(),
            )
        };
        let ops = write
            .ops
            .into_iter()
            .map(|(leaf, value)| {
                (
                    Inner::control_path(&slug, &input.control, &leaf),
                    Some(value),
                )
            })
            .collect();
        let (seq, turn) =
            match self
                .inner
                .patch_turn(surface_origin(&self.surface), None, ops, Some(input.ts))
            {
                Ok(written) => written,
                Err(e) => {
                    tracing::error!(surface = %self.surface, "cannot journal input: {e}");
                    return None;
                }
            };
        let state = self.controls.entry(input.control.clone()).or_default();
        // The layer of a release is its press's; of anything else, the layer
        // shown now.
        let layer = match &write.change {
            Change::Pressed(true) => {
                state.pressed = true;
                self.downs
                    .insert(input.control.clone(), (seq, layer.clone()));
                layer
            }
            Change::Pressed(false) => {
                state.pressed = false;
                self.downs
                    .remove(&input.control)
                    .map_or(layer, |(_, layer)| layer)
            }
            Change::Turned(n) => {
                state.detents += n;
                layer
            }
            Change::Value(v) => {
                state.value = Some(v.clone());
                layer
            }
            Change::Touched(t) => {
                state.touched = *t;
                layer
            }
        };
        let found = self
            .recogniser
            .change(&input.control, write.change, input.ts, Instant::now());
        self.journal_events(layer.as_deref(), found, Some(seq));
        Some(write.edge.then_some(turn))
    }

    fn journal_events(&self, layer: Option<&str>, found: Vec<Detected>, cause: Option<Seq>) {
        for Detected {
            control,
            event,
            delta,
            value,
        } in found
        {
            // A release from a timer or detach carries its press's layer.
            let layer = match (event, self.downs.get(&control)) {
                (ControlEvent::Release, Some((_, pressed_on))) => pressed_on.as_deref(),
                _ => layer,
            };
            let entry = NewEntry::new(
                surface_origin(&self.surface),
                Record::InputEvent(InputEventRecord {
                    surface: self.surface.clone(),
                    layer: layer.map(str::to_owned),
                    control,
                    event,
                    delta,
                    value,
                }),
            )
            .cause(cause);
            if let Err(e) = self.inner.shared.journal.append(entry) {
                tracing::error!(surface = %self.surface, "cannot journal an event: {e}");
            }
        }
    }

    /// The transport went away: `connected` becomes `false`, then whatever
    /// was pressed or touched is released with `cause` → that change, and
    /// its events with `cause` → the release.
    fn detach(&mut self) {
        let inner = &self.inner;
        let mut book = inner.book();
        if book
            .attached
            .get(&self.surface)
            .is_none_or(|a| a.session != self.session)
        {
            return;
        }
        book.attached.remove(&self.surface);
        inner.bindings.hide(&self.surface);
        let layer = book.shown.get(&self.surface).cloned().flatten();
        let slug = book.surfaces[&self.surface].slug.clone();
        drop(book);
        let origin = surface_origin(&self.surface);
        let path = Inner::path(&slug, "connected");
        let connected = match inner.patch(origin.clone(), None, vec![(path, Some(false.into()))]) {
            Ok(seq) => seq,
            Err(e) => {
                tracing::error!(surface = %self.surface, "cannot journal a detach: {e}");
                return;
            }
        };
        let mut ops = Vec::new();
        for (id, state) in &self.controls {
            if state.pressed {
                ops.push((
                    Inner::control_path(&slug, id, "pressed"),
                    Some(false.into()),
                ));
            }
            if state.touched {
                ops.push((
                    Inner::control_path(&slug, id, "touched"),
                    Some(false.into()),
                ));
            }
        }
        let cause = if ops.is_empty() {
            connected
        } else {
            match inner.patch(origin, Some(connected), ops) {
                Ok(seq) => seq,
                Err(e) => {
                    tracing::error!(surface = %self.surface, "cannot journal a release: {e}");
                    return;
                }
            }
        };
        let released = self.recogniser.release_all();
        self.journal_events(layer.as_deref(), released, Some(cause));
    }
}

impl Inner {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn meta(seq: Seq) -> Meta {
        Meta {
            ts: clock::epoch_ms(),
            seq,
            quality: Quality::Good,
            source: Source::Core,
        }
    }

    /// Journals a `state.patch` and applies it. `None` deletes a path.
    fn patch(
        &self,
        origin: Origin,
        cause: Option<Seq>,
        ops: Vec<(StatePath, Option<Value>)>,
    ) -> Result<Seq, JournalError> {
        self.patch_turn(origin, cause, ops, None)
            .map(|(seq, _)| seq)
    }

    /// [`patch`](Self::patch), with control input's `deviceTs`, and the
    /// state turn it published.
    fn patch_turn(
        &self,
        origin: Origin,
        cause: Option<Seq>,
        ops: Vec<(StatePath, Option<Value>)>,
        device_ts: Option<f64>,
    ) -> Result<(Seq, u64), JournalError> {
        let record = Record::StatePatch(StatePatchRecord {
            ops: ops
                .iter()
                .map(|(path, value)| match value {
                    Some(value) => JournalOp::Set {
                        path: path.clone(),
                        value: value.clone(),
                    },
                    None => JournalOp::Delete {
                        path: path.clone(),
                        delete: True,
                    },
                })
                .collect(),
            replaces: None,
            device_ts,
        });
        let journal = &self.shared.journal;
        let (seq, turn) = self.shared.state.mutate_turn(|tree| {
            let seq = match journal.append(NewEntry::new(origin, record).cause(cause)) {
                Ok(e) => e.seq,
                Err(e) => return (Err(e), Default::default()),
            };
            let mut dirty = crate::state::Dirty::new();
            for (path, value) in &ops {
                dirty.extend(match value {
                    Some(v) => tree.set(path, v, &Self::meta(seq)),
                    None => tree.delete(path),
                });
            }
            (Ok(seq), dirty)
        });
        Ok((seq?, turn))
    }

    fn path(slug: &str, leaf: &str) -> StatePath {
        StatePath::new(STATE_ROOT, vec![key(slug), key(leaf)])
    }

    /// `surface.<slug>.controls.<id>.<leaf>`.
    fn control_path(slug: &str, control: &str, leaf: &str) -> StatePath {
        StatePath::new(
            STATE_ROOT,
            vec![key(slug), key("controls"), key(control), key(leaf)],
        )
    }

    /// A fresh attach's input state (surfaces spec §4.1): every control
    /// released, untouched and at 0 detents, with no `value` until the
    /// device sends one.
    fn input_reset(book: &Book, surface: &str) -> Vec<(StatePath, Option<Value>)> {
        let (Some(entity), Some(attached)) =
            (book.surfaces.get(surface), book.attached.get(surface))
        else {
            return Vec::new();
        };
        let slug = &entity.slug;
        let mut ops = vec![(
            StatePath::new(STATE_ROOT, vec![key(slug), key("controls")]),
            None,
        )];
        for control in &attached.attach.controls {
            for input in &control.inputs {
                let (leaf, value): (&str, Value) = match input {
                    InputCap::Momentary { .. } => ("pressed", false.into()),
                    InputCap::Relative => ("detents", 0.into()),
                    InputCap::Absolute { .. } => continue,
                    InputCap::Touch => ("touched", false.into()),
                };
                ops.push((Self::control_path(slug, &control.id, leaf), Some(value)));
            }
        }
        ops
    }

    /// `connected`, `current.main` and, while attached, `model`.
    fn surface_state(&self, book: &Book, surface: &str) -> Vec<(StatePath, Option<Value>)> {
        let Some(entity) = book.surfaces.get(surface) else {
            return Vec::new();
        };
        let attached = book.attached.get(surface);
        let mut ops = vec![(
            Self::path(&entity.slug, "connected"),
            Some(attached.is_some().into()),
        )];
        if let Some(attached) = attached {
            ops.push((
                Self::path(&entity.slug, "model"),
                Some(attached.attach.model.as_str().into()),
            ));
        }
        ops.push((
            Self::current_path(&entity.slug),
            Some(Self::current_value(book, surface)),
        ));
        ops
    }

    /// The slug of the layer a surface shows, or `null`.
    fn current_value(book: &Book, surface: &str) -> Value {
        book.shown
            .get(surface)
            .and_then(|p| p.as_ref())
            .and_then(|p| book.layers.get(p))
            .map_or(Value::Nil, |p| p.slug.as_str().into())
    }

    fn publish_current(
        &self,
        book: &Book,
        surface: &str,
        origin: Origin,
    ) -> Result<Seq, JournalError> {
        let slug = &book.surfaces[surface].slug;
        let value = Self::current_value(book, surface);
        self.patch(origin, None, vec![(Self::current_path(slug), Some(value))])
    }

    /// `surface.<slug>.current.main`: the layer of the `main` navigation
    /// group (surfaces spec §5.3).
    fn current_path(slug: &str) -> StatePath {
        StatePath::new(STATE_ROOT, vec![key(slug), key("current"), key("main")])
    }

    /// Tells the binding worker what an attached surface shows, so it
    /// evaluates the layer and draws it.
    fn show_on(&self, book: &Book, surface: &str) {
        if let Some(attached) = book.attached.get(surface) {
            let layer = book.shown.get(surface).cloned().flatten();
            self.bindings.show(
                surface,
                layer,
                attached.attach.controls.clone(),
                attached.draws.clone(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(report: &Report) -> Vec<(serde_json::Value, String)> {
        report
            .errors
            .iter()
            .map(|p| {
                let json = serde_json::to_value(p).unwrap();
                (
                    json["path"].clone(),
                    json["code"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn control(id: &str, outputs: Vec<OutputCapKind>) -> LayerControl {
        LayerControl {
            id: id.into(),
            at: None,
            inputs: vec![InputCapKind::Momentary],
            outputs,
        }
    }

    fn layer(controls: Vec<LayerControl>) -> Layer {
        Layer {
            id: "l".into(),
            name: "Cams".into(),
            slug: "cams".into(),
            controls,
        }
    }

    #[test]
    fn a_layer_lists_each_control_once_and_keeps_what_is_bound() {
        let none = BTreeSet::new();
        let ok = layer(vec![
            control("k0", vec![OutputCapKind::Image]),
            control("pad.36", vec![]),
        ]);
        let image: BTreeSet<_> = [OutputCapKind::Image].into();
        let bound = BTreeMap::from([("k0".to_owned(), image.clone())]);
        assert!(validate_layer(&ok, &none, &bound).errors.is_empty());

        let twice = layer(vec![control("k0", vec![]), control("k0", vec![])]);
        assert_eq!(
            codes(&validate_layer(&twice, &none, &BTreeMap::new())),
            [(
                serde_json::json!(["controls", 1, "id"]),
                "duplicateControl".into()
            )]
        );

        let bound = BTreeMap::from([("k0".to_owned(), image.clone()), ("k9".to_owned(), image)]);
        let lost = layer(vec![control("k0", vec![])]);
        assert_eq!(
            codes(&validate_layer(&lost, &none, &bound)),
            [
                (serde_json::json!(["controls"]), "noOutput".into()),
                (serde_json::json!(["controls"]), "unknownControl".into()),
            ]
        );

        let many = layer(
            (0..257)
                .map(|i| control(&format!("k{i}"), vec![]))
                .collect(),
        );
        assert_eq!(
            codes(&validate_layer(&many, &none, &BTreeMap::new())),
            [(serde_json::json!(["controls"]), "invalid".into())]
        );
    }
}
