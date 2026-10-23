//! Surfaces and pages (surfaces spec, ADR 0022).
//!
//! [`Surfaces`] holds the `surface` and `page` entities and the surfaces
//! attached right now. It doesn't know how a surface is connected: a
//! transport (the web surface's WebSocket, later the plugin host) attaches
//! one with its controls, feeds it [`Input`] and sends on the [`Draw`]s it
//! gets back.
//!
//! What a surface shows is drawn by the binding worker
//! ([`crate::binding::worker`]), which evaluates the bindings of the page it
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
    ControlEvent, InputEventRecord, InputKind, JournalError, JournalOp, NewEntry, Origin, Record,
    Seq, StatePatchRecord,
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
pub const PAGE_ENTITY: &str = "page";
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
pub type PageId = String;

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
    pub start_page: Option<PageId>,
}

/// A layout of control bindings (surfaces spec §1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    #[serde(default)]
    pub id: PageId,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    pub layout: Layout,
}

/// A grid of buttons, named `k0`, `k1` … in row-major order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub cols: u32,
    pub rows: u32,
}

impl Layout {
    /// Whether `control` is a button in this layout: `k<n>`, with no
    /// leading zeros, `n` below `cols × rows`.
    pub fn contains(&self, control: &str) -> bool {
        let Some(n) = control.strip_prefix('k') else {
            return false;
        };
        let canonical = n == "0" || (!n.is_empty() && !n.starts_with('0'));
        canonical
            && n.bytes().all(|b| b.is_ascii_digit())
            && n.parse::<u64>()
                .is_ok_and(|n| n < u64::from(self.cols) * u64::from(self.rows))
    }

