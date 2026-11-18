//! Surfaces and layers (surfaces spec, surfaces spec §11.1).
//!
//! [`Surfaces`] holds the `surface` and `layer` entities and the surfaces
//! attached right now. It doesn't know how a surface is connected: a
//! transport (the web surface's WebSocket, or the plugin host) attaches one
//! with its controls, feeds it [`Input`] and sends on the [`Output`]s it gets
//! back: images and levels.
//!
//! What a surface shows is drawn by the binding worker
//! ([`crate::binding::worker`]), which evaluates the bindings of the layer it
//! shows.
//!
//! Each attached surface has a task that applies its input to the controls'
//! input state, `surface.<slug>.controls.<id>.*`, as `state.patch`es with the
//! surface as origin (surfaces spec §4.1). Each change gives its edge events
//! ([`gesture`]), journaled as `input.event` with `cause` → the patch. Flows
//! match `control` triggers on those entries. Key, touch and encoder changes
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
    ControlEvent, EventRecord, InputKind as SentKind, JournalError, JournalOp, NewEntry, Origin,
    Record, Seq, StatePatchRecord,
};
use crate::state::{Meta, Quality, Source, StatePath};
use crate::{Shared, clock};

pub mod gesture;
pub mod render;
pub mod resolve;

use crate::binding::ControlBinding;
use crate::binding::worker::{Bindings, Problems};
use crate::flow::{Code, Problem, Report, is_identifier};
use crate::select::{Entities, Selector};
use gesture::{Change, Detected, Recogniser, Timing};

/// Entity types in the store.
pub const SURFACE_ENTITY: &str = "surface";
pub const LAYER_ENTITY: &str = "layer";
/// The state root for surfaces: `{conn: "surface", path: [slug, ...]}`.
pub const STATE_ROOT: &str = "surface";
/// How many outputs wait for a slow transport before more are dropped.
const OUTPUT_BACKLOG: usize = 256;
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
    /// The connection whose device this is, for a surface a plugin provides
    /// (plugin protocol §8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// A virtual surface's encoders and faders, beside its 15 keys (surfaces spec §11.4).
    #[serde(default, rename = "virtual", skip_serializing_if = "Option::is_none")]
    pub virtual_layout: Option<VirtualLayout>,
}

/// The model of the browser's virtual deck.
pub const VIRTUAL_DECK: &str = "virtual.deck15";
/// The most encoders, or faders, a virtual surface can have.
pub const MAX_VIRTUAL: u8 = 4;
/// The virtual deck's key images, in pixels a side.
const VIRTUAL_KEY: u32 = 144;

/// What a virtual surface has besides its keys (surfaces spec §11.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VirtualLayout {
    /// `e0`…`e3`, a row beneath the keys: `relative` + `momentary`, with a
    /// `level` output for the ring.
    #[serde(default)]
    pub encoders: u8,
    /// `f0`…`f3`, to the right of the keys, three cells tall: `absolute` +
    /// `touch`, with a `level` output.
    #[serde(default)]
    pub faders: u8,
    /// Whether the faders' `level` moves them.
    #[serde(default = "yes")]
    pub motor: bool,
}

fn yes() -> bool {
    true
}

impl Default for VirtualLayout {
    fn default() -> Self {
        VirtualLayout {
            encoders: 0,
            faders: 0,
            motor: true,
        }
    }
}

/// The controls the browser's virtual deck attaches with (surfaces spec §8,
/// surfaces spec §11.4): keys `k0`…`k14` in 5 × 3, then the layout's encoders and
/// faders. The UI builds the same list (`packages/ui/src/surfaces/model.ts`).
pub fn virtual_controls(layout: Option<&VirtualLayout>) -> Vec<ControlDef> {
    let layout = layout.copied().unwrap_or_default();
    let mut controls: Vec<ControlDef> = (0..15)
        .map(|i| ControlDef {
            id: format!("k{i}"),
            label: None,
            at: Placement::at(i % 5, i / 5),
            inputs: vec![InputCap::Momentary { velocity: false }],
            outputs: vec![OutputCap::Image(ImageDef {
                w: VIRTUAL_KEY,
                h: VIRTUAL_KEY,
                format: "png".into(),
                rotation: 0,
            })],
        })
        .collect();
    for i in 0..u32::from(layout.encoders.min(MAX_VIRTUAL)) {
        controls.push(ControlDef {
            id: format!("e{i}"),
            label: None,
            at: Placement::at(i, 3),
            inputs: vec![InputCap::Relative, InputCap::Momentary { velocity: false }],
            outputs: vec![OutputCap::Level { motor: false }],
        });
    }
    for i in 0..u32::from(layout.faders.min(MAX_VIRTUAL)) {
        controls.push(ControlDef {
            id: format!("f{i}"),
            label: None,
            at: Placement {
                x: 5 + i,
                y: 0,
                w: None,
                h: Some(3),
            },
            inputs: vec![InputCap::Absolute { axes: None }, InputCap::Touch],
            outputs: vec![OutputCap::Level {
                motor: layout.motor,
            }],
        });
    }
    controls
}

/// A set of control bindings on the controls it covers (surfaces spec §1).
/// `on`, `when`, `level` and `rank` decide where and when it controls them
/// (surfaces spec §5.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layer {
    #[serde(default)]
    pub id: LayerId,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    pub controls: Vec<LayerControl>,
    /// The surfaces it may appear on; every surface when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Selector<Entities>>,
    /// When it's active; `{nav: "main"}` when absent (surfaces spec §5.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
    #[serde(default, skip_serializing_if = "Level::is_normal")]
    pub level: Level,
    /// Its order within its level, 0 to 99; 0 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<crate::source::Source<f64>>,
}

