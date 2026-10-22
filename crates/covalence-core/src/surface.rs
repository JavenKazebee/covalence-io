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
//! Each attached surface has a task that journals its raw input as
//! `input.raw`, turns it into gestures ([`gesture`]) and journals those as
//! `input.gesture`, with `cause` → the raw input that completed them. Flows
//! match `gesture` triggers on those entries. The surface's state,
//! `surface.<slug>.*`, is written through the journal as `state.patch`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use covalence_protocol::state::PathSegment;
use covalence_protocol::{True, Value};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::journal::{
    InputGestureRecord, InputKind, InputRawRecord, JournalError, JournalOp, NewEntry, Origin,
    Record, Seq, StatePatchRecord,
};
use crate::state::{Meta, Quality, Source, StatePath};
use crate::{Shared, clock};

pub mod gesture;
pub mod render;

use crate::binding::ControlBinding;
use crate::binding::worker::{Bindings, Problems};
use gesture::{Detected, Recogniser, Timing};

/// Entity types in the store.
pub const SURFACE_ENTITY: &str = "surface";
pub const PAGE_ENTITY: &str = "page";
/// The state root for surfaces: `{conn: "surface", path: [slug, ...]}`.
pub const STATE_ROOT: &str = "surface";
/// How many draws wait for a slow transport before more are dropped.
const DRAW_BACKLOG: usize = 256;
/// How much input waits for a busy core.
const INPUT_BACKLOG: usize = 256;

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
}

/// One input or output on a surface (plugin protocol §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlDef {
    pub id: String,
    pub kind: ControlKind,
    pub x: u32,
    pub y: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    /// The image the control shows, if it shows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressable: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlKind {
    Button,
    Encoder,
    Fader,
    Touchstrip,
    Lcd,
    Led,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// `surface.attach`, without the surface: the transport knows which it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attach {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub name: String,
    pub controls: Vec<ControlDef>,
    pub layout: Layout,
}