    /// Its buttons, `k0` to `k<cols × rows - 1>`.
    pub fn controls(&self) -> impl Iterator<Item = String> + use<> {
        (0..u64::from(self.cols) * u64::from(self.rows)).map(|n| format!("k{n}"))
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
    pub fn kind(&self) -> &'static str {
        match self {
            InputCap::Momentary { .. } => "momentary",
            InputCap::Relative => "relative",
            InputCap::Absolute { .. } => "absolute",
            InputCap::Touch => "touch",
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
    pub fn kind(&self) -> &'static str {
        match self {
            OutputCap::Image(_) => "image",
            OutputCap::Led { .. } => "led",
            OutputCap::Level { .. } => "level",
            OutputCap::Text { .. } => "text",
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

    fn input(&self, kind: &str) -> Option<&InputCap> {
        self.inputs.iter().find(|i| i.kind() == kind)
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
    pub kind: InputKind,
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
    #[error("no page {0}")]
    UnknownPage(PageId),
    #[error("page {page} doesn't fit: the surface has no key for {}", missing.join(", "))]
    DoesNotFit { page: PageId, missing: Vec<String> },
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
    pages: BTreeMap<PageId, Page>,
    attached: BTreeMap<SurfaceId, Attached>,
    /// The page each surface last showed, kept while it's detached.
    shown: BTreeMap<SurfaceId, Option<PageId>>,
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
        inner.bindings.hide(id);
        let path = StatePath::new(STATE_ROOT, vec![key(&surface.slug)]);
        inner.patch(Origin::system(), None, vec![(path, None)])?;
        Ok(())
    }

    /// Adds or replaces a page. Surfaces showing it whose layout no longer
    /// fits stop showing it.
    pub fn set_page(&self, page: Page) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        book.pages.insert(page.id.clone(), page.clone());
        let showing: Vec<SurfaceId> = book
            .shown
            .iter()
            .filter(|(_, p)| p.as_deref() == Some(page.id.as_str()))
            .map(|(s, _)| s.clone())
            .collect();
        for surface in showing {
            let fits = book
                .attached
                .get(&surface)
                .is_none_or(|a| missing(&page, &a.attach.controls).is_empty());
            if !fits {
                book.shown.insert(surface.clone(), None);
            }
            inner.publish_page(&book, &surface, Origin::system())?;
            inner.show_on(&book, &surface);
        }
        Ok(())
    }

    /// Removes a page from the surfaces showing it.
    pub fn remove_page(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        book.pages.remove(id);
        let showing: Vec<SurfaceId> = book
            .shown
            .iter()
            .filter(|(_, p)| p.as_deref() == Some(id))
            .map(|(s, _)| s.clone())
            .collect();
        for surface in showing {
            book.shown.insert(surface.clone(), None);
            inner.publish_page(&book, &surface, Origin::system())?;
            inner.show_on(&book, &surface);
        }
        Ok(())
    }

    /// Every page's layout, by ID, for validating `gesture` triggers.
    pub fn layouts(&self) -> BTreeMap<PageId, Layout> {
        let book = self.inner.book();
        book.pages
            .iter()
            .map(|(id, p)| (id.clone(), p.layout))
            .collect()
    }

    /// The page a surface shows, or last showed while detached.
    pub fn shown(&self, surface: &str) -> Option<PageId> {
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
    /// pages evaluated and drawn.
    pub async fn flush(&self) {
        self.inner.bindings.flush().await;
    }

    pub fn is_attached(&self, surface: &str) -> bool {
        self.inner.book().attached.contains_key(surface)
    }

    /// Shows a page on a surface, or nothing (surfaces spec §5). It must fit
    /// an attached surface (§5.4).
    pub fn show(&self, surface: &str, page: Option<&str>, origin: Origin) -> Result<(), ShowError> {
        let inner = &self.inner;
        let mut book = inner.book();
        if !book.surfaces.contains_key(surface) {
            return Err(ShowError::UnknownSurface(surface.to_owned()));
        }
        if let Some(id) = page {
            let page = book
                .pages
                .get(id)
                .ok_or_else(|| ShowError::UnknownPage(id.to_owned()))?;
            if let Some(attached) = book.attached.get(surface) {
                let missing = missing(page, &attached.attach.controls);
                if !missing.is_empty() {
                    return Err(ShowError::DoesNotFit {
                        page: id.to_owned(),
                        missing,
                    });
                }
            }
        }
        book.shown
            .insert(surface.to_owned(), page.map(str::to_owned));
        inner.publish_page(&book, surface, origin)?;
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
        // The page it showed last, or its start page the first time.
        let page = match book.shown.get(surface) {
            Some(page) => page.clone(),
            None => entity.start_page.clone(),
        }
        .filter(|p| {
            book.pages
                .get(p)
                .is_some_and(|p| missing(p, &attach.controls).is_empty())
        });
        book.shown.insert(surface.to_owned(), page);
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
                return invalid(format!("control {id:?} has two {kind} inputs"));
            }
        }
        if let Some(InputCap::Absolute { axes: Some(axes) }) = control.input("absolute")
            && !(1..=2).contains(axes)
        {
            return invalid(format!("control {id:?} has {axes} axes: use 1 or 2"));
        }
        let mut kinds = BTreeSet::new();
        for output in &control.outputs {
            if !kinds.insert(output.kind()) {
                return invalid(format!("control {id:?} has two {} outputs", output.kind()));
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

/// The page's controls the surface lacks (surfaces spec §5.4): each must be
/// a key there, with a `momentary` input and an `image` output.
fn missing(page: &Page, controls: &[ControlDef]) -> Vec<String> {
    page.layout
        .controls()
        .filter(|id| {
            !controls
                .iter()
                .any(|c| &c.id == id && c.input("momentary").is_some() && c.image().is_some())
        })
        .collect()
}

impl Surface {
    /// Reads a surface from JSON, or one `invalid` problem.
    pub fn parse(json: &serde_json::Value) -> Result<Surface, Vec<Problem>> {
        crate::binding::deserialize(json, &[]).map_err(|p| vec![p])
    }
}

impl Page {
    /// Reads a page from JSON, or one `invalid` problem.
    pub fn parse(json: &serde_json::Value) -> Result<Page, Vec<Problem>> {
        crate::binding::deserialize(json, &[]).map_err(|p| vec![p])
    }
}

/// Checks a surface: a name, a slug no other surface has, a model, and a
/// start page that exists.
pub fn validate_surface(
    surface: &Surface,
    taken_slugs: &BTreeSet<String>,
    pages: &BTreeMap<PageId, Layout>,
) -> Report {
    let mut errors = named("surface", &surface.name, &surface.slug, taken_slugs);
    if surface.model.trim().is_empty() {
        errors.push(Problem::new(
            vec![key("model")],
            Code::Invalid,
            "model is required",
        ));
    }
    if let Some(page) = &surface.start_page
        && !pages.contains_key(page)
    {
        errors.push(Problem::new(
            vec![key("startPage")],
            Code::UnknownPage,
            format!("no page {page:?}"),
        ));
    }
    Report {
        errors,
        warnings: Vec::new(),
    }
}

/// Checks a page: a name, a slug no other page has, and a layout that
/// still holds every control bound on it.
pub fn validate_page(
    page: &Page,
    taken_slugs: &BTreeSet<String>,
    bound: &BTreeSet<String>,
) -> Report {
    let mut errors = named("page", &page.name, &page.slug, taken_slugs);
    let Layout { cols, rows } = page.layout;
    if cols == 0 || rows == 0 || cols > 32 || rows > 32 {
        errors.push(Problem::new(
            vec![key("layout")],
            Code::Invalid,
            format!("a {cols}×{rows} layout isn't supported: use 1 to 32 columns and rows"),
        ));
    } else {
        let outside: Vec<&str> = bound
            .iter()
            .filter(|c| !page.layout.contains(c))
            .map(String::as_str)
            .collect();
        if !outside.is_empty() {
            let verb = if outside.len() == 1 { "is" } else { "are" };
            errors.push(Problem::new(
                vec![key("layout")],
                Code::UnknownControl,
                format!(
                    "{} {verb} bound, but a {cols}×{rows} layout ends at k{}",
                    outside.join(", "),
                    cols * rows - 1
                ),
            ));
        }
    }
    Report {
        errors,
        warnings: Vec::new(),
    }
}

/// The name and slug problems of a surface or page.
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
    /// The patch of each pressed control's press, and the page shown then:
    /// the cause and layer of its `long`, `hold` and `release`.
    downs: BTreeMap<String, (Seq, Option<PageId>)>,
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
        let needs = |kind: &str| {
            control
                .input(kind)
                .ok_or_else(|| format!("{:?} has no {kind} input", control.id))
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
            InputKind::Down => {
                let InputCap::Momentary { velocity } = needs("momentary")? else {
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
            InputKind::Up => {
                needs("momentary")?;
                if was_pressed {
                    vec![pressed(false, None)]
                } else {
                    Vec::new()
                }
            }
            InputKind::Turn => {
                needs("relative")?;
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
            InputKind::Value => {
                let InputCap::Absolute { axes } = needs("absolute")? else {
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
            InputKind::Touch => {
                needs("touch")?;
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
        let (slug, page) = {
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
        // The layer of a release is its press's; of anything else, the page
        // shown now.
        let layer = match &write.change {
            Change::Pressed(true) => {
                state.pressed = true;
                self.downs
                    .insert(input.control.clone(), (seq, page.clone()));
                page
            }
            Change::Pressed(false) => {
                state.pressed = false;
                self.downs
                    .remove(&input.control)
                    .map_or(page, |(_, layer)| layer)
            }
            Change::Turned(n) => {
                state.detents += n;
                page
            }
            Change::Value(v) => {
                state.value = Some(v.clone());
                page
            }
            Change::Touched(t) => {
                state.touched = *t;
                page
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
        let page = book.shown.get(&self.surface).cloned().flatten();
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
        self.journal_events(page.as_deref(), released, Some(cause));
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

    /// `connected`, `page` and, while attached, `model`.
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
            Self::path(&entity.slug, "page"),
            Some(Self::page_value(book, surface)),
        ));
        ops
    }

    /// The slug of the page a surface shows, or `null`.
    fn page_value(book: &Book, surface: &str) -> Value {
        book.shown
            .get(surface)
            .and_then(|p| p.as_ref())
            .and_then(|p| book.pages.get(p))
            .map_or(Value::Nil, |p| p.slug.as_str().into())
    }

    fn publish_page(
        &self,
        book: &Book,
        surface: &str,
        origin: Origin,
    ) -> Result<Seq, JournalError> {
        let slug = &book.surfaces[surface].slug;
        let value = Self::page_value(book, surface);
        self.patch(origin, None, vec![(Self::path(slug, "page"), Some(value))])
    }

    /// Tells the binding worker what an attached surface shows, so it
    /// evaluates the page and draws it.
    fn show_on(&self, book: &Book, surface: &str) {
        if let Some(attached) = book.attached.get(surface) {
            let page = book.shown.get(surface).cloned().flatten();
            self.bindings.show(
                surface,
                page,
                attached.attach.controls.clone(),
                attached.draws.clone(),
            );
        }
    }
}