/// When a layer is active (surfaces spec §5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum When {
    /// While it's the current layer of a navigation group.
    Nav(String),
    Always(True),
    /// While the source is truthy.
    While(crate::source::Source<serde_json::Value>),
}

impl Default for When {
    fn default() -> Self {
        When::Nav(MAIN_GROUP.to_owned())
    }
}

/// The navigation group every surface has, and layers are in by default.
pub const MAIN_GROUP: &str = "main";

/// A layer's band (surfaces spec §5.3): `rank` orders layers within it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Under,
    #[default]
    Normal,
    Over,
}

impl Level {
    fn is_normal(&self) -> bool {
        *self == Level::Normal
    }

    /// The bottom of its band of ranks: 0, 100 or 200 (surfaces spec §5.3).
    pub fn base(self) -> f64 {
        match self {
            Level::Under => 0.0,
            Level::Normal => 100.0,
            Level::Over => 200.0,
        }
    }
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

/// The input a control needs to give an event (surfaces spec §4.2).
pub fn input_for(event: ControlEvent) -> InputCapKind {
    match event {
        ControlEvent::Press
        | ControlEvent::Release
        | ControlEvent::Double
        | ControlEvent::Long
        | ControlEvent::Hold => InputCapKind::Momentary,
        ControlEvent::Turn => InputCapKind::Relative,
        ControlEvent::Change => InputCapKind::Absolute,
        ControlEvent::Touch | ControlEvent::Untouch => InputCapKind::Touch,
    }
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
    /// The host says the device moved itself: a motor fader following what
    /// it was sent (surfaces spec §11.4). It updates input state and gives no events.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub echo: bool,
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

/// What the core sets on a control (surfaces spec §11.4): `surface.draw` for an image,
/// `surface.level` for a level. One channel keeps them in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Image(Draw),
    Level { control: String, value: f64 },
}

impl Output {
    pub fn control(&self) -> &str {
        match self {
            Output::Image(draw) => &draw.control,
            Output::Level { control, .. } => control,
        }
    }
}

/// An attached surface's half of the connection to the core.
pub struct Link {
    /// Input from the device. Dropping it detaches the surface.
    pub input: mpsc::Sender<Input>,
    /// What to show. Closes when the surface is detached by the core:
    /// replaced by a newer attach, or deleted.
    pub outputs: mpsc::Receiver<Output>,
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
    #[error("layer {layer} doesn't fit: the surface lacks {}", missing.join(", "))]
    DoesNotFit {
        layer: LayerId,
        missing: Vec<String>,
    },
    #[error("{0:?} isn't a navigation group's name: use letters, digits and _")]
    BadGroup(String),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

pub struct Surfaces {
    inner: Arc<Inner>,
}

/// Told the layer a surface shows whenever that changes, so it can be kept
/// across restarts: the surface's ID, and its layer's or `None`. That's the
/// `main` navigation group's current layer.
pub type OnShown = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;

/// Told when a layer becomes active on a surface (surfaces spec §5.2), so
/// its surface-scoped helpers can be added there (surfaces spec §11.4): the
/// surface's ID and the layer's.
pub type OnActive = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct Inner {
    shared: Shared,
    timing: Timing,
    book: Mutex<Book>,
    bindings: Bindings,
    /// Which layer controls each control (surfaces spec §5.2).
    resolver: Arc<resolve::Resolver>,
    sessions: AtomicU64,
    on_shown: Mutex<Option<OnShown>>,
    on_active: Mutex<Option<OnActive>>,
    /// Each control's `gestures`, by `(layer, control)`, with its binding.
    gestures: Mutex<BTreeMap<(LayerId, String), (String, crate::binding::Gestures)>>,
    /// The connection aliases gesture settings' expressions read.
    aliases: Mutex<Arc<BTreeMap<String, crate::state::ConnId>>>,
    /// Evaluates gesture settings' expressions, with a binding's limit.
    evaluator: crate::expr::QuickJs,
}

#[derive(Default)]
struct Book {
    surfaces: BTreeMap<SurfaceId, Surface>,
    layers: BTreeMap<LayerId, Layer>,
    attached: BTreeMap<SurfaceId, Attached>,
    /// The current layer of each surface's `main` navigation group, kept
    /// while it's detached.
    shown: BTreeMap<SurfaceId, Option<LayerId>>,
    /// The current layer of its other navigation groups, by group.
    groups: BTreeMap<SurfaceId, BTreeMap<String, Option<LayerId>>>,
    /// The controls each surface last attached with.
    controls: BTreeMap<SurfaceId, Vec<ControlDef>>,
}

struct Attached {
    /// Tells a replaced attach's task apart from the current one.
    session: u64,
    attach: Attach,
    outputs: mpsc::Sender<Output>,
    /// Input applied as if the device sent it (surfaces spec §4.5).
    inject: mpsc::Sender<Injection>,
    /// Dropped to stop the task when the attach is replaced or deleted.
    _stop: oneshot::Sender<()>,
}

/// One injected input: whose it is, and where to say how it went.
struct Injection {
    input: Input,
    origin: Origin,
    cause: Option<Seq>,
    done: oneshot::Sender<Result<(), InjectError>>,
}

/// Why an injected input wasn't applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InjectError {
    #[error("no surface {0}")]
    UnknownSurface(String),
    #[error("surface {0} isn't attached")]
    NotAttached(String),
    #[error("the surface has no control {0:?}")]
    UnknownControl(String),
    #[error("{0}")]
    Unsupported(String),
}

impl Surfaces {
    /// Surfaces on `shared`'s state, whose layer resolution runs in its
    /// turn ([`crate::state::LiveState::set_resolver`]).
    pub fn new(shared: Shared, timing: Timing) -> Self {
        let resolver = Arc::new(resolve::Resolver::new(
            &shared.state,
            shared.journal.clone(),
        ));
        shared
            .state
            .set_resolver(Some(resolver.clone() as Arc<dyn crate::state::Settler>));
        Surfaces {
            inner: Arc::new(Inner {
                bindings: Bindings::start(shared.clone(), resolver.clone()),
                resolver,
                shared,
                timing,
                book: Mutex::new(Book::default()),
                sessions: AtomicU64::new(0),
                on_shown: Mutex::new(None),
                on_active: Mutex::new(None),
                gestures: Mutex::new(BTreeMap::new()),
                aliases: Mutex::new(Arc::new(BTreeMap::new())),
                evaluator: crate::expr::QuickJs::new(crate::expr::Limits {
                    time: crate::binding::worker::EXPR_TIME,
                    ..crate::expr::Limits::default()
                }),
            }),
        }
    }