/// `surface.input`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub control: String,
    pub kind: InputKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The host's timestamp, in milliseconds on its own clock.
    pub ts: f64,
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
    #[error("page {page} is {page_cols}×{page_rows}, but the surface is {cols}×{rows}")]
    LayoutMismatch {
        page: PageId,
        page_cols: u32,
        page_rows: u32,
        cols: u32,
        rows: u32,
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
                .is_none_or(|a| a.attach.layout == page.layout);
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

    /// Shows a page on a surface, or nothing (surfaces spec §5). An attached
    /// surface must have the page's layout.
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
            if let Some(attached) = book.attached.get(surface)
                && attached.attach.layout != page.layout
            {
                let Layout { cols, rows } = attached.attach.layout;
                return Err(ShowError::LayoutMismatch {
                    page: id.to_owned(),
                    page_cols: page.layout.cols,
                    page_rows: page.layout.rows,
                    cols,
                    rows,
                });
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
        .filter(|p| book.pages.get(p).is_some_and(|p| p.layout == attach.layout));
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
        let ops = inner.surface_state(&book, surface);
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

/// What makes an attach unusable: no controls, a duplicate control, or a
/// button that isn't in the layout.
fn check_attach(attach: &Attach) -> Result<(), AttachError> {
    let invalid = |m: String| Err(AttachError::Invalid(m));
    let Layout { cols, rows } = attach.layout;
    if cols == 0 || rows == 0 || cols > 32 || rows > 32 {
        return invalid(format!("a {cols}×{rows} layout isn't supported"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for control in &attach.controls {
        if !seen.insert(&control.id) {
            return invalid(format!("control {:?} appears twice", control.id));
        }
        if control.kind == ControlKind::Button && !attach.layout.contains(&control.id) {
            return invalid(format!(
                "button {:?} must be k0 to k{} in a {cols}×{rows} layout",
                control.id,
                cols * rows - 1
            ));
        }
    }
    Ok(())
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
    let mut recogniser = Recogniser::new(inner.timing);
    // The `input.raw` of each control's last `down`, the cause of a timed
    // `long` or `hold`.
    let mut downs: BTreeMap<String, Seq> = BTreeMap::new();
    loop {
        let deadline = recogniser.deadline();
        tokio::select! {
            _ = &mut stop => {
                // Replaced or deleted: whoever did that owns the state now.
                inner.journal_gestures(&surface, None, recogniser.release_all(), None);
                return;
            }
            next = input.recv() => match next {
                Some(i) => inner.input(&surface, session, &mut recogniser, &mut downs, i),
                None => {
                    inner.detach(&surface, session, recogniser.release_all());
                    return;
                }
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                let page = inner.book().shown.get(&surface).cloned().flatten();
                for found in recogniser.tick(Instant::now()) {
                    let cause = downs.get(&found.control).copied();
                    inner.journal_gestures(&surface, page.as_deref(), vec![found], cause);
                }
            }
        }
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
        });
        let journal = &self.shared.journal;
        self.shared.state.mutate(|tree| {
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
        })
    }

    fn path(slug: &str, leaf: &str) -> StatePath {
        StatePath::new(STATE_ROOT, vec![key(slug), key(leaf)])
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

    /// Journals one input and the gestures it completes.
    fn input(
        &self,
        surface: &str,
        session: u64,
        recogniser: &mut Recogniser,
        downs: &mut BTreeMap<String, Seq>,
        input: Input,
    ) {
        let page = {
            let book = self.book();
            let Some(attached) = book.attached.get(surface).filter(|a| a.session == session) else {
                return;
            };
            if !attached
                .attach
                .controls
                .iter()
                .any(|c| c.id == input.control)
            {
                tracing::warn!(%surface, control = %input.control, "input from an unknown control");
                return;
            }
            book.shown.get(surface).cloned().flatten()
        };
        let raw = self.shared.journal.append(NewEntry::new(
            surface_origin(surface),
            Record::InputRaw(InputRawRecord {
                surface: surface.to_owned(),
                control: input.control.clone(),
                kind: input.kind,
                value: input.value,
                device_ts: input.ts,
            }),
        ));
        let raw = match raw {
            Ok(entry) => entry.seq,
            Err(e) => {
                tracing::error!(%surface, "cannot journal input: {e}");
                return;
            }
        };
        if input.kind == InputKind::Down {
            downs.insert(input.control.clone(), raw);
        }
        let found = recogniser.input(&input.control, input.kind, input.ts, Instant::now());
        self.journal_gestures(surface, page.as_deref(), found, Some(raw));
    }

    fn journal_gestures(
        &self,
        surface: &str,
        page: Option<&str>,
        found: Vec<Detected>,
        cause: Option<Seq>,
    ) {
        for Detected {
            control,
            gesture,
            delta,
        } in found
        {
            let entry = NewEntry::new(
                surface_origin(surface),
                Record::InputGesture(InputGestureRecord {
                    surface: surface.to_owned(),
                    page: page.map(str::to_owned),
                    control,
                    gesture,
                    delta,
                }),
            )
            .cause(cause);
            if let Err(e) = self.shared.journal.append(entry) {
                tracing::error!(%surface, "cannot journal a gesture: {e}");
            }
        }
    }

    /// The transport went away: `connected` becomes `false`, then whatever
    /// was down is released, with `cause` → that change.
    fn detach(&self, surface: &str, session: u64, released: Vec<Detected>) {
        let mut book = self.book();
        if book
            .attached
            .get(surface)
            .is_none_or(|a| a.session != session)
        {
            return;
        }
        book.attached.remove(surface);
        self.bindings.hide(surface);
        let page = book.shown.get(surface).cloned().flatten();
        let slug = book.surfaces[surface].slug.clone();
        drop(book);
        let path = Self::path(&slug, "connected");
        match self.patch(
            surface_origin(surface),
            None,
            vec![(path, Some(false.into()))],
        ) {
            Ok(seq) => self.journal_gestures(surface, page.as_deref(), released, Some(seq)),
            Err(e) => tracing::error!(%surface, "cannot journal a detach: {e}"),
        }
    }
}