    /// Calls `f` whenever the layer a surface shows changes, including to
    /// nothing. It's called with the surfaces locked, so it mustn't call
    /// back into them.
    pub fn on_shown(&self, f: OnShown) {
        *self
            .inner
            .on_shown
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Calls `f` whenever a layer becomes active on a surface. It's called
    /// without the surfaces locked, before the surface's next input.
    pub fn on_active(&self, f: OnActive) {
        *lock(&self.inner.on_active) = Some(f);
    }

    /// Which layers are active on an attached surface and which controls
    /// each (surfaces spec §5.2), as of the last turn.
    pub fn resolution(&self, surface: &str) -> Option<Arc<resolve::Resolution>> {
        self.inner.resolver.resolution(surface)
    }

    /// Each failing layer field's last error, by layer ID and field
    /// (`on`, `when` or `rank`).
    pub fn layer_problems(&self) -> BTreeMap<(LayerId, &'static str), String> {
        self.inner.resolver.problems()
    }

    /// Sets the layer a surface last showed, as kept from before a restart,
    /// without telling [`Surfaces::on_shown`]. It shows it when it attaches,
    /// if it still fits, in place of its start layer.
    pub fn restore_shown(&self, surface: &str, layer: Option<LayerId>) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        if !book.surfaces.contains_key(surface) || book.attached.contains_key(surface) {
            return Ok(());
        }
        book.shown.insert(surface.to_owned(), layer);
        inner.publish_current(&book, surface, MAIN_GROUP, Origin::system())?;
        Ok(())
    }

    /// Adds or replaces a surface entity. A new slug moves its state.
    pub fn set_surface(&self, surface: Surface) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        let old = book.surfaces.insert(surface.id.clone(), surface.clone());
        inner
            .resolver
            .set_universe(book.surfaces.keys().cloned().collect());
        match old {
            Some(old) if old.slug != surface.slug => {
                if let Some(attached) = book.attached.get(&surface.id) {
                    inner.resolver.attach(
                        &surface.id,
                        &surface.slug,
                        attached.attach.controls.clone(),
                    );
                }
                let mut ops = vec![(StatePath::new(STATE_ROOT, vec![key(&old.slug)]), None)];
                ops.extend(inner.surface_state(&book, &surface.id));
                inner.patch(Origin::system(), None, ops)?;
                // Its levels are written again under the new slug.
                inner.show_on(&book, &surface.id);
            }
            _ => {
                inner.shared.state.settle();
            }
        }
        drop(book);
        inner.activated();
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
        book.groups.remove(id);
        book.controls.remove(id);
        inner.bindings.hide(id);
        inner.resolver.remove(id);
        inner
            .resolver
            .set_universe(book.surfaces.keys().cloned().collect());
        let path = StatePath::new(STATE_ROOT, vec![key(&surface.slug)]);
        inner.patch(Origin::system(), None, vec![(path, None)])?;
        Ok(())
    }

    /// Adds or replaces a layer. A navigation group whose current layer no
    /// longer fits its surface is left with none, and every surface's
    /// layers are resolved again.
    pub fn set_layer(&self, layer: Layer) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        book.layers.insert(layer.id.clone(), layer.clone());
        inner.resolver.set_layers(Arc::new(book.layers.clone()));
        for (surface, group) in Inner::navigated_to(&book, &layer.id) {
            let fits = book
                .attached
                .get(&surface)
                .is_none_or(|a| missing(&layer, &a.attach.controls).is_empty());
            if !fits {
                inner.set_current(&mut book, &surface, &group, None);
            }
            // Its slug may have changed.
            inner.publish_current(&book, &surface, &group, Origin::system())?;
        }
        inner.shared.state.settle();
        drop(book);
        inner.activated();
        Ok(())
    }

    /// Removes a layer, leaving the navigation groups it was current in
    /// with none.
    pub fn remove_layer(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        book.layers.remove(id);
        inner.resolver.set_layers(Arc::new(book.layers.clone()));
        for (surface, group) in Inner::navigated_to(&book, id) {
            inner.set_current(&mut book, &surface, &group, None);
            inner.publish_current(&book, &surface, &group, Origin::system())?;
        }
        inner.shared.state.settle();
        drop(book);
        inner.activated();
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

    /// The current layer of a surface's `main` navigation group, also while
    /// it's detached.
    pub fn shown(&self, surface: &str) -> Option<LayerId> {
        self.inner.book().shown.get(surface).cloned().flatten()
    }

    /// Adds or replaces a control binding.
    pub fn set_binding(&self, binding: ControlBinding) {
        {
            let mut gestures = lock(&self.inner.gestures);
            gestures.retain(|_, (id, _)| *id != binding.id);
            if let Some(g) = &binding.gestures {
                gestures.insert(
                    (binding.layer.clone(), binding.control.clone()),
                    (binding.id.clone(), g.clone()),
                );
            }
        }
        self.inner.bindings.set(binding);
    }

    pub fn remove_binding(&self, id: &str) {
        lock(&self.inner.gestures).retain(|_, (b, _)| b != id);
        self.inner.bindings.remove(id);
    }

    /// The connection aliases `expr` bindings read, by alias.
    pub fn set_aliases(&self, aliases: BTreeMap<String, crate::state::ConnId>) {
        *lock(&self.inner.aliases) = Arc::new(aliases.clone());
        self.inner.resolver.set_aliases(Arc::new(aliases.clone()));
        self.inner.bindings.set_aliases(aliases);
        self.inner.shared.state.settle();
        self.inner.activated();
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

    /// Applies an input as if the surface had sent it (surfaces spec §4.5),
    /// in order with its own input, journaled with `origin` and `cause`.
    /// Answers once it's applied and its events are journaled.
    pub async fn inject(
        &self,
        surface: &str,
        control: &str,
        kind: SentKind,
        value: Option<InputValue>,
        origin: Origin,
        cause: Option<Seq>,
    ) -> Result<(), InjectError> {
        let sender = {
            let book = self.inner.book();
            if !book.surfaces.contains_key(surface) {
                return Err(InjectError::UnknownSurface(surface.to_owned()));
            }
            book.attached
                .get(surface)
                .map(|a| a.inject.clone())
                .ok_or_else(|| InjectError::NotAttached(surface.to_owned()))?
        };
        let (done, answer) = oneshot::channel();
        let injection = Injection {
            input: Input {
                control: control.to_owned(),
                kind,
                value,
                ts: crate::clock::epoch_ms() as f64,
                echo: false,
            },
            origin,
            cause,
            done,
        };
        let gone = || InjectError::NotAttached(surface.to_owned());
        sender.send(injection).await.map_err(|_| gone())?;
        answer.await.map_err(|_| gone())?
    }

    pub fn is_attached(&self, surface: &str) -> bool {
        self.inner.book().attached.contains_key(surface)
    }

    /// Navigates a surface's `main` group to a layer, or to nothing.
    pub fn show(
        &self,
        surface: &str,
        layer: Option<&str>,
        origin: Origin,
    ) -> Result<(), ShowError> {
        self.navigate(surface, MAIN_GROUP, layer, origin)
    }

    /// Navigates one of a surface's navigation groups to a layer, or to
    /// nothing (surfaces spec §5.3): `surface.<slug>.current.<group>`. The
    /// layer must fit an attached surface (§5.4).
    pub fn navigate(
        &self,
        surface: &str,
        group: &str,
        layer: Option<&str>,
        origin: Origin,
    ) -> Result<(), ShowError> {
        let inner = &self.inner;
        if !is_identifier(group) {
            return Err(ShowError::BadGroup(group.to_owned()));
        }
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
        inner.set_current(&mut book, surface, group, layer.map(str::to_owned));
        inner.publish_current(&book, surface, group, origin)?;
        drop(book);
        inner.activated();
        Ok(())
    }

    /// Attaches a surface. A surface already attached is replaced: its
    /// [`Link::outputs`] closes. Must be called inside a tokio runtime.
    pub fn attach(&self, surface: &str, attach: Attach) -> Result<Link, AttachError> {
        check_attach(&attach)?;
        let inner = &self.inner;
        let session = inner.sessions.fetch_add(1, Ordering::Relaxed);
        let (input_tx, input_rx) = mpsc::channel(INPUT_BACKLOG);
        let (outputs_tx, outputs_rx) = mpsc::channel(OUTPUT_BACKLOG);
        let (stop_tx, stop_rx) = oneshot::channel();
        let (inject_tx, inject_rx) = mpsc::channel(INPUT_BACKLOG);

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
        inner.set_shown(&mut book, surface, layer);
        // Other groups keep their layer if it still fits.
        let fits = |l: &Option<LayerId>| {
            l.as_ref().is_none_or(|l| {
                book.layers
                    .get(l)
                    .is_some_and(|l| missing(l, &attach.controls).is_empty())
            })
        };
        let unfit: Vec<String> = book
            .groups
            .get(surface)
            .into_iter()
            .flatten()
            .filter(|(_, l)| !fits(l))
            .map(|(g, _)| g.clone())
            .collect();
        for group in unfit {
            inner.set_current(&mut book, surface, &group, None);
        }
        book.controls
            .insert(surface.to_owned(), attach.controls.clone());
        let slug = book.surfaces[surface].slug.clone();
        inner
            .resolver
            .attach(surface, &slug, attach.controls.clone());
        book.attached.insert(
            surface.to_owned(),
            Attached {
                session,
                attach,
                outputs: outputs_tx,
                inject: inject_tx,
                _stop: stop_tx,
            },
        );
        let mut ops = inner.surface_state(&book, surface);
        ops.extend(inner.input_reset(&book, surface));
        inner.patch(surface_origin(surface), None, ops)?;
        inner.show_on(&book, surface);
        drop(book);
        inner.activated();

        tokio::spawn(run(
            inner.clone(),
            surface.to_owned(),
            session,
            input_rx,
            inject_rx,
            stop_rx,
        ));
        Ok(Link {
            input: input_tx,
            outputs: outputs_rx,
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
            if let OutputCap::Image(image) = output {
                if !((1..=MAX_IMAGE).contains(&image.w) && (1..=MAX_IMAGE).contains(&image.h)) {
                    return invalid(format!(
                        "control {id:?} has a {}×{} image: use 1 to {MAX_IMAGE} pixels a side",
                        image.w, image.h
                    ));
                }
                if let Err(e) = render::check_output(&image.format, image.rotation) {
                    return invalid(format!("control {id:?}: {e}"));
                }
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
    if let Some(v) = &surface.virtual_layout {
        if surface.model != VIRTUAL_DECK {
            errors.push(Problem::new(
                vec![key("virtual")],
                Code::Invalid,
                format!("only a {VIRTUAL_DECK} surface has a virtual layout"),
            ));
        }
        for (name, n) in [("encoders", v.encoders), ("faders", v.faders)] {
            if n > MAX_VIRTUAL {
                errors.push(Problem::new(
                    vec![key("virtual"), key(name)],
                    Code::Invalid,
                    format!("a virtual surface can have at most {MAX_VIRTUAL} {name}"),
                ));
            }
        }
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

/// What a layer is checked against.
pub struct LayerContext<'a> {
    /// Slugs other layers have.
    pub taken_slugs: &'a BTreeSet<String>,
    /// The outputs the layer's bindings drive, by control.
    pub bound: &'a BTreeMap<String, BTreeSet<OutputCapKind>>,
    /// Every surface's ID, for `on`.
    pub surfaces: &'a BTreeSet<String>,
    /// Compiles expressions in its fields.
    pub evaluator: &'a dyn crate::expr::Evaluator,
}

/// Checks a layer (surfaces spec §9): a name, a slug no other layer has, at
/// most 256 controls with unique IDs, and every control bound on it still
/// listed with the outputs its bindings drive. Its `on`, `when` and `rank`
/// must name surfaces that exist and compile, and can't read resolution's
/// results (`resolutionCycle`, surfaces spec §5.2).
pub fn validate_layer(layer: &Layer, ctx: &LayerContext<'_>) -> Report {
    let bound = ctx.bound;
    let mut errors = named("layer", &layer.name, &layer.slug, ctx.taken_slugs);
    if let Some(on) = &layer.on {
        for id in on.literal_ids() {
            if !ctx.surfaces.contains(id) {
                errors.push(Problem::new(
                    vec![key("on")],
                    Code::UnknownSurface,
                    format!("no surface {id:?}"),
                ));
            }
        }
        if let Selector::Expr(e) = on {
            field_expr(e, vec![key("on"), key("expr")], ctx, &mut errors);
        }
    }
    match &layer.when {
        Some(When::Nav(group)) if !is_identifier(group) => errors.push(Problem::new(
            vec![key("when"), key("nav")],
            Code::Invalid,
            format!("{group:?} isn't a navigation group's name: use letters, digits and _"),
        )),
        Some(When::While(source)) => {
            field_source(source, vec![key("when"), key("while")], ctx, &mut errors)
        }
        _ => {}
    }
    if let Some(rank) = &layer.rank {
        field_source(rank, vec![key("rank")], ctx, &mut errors);
    }
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

/// A layer field's source: its expressions compile, and nothing in it
/// reads resolution's results.
fn field_source<T>(
    source: &crate::source::Source<T>,
    base: Vec<PathSegment>,
    ctx: &LayerContext<'_>,
    errors: &mut Vec<Problem>,
) {
    use crate::source::{Part, Ref, Root};
    let mut here = base;
    source.walk(&mut here, &mut |at, part| match part {
        Part::Expr(e) => field_expr(e, at.clone(), ctx, errors),
        Part::Ref(Ref::Root {
            root: Root::Surface,
            path,
        }) if reads_resolution(path.iter().skip(1).map(|s| match s {
            PathSegment::Key(k) => k.as_str(),
            PathSegment::Index(_) => "*",
        })) =>
        {
            errors.push(resolution_cycle(at.clone()))
        }
        Part::Ref(_) => {}
        Part::Transform(t) => {
            for (rel, message) in t.problems() {
                let mut path = at.clone();
                path.extend(rel);
                errors.push(Problem::new(path, Code::BadValue, message));
            }
        }
    });
}

fn field_expr(e: &str, at: Vec<PathSegment>, ctx: &LayerContext<'_>, errors: &mut Vec<Problem>) {
    if let Err(problem) = ctx.evaluator.check(e) {
        errors.push(Problem::new(at.clone(), Code::ExprSyntax, problem.message));
    }
    if expr_reads_resolution(e) {
        errors.push(resolution_cycle(at));
    }
}

fn resolution_cycle(at: Vec<PathSegment>) -> Problem {
    Problem::new(
        at,
        Code::ResolutionCycle,
        "a layer's fields can't read which layers are active or control a control",
    )
}

/// Whether the rest of a surface's path, after its slug, is a resolution
/// result: `layers`, or `controls.<id>.layer`.
fn reads_resolution<'a>(mut rest: impl Iterator<Item = &'a str>) -> bool {
    match rest.next() {
        Some("layers") => true,
        Some("controls") => rest.nth(1) == Some("layer"),
        _ => false,
    }
}

/// Whether an expression names a resolution result, as far as can be told
/// without running it: `self.layers`, `self.controls.<id>.layer`, and the
/// same under `surface.<slug>`. A computed path isn't caught.
pub(crate) fn expr_reads_resolution(e: &str) -> bool {
    dotted_chains(e).iter().any(|chain| {
        let parts = chain.iter().map(String::as_str);
        match chain.first().map(String::as_str) {
            Some("self") => reads_resolution(parts.skip(1)),
            Some("surface") => reads_resolution(parts.skip(2)),
            _ => false,
        }
    })
}

/// The dotted names in an expression, `a.b[c].d` as `[a, b, *, d]`.
/// Strings aren't told apart from code, which only errs towards finding more.
fn dotted_chains(e: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = e.chars().collect();
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut chains = Vec::new();
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
            chain.push(chars[start..i].iter().collect::<String>());
            // `.`, `?.`, or `[…]` (one segment), then the next name.
            loop {
                if i < chars.len() && chars[i] == '[' {
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
                    chain.push("*".into());
                    continue;
                }
                break;
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
        chains.push(chain);
    }
    chains
}

/// The name and slug problems of a surface or layer.
pub(crate) fn named(kind: &str, name: &str, slug: &str, taken: &BTreeSet<String>) -> Vec<Problem> {
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

/// `surface.<slug>.controls.<id>.requested.level`: what the core last sent
/// a `level` output (surfaces spec §4.4).
pub(crate) fn requested_level_path(slug: &str, control: &str) -> StatePath {
    StatePath::new(
        STATE_ROOT,
        vec![
            key(slug),
            key("controls"),
            key(control),
            key("requested"),
            key("level"),
        ],
    )
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
    mut injected: mpsc::Receiver<Injection>,
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
        acting: None,
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
            Some(injection) = injected.recv() => {
                task.acting = Some((injection.origin, injection.cause));
                let result = task.input(injection.input).await;
                task.acting = None;
                let _ = injection.done.send(result);
            }
            next = input.recv() => match next {
                Some(i) => {
                    let _ = task.input(i).await;
                }
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
    /// While an injected input is applied: its origin and cause.
    acting: Option<(Origin, Option<Seq>)>,
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
    async fn input(&mut self, input: Input) -> Result<(), InjectError> {
        // Cells of layers that became active since are added first.
        self.inner.activated();
        let control = {
            let book = self.inner.book();
            let Some(attached) = book
                .attached
                .get(&self.surface)
                .filter(|a| a.session == self.session)
            else {
                return Err(InjectError::NotAttached(self.surface.clone()));
            };
            match attached
                .attach
                .controls
                .iter()
                .find(|c| c.id == input.control)
            {
                Some(control) => control.clone(),
                None => {
                    if self.acting.is_none() {
                        self.drops.drop(&self.surface, &input, "no such control");
                    }
                    return Err(InjectError::UnknownControl(input.control.clone()));
                }
            }
        };
        let writes = match self.writes(&control, &input) {
            Ok(writes) => writes,
            Err(why) => {
                if self.acting.is_none() {
                    self.drops.drop(&self.surface, &input, &why);
                }
                return Err(InjectError::Unsupported(why));
            }
        };
        for write in writes {
            let Some(turn) = self.write(&input, write) else {
                return Err(InjectError::NotAttached(self.surface.clone()));
            };
            if let Some(turn) = turn
                && !self.inner.shared.state.seen(turn, TURN_LIMIT).await
            {
                tracing::debug!(surface = %self.surface, "input went ahead of a slow state consumer");
            }
        }
        Ok(())
    }

    /// Whose input this is: the device's, or an injection's.
    fn origin_and_cause(&self) -> (Origin, Option<Seq>) {
        self.acting
            .clone()
            .unwrap_or_else(|| (surface_origin(&self.surface), None))
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
        let (origin, cause) = self.origin_and_cause();
        let slug = {
            let book = self.inner.book();
            book.attached
                .get(&self.surface)
                .filter(|a| a.session == self.session)?;
            book.surfaces[&self.surface].slug.clone()
        };
        // The layer controlling the control as the input arrives, before
        // the input itself can change that (surfaces spec §5.2).
        let layer = self
            .inner
            .resolver
            .controlling(&self.surface, &input.control);
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
        let (seq, turn) = match self
            .inner
            .patch_turn(origin.clone(), cause, ops, Some(input.ts))
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
        // A press reads its control's timing, and keeps it (surfaces spec §11.4).
        let timing = match write.change {
            Change::Pressed(true) => self.inner.press_timing(layer.as_deref(), &input.control),
            _ => self.inner.timing,
        };
        let found = self.recogniser.change_timed(
            &input.control,
            write.change,
            input.ts,
            Instant::now(),
            timing,
        );
        // An echo is state only: nothing reacts to a device moving itself.
        if !input.echo {
            self.journal_events(layer.as_deref(), found, Some(seq));
        }
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
            // Addressed at the control (dataflow §3); a surface removed
            // since keeps its ID in place of its slug.
            let slug = self
                .inner
                .book()
                .surfaces
                .get(&self.surface)
                .map_or_else(|| self.surface.clone(), |s| s.slug.clone());
            let payload = match (delta, value) {
                (Some(d), _) => Some(Value::Map(vec![("delta".into(), d.into())])),
                (None, Some(v)) => Some(Value::Map(vec![("value".into(), v)])),
                (None, None) => None,
            };
            let entry = NewEntry::new(
                self.origin_and_cause().0,
                Record::Event(EventRecord {
                    surface: Some(self.surface.clone()),
                    layer: layer.map(str::to_owned),
                    control: Some(control.clone()),
                    ..EventRecord::at(
                        StatePath::new(
                            STATE_ROOT,
                            vec![key(&slug), key("controls"), key(&control)],
                        ),
                        event.as_str(),
                        payload,
                    )
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
        // Releases carry their presses' layers.
        let released = self.recogniser.release_all();
        self.journal_events(None, released, Some(cause));
        inner.resolver.detach(&self.surface);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn book(&self) -> MutexGuard<'_, Book> {
        lock(&self.book)
    }

    /// A control's gesture timing at a press (surfaces spec §11.4): from the binding on
    /// the layer controlling it, each setting read now, or the default. A
    /// setting that fails is reported as the binding's problem.
    fn press_timing(&self, layer: Option<&str>, control: &str) -> Timing {
        let mut timing = self.timing;
        let Some(layer) = layer else { return timing };
        let Some((binding, gestures)) = lock(&self.gestures)
            .get(&(layer.to_owned(), control.to_owned()))
            .cloned()
        else {
            return timing;
        };
        let aliases = lock(&self.aliases).clone();
        let scope = crate::expr::Scope::new(&self.shared.state, &aliases);
        for (name, setting) in gestures.settings() {
            let Some(source) = setting else { continue };
            let value = match source.evaluate(&scope, &self.evaluator).value {
                Ok(Some(v)) => crate::binding::gesture_duration(&v),
                Ok(None) => source
                    .fallback()
                    .map_or(Err("it has no value".to_owned()), |f| {
                        crate::binding::gesture_duration(f)
                    }),
                Err(e) => Err(e.to_string()),
            };
            let prop = format!("gestures.{name}");
            match value {
                Ok(d) => {
                    self.bindings.note(&binding, &prop, None);
                    match name {
                        "doubleWindow" => timing.double_window = d,
                        "longAfter" => timing.long_after = d,
                        _ => timing.hold_every = d,
                    }
                }
                Err(e) => self.bindings.note(&binding, &prop, Some(e)),
            }
        }
        timing
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
                        reducer: None,
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
    /// device sends one, and nothing requested. Helpers with `scope:
    /// "surface"` keep their values (surfaces spec §11.4).
    fn input_reset(&self, book: &Book, surface: &str) -> Vec<(StatePath, Option<Value>)> {
        let (Some(entity), Some(attached)) =
            (book.surfaces.get(surface), book.attached.get(surface))
        else {
            return Vec::new();
        };
        let slug = &entity.slug;
        let controls = StatePath::new(STATE_ROOT, vec![key(slug), key("controls")]);
        let mut ops: Vec<(StatePath, Option<Value>)> = self.shared.state.read(|tree| {
            tree.leaves_under(&controls)
                .map(|(path, _)| path)
                // Helpers and resolution keep theirs (surfaces spec §11.4, §5.2).
                .filter(|path| {
                    path.path.get(3) != Some(&key("helpers"))
                        && !(path.path.len() == 4 && path.path[3] == key("layer"))
                })
                .map(|path| (path.clone(), None))
                .collect()
        });
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
        let mut groups = vec![MAIN_GROUP.to_owned()];
        groups.extend(
            book.groups
                .get(surface)
                .into_iter()
                .flat_map(|g| g.keys().cloned()),
        );
        for group in groups {
            ops.push((
                Self::current_path(&entity.slug, &group),
                Some(Self::current_value(book, surface, &group)),
            ));
        }
        ops
    }

    /// A group's current layer.
    fn current(book: &Book, surface: &str, group: &str) -> Option<LayerId> {
        if group == MAIN_GROUP {
            book.shown.get(surface).cloned().flatten()
        } else {
            book.groups.get(surface)?.get(group).cloned().flatten()
        }
    }

    /// The slug of a group's current layer, or `null`.
    fn current_value(book: &Book, surface: &str, group: &str) -> Value {
        Self::current(book, surface, group)
            .and_then(|p| book.layers.get(&p))
            .map_or(Value::Nil, |p| p.slug.as_str().into())
    }

    /// Every `(surface, group)` whose current layer is `layer`.
    fn navigated_to(book: &Book, layer: &str) -> Vec<(SurfaceId, String)> {
        let main = book
            .shown
            .iter()
            .filter(|(_, l)| l.as_deref() == Some(layer))
            .map(|(s, _)| (s.clone(), MAIN_GROUP.to_owned()));
        let others = book.groups.iter().flat_map(|(s, groups)| {
            groups
                .iter()
                .filter(|(_, l)| l.as_deref() == Some(layer))
                .map(|(g, _)| (s.clone(), g.clone()))
        });
        main.chain(others).collect()
    }

    /// Sets a group's current layer.
    fn set_current(&self, book: &mut Book, surface: &str, group: &str, layer: Option<LayerId>) {
        if group == MAIN_GROUP {
            self.set_shown(book, surface, layer);
        } else {
            book.groups
                .entry(surface.to_owned())
                .or_default()
                .insert(group.to_owned(), layer);
        }
    }

    /// Tells [`Surfaces::on_active`] which layers became active since it
    /// was last told.
    fn activated(&self) {
        let activated = self.resolver.take_activated();
        if activated.is_empty() {
            return;
        }
        let hook = lock(&self.on_active).clone();
        if let Some(hook) = hook {
            for (surface, layer) in activated {
                hook(&surface, &layer);
            }
        }
    }

    /// Records the layer a surface shows, telling [`Surfaces::on_shown`] if
    /// it changed.
    fn set_shown(&self, book: &mut Book, surface: &str, layer: Option<LayerId>) {
        let old = book.shown.insert(surface.to_owned(), layer.clone());
        if old.as_ref() == Some(&layer) {
            return;
        }
        let hook = self
            .on_shown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook(surface, layer.as_deref());
        }
    }

    fn publish_current(
        &self,
        book: &Book,
        surface: &str,
        group: &str,
        origin: Origin,
    ) -> Result<Seq, JournalError> {
        let slug = &book.surfaces[surface].slug;
        let value = Self::current_value(book, surface, group);
        self.patch(
            origin,
            None,
            vec![(Self::current_path(slug, group), Some(value))],
        )
    }

    /// `surface.<slug>.current.<group>`: a navigation group's current layer
    /// (surfaces spec §5.3).
    fn current_path(slug: &str, group: &str) -> StatePath {
        StatePath::new(STATE_ROOT, vec![key(slug), key("current"), key(group)])
    }

    /// Tells the binding worker about an attached surface, so it draws it
    /// from the layers resolution gives it.
    fn show_on(&self, book: &Book, surface: &str) {
        if let Some(attached) = book.attached.get(surface) {
            self.bindings.show(
                surface,
                &book.surfaces[surface].slug,
                attached.attach.controls.clone(),
                attached.outputs.clone(),
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
            ..Layer::default()
        }
    }

    static NO_SURFACES: BTreeSet<String> = BTreeSet::new();
    static QJS: std::sync::LazyLock<crate::expr::QuickJs> =
        std::sync::LazyLock::new(crate::expr::QuickJs::default);

    fn lctx<'a>(
        taken: &'a BTreeSet<String>,
        bound: &'a BTreeMap<String, BTreeSet<OutputCapKind>>,
    ) -> LayerContext<'a> {
        LayerContext {
            taken_slugs: taken,
            bound,
            surfaces: &NO_SURFACES,
            evaluator: &*QJS,
        }
    }

    #[test]
    fn a_layers_fields_name_surfaces_and_cannot_read_resolution() {
        let none = BTreeSet::new();
        let bound = BTreeMap::new();
        let surfaces: BTreeSet<String> = ["s1".to_owned()].into();
        let ctx = LayerContext {
            surfaces: &surfaces,
            ..lctx(&none, &bound)
        };
        let with = |json: serde_json::Value| {
            let mut l = serde_json::json!({ "name": "Shift", "slug": "shift", "controls": [] });
            l.as_object_mut()
                .unwrap()
                .extend(json.as_object().unwrap().clone());
            codes(&validate_layer(&Layer::parse(&l).unwrap(), &ctx))
        };
        assert!(
            with(serde_json::json!({
                "on": ["s1"], "level": "over", "rank": 3,
                "when": { "while": { "expr": "self.controls.k15.pressed" } }
            }))
            .is_empty()
        );
        assert_eq!(
            with(serde_json::json!({ "on": { "any": ["s1", "s2"] } })),
            [(serde_json::json!(["on"]), "unknownSurface".into())]
        );
        assert_eq!(
            with(serde_json::json!({ "when": { "while": { "expr": "self.layers.length > 1" } } })),
            [(
                serde_json::json!(["when", "while", "expr"]),
                "resolutionCycle".into()
            )]
        );
        assert_eq!(
            with(
                serde_json::json!({ "rank": { "expr": "surface.deck.controls['k3'].layer == 'cams' ? 1 : 0" } })
            ),
            [(
                serde_json::json!(["rank", "expr"]),
                "resolutionCycle".into()
            )]
        );
        assert_eq!(
            with(
                serde_json::json!({ "when": { "while": { "ref": { "root": "surface", "path": ["deck", "layers"] } } } })
            ),
            [(
                serde_json::json!(["when", "while", "ref"]),
                "resolutionCycle".into()
            )]
        );
        assert_eq!(
            with(serde_json::json!({ "when": { "nav": "pop over" } })),
            [(serde_json::json!(["when", "nav"]), "invalid".into())]
        );
        // Input state and a layer's own helpers are fine to read.
        assert!(
            with(serde_json::json!({
                "when": { "while": { "expr": "self.controls.k3.layerName || layer.cams.controls.k3.on" } }
            }))
            .is_empty()
        );
    }

    #[test]
    fn a_virtual_layout_is_for_the_virtual_deck_with_up_to_four_of_each() {
        let surface = |model: &str, json: serde_json::Value| Surface {
            id: "s".into(),
            name: "Deck".into(),
            slug: "deck".into(),
            model: model.into(),
            serial: None,
            start_layer: None,
            provider: None,
            virtual_layout: Some(serde_json::from_value(json).unwrap()),
        };
        let check = |s: &Surface| codes(&validate_surface(s, &BTreeSet::new(), &BTreeMap::new()));
        let ok = surface(
            VIRTUAL_DECK,
            serde_json::json!({ "encoders": 4, "faders": 2 }),
        );
        assert!(ok.virtual_layout.unwrap().motor, "motor defaults to true");
        assert!(check(&ok).is_empty());
        assert!(
            check_attach(&Attach {
                model: VIRTUAL_DECK.into(),
                serial: None,
                name: "Deck".into(),
                controls: virtual_controls(ok.virtual_layout.as_ref()),
            })
            .is_ok()
        );
        assert_eq!(
            check(&surface(VIRTUAL_DECK, serde_json::json!({ "faders": 5 }))),
            [(serde_json::json!(["virtual", "faders"]), "invalid".into())]
        );
        assert_eq!(
            check(&surface("elgato.mk2", serde_json::json!({}))),
            [(serde_json::json!(["virtual"]), "invalid".into())]
        );
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
        assert!(validate_layer(&ok, &lctx(&none, &bound)).errors.is_empty());

        let twice = layer(vec![control("k0", vec![]), control("k0", vec![])]);
        assert_eq!(
            codes(&validate_layer(&twice, &lctx(&none, &BTreeMap::new()))),
            [(
                serde_json::json!(["controls", 1, "id"]),
                "duplicateControl".into()
            )]
        );

        let bound = BTreeMap::from([("k0".to_owned(), image.clone()), ("k9".to_owned(), image)]);
        let lost = layer(vec![control("k0", vec![])]);
        assert_eq!(
            codes(&validate_layer(&lost, &lctx(&none, &bound))),
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
            codes(&validate_layer(&many, &lctx(&none, &BTreeMap::new()))),
            [(serde_json::json!(["controls"]), "invalid".into())]
        );
    }
}
