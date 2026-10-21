//! The runtime journal (state spec §2.1–2.2).
//!
//! Entries get a gap-free `seq` under one lock, are written to SQLite (when a
//! store is attached) before they become visible, and are kept in an in-memory
//! ring for fast reads. Only the kinds Milestone 1 needs exist so far.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime};

use covalence_protocol::connection::ConnStatusKind;
use covalence_protocol::state::LogLevel;
use covalence_protocol::{Object, ProtocolError, True, Value, ValueError, from_value, to_value};
use covalence_store::{JournalRow, Store, StoreError};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::clock;
use crate::state::{ConnId, Quality, Source, StatePath};

pub type Seq = u64;

/// Default number of entries kept in memory.
pub const DEFAULT_RING_CAPACITY: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: Seq,
    /// Leadership term (ADR 0008); 1 for a single node.
    pub term: u32,
    /// Wall clock, RFC 3339 with microseconds. For display only.
    pub ts: String,
    /// Monotonic nanoseconds since node start.
    pub mono: u64,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Seq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(flatten)]
    pub record: Record,
}

/// Who produced an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Origin {
    // Most specific shapes first: untagged decoding takes the first match.
    Flow {
        flow: String,
        run: String,
        /// Absent on run-level entries (`run.started`, `run.finished`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<String>,
    },
    User {
        user: String,
        client: String,
    },
    Surface {
        surface: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orbital: Option<String>,
    },
    Conn {
        conn: ConnId,
    },
    Core {
        core: CoreOrigin,
    },
}

impl Origin {
    pub fn conn(conn: impl Into<ConnId>) -> Self {
        Origin::Conn { conn: conn.into() }
    }

    pub fn system() -> Self {
        Origin::Core {
            core: CoreOrigin::System,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreOrigin {
    Scheduler,
    System,
    Replication,
}

/// The kind and its data. On the wire: `{kind, data}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Record {
    #[serde(rename = "state.patch")]
    StatePatch(StatePatchRecord),
    #[serde(rename = "state.quality")]
    StateQuality(StateQualityRecord),
    #[serde(rename = "event")]
    Event(EventRecord),
    #[serde(rename = "action.requested")]
    ActionRequested(ActionRequestedRecord),
    #[serde(rename = "action.dispatched")]
    ActionDispatched(ActionDispatchedRecord),
    #[serde(rename = "action.suppressed")]
    ActionSuppressed(ActionSuppressedRecord),
    #[serde(rename = "action.result")]
    ActionResult(ActionResultRecord),
    #[serde(rename = "conn.status")]
    ConnStatus(ConnStatusRecord),
    #[serde(rename = "log")]
    Log(LogRecord),
}

impl Record {
    pub fn kind(&self) -> &'static str {
        match self {
            Record::StatePatch(_) => "state.patch",
            Record::StateQuality(_) => "state.quality",
            Record::Event(_) => "event",
            Record::ActionRequested(_) => "action.requested",
            Record::ActionDispatched(_) => "action.dispatched",
            Record::ActionSuppressed(_) => "action.suppressed",
            Record::ActionResult(_) => "action.result",
            Record::ConnStatus(_) => "conn.status",
            Record::Log(_) => "log",
        }
    }
}

/// `state.patch`. A `state.snapshot` is journaled as a patch with `replaces`
/// set: the subtree was cleared, then `ops` applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatePatchRecord {
    pub ops: Vec<JournalOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<StatePath>,
}

/// A patch op with its path in ID form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JournalOp {
    Delete { path: StatePath, delete: True },
    Set { path: StatePath, value: Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateQualityRecord {
    pub subtree: StatePath,
    pub quality: Quality,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub source: Source,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionRequestedRecord {
    /// ULID; the idempotency key.
    pub call: String,
    pub conn: ConnId,
    pub action: String,
    pub params: Value,
    /// Milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionDispatchedRecord {
    pub call: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionSuppressedRecord {
    pub call: String,
    pub reason: SuppressReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuppressReason {
    Standby,
    Simulate,
    ShowLock,
    Permission,
    StaleTerm,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResultRecord {
    pub call: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnStatusRecord {
    pub conn: ConnId,
    pub status: ConnStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    pub level: LogLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Object>,
}

/// An entry before it gets its `seq`, `term` and timestamps.
#[derive(Clone, Debug)]
pub struct NewEntry {
    pub origin: Origin,
    pub cause: Option<Seq>,
    pub run: Option<String>,
    pub record: Record,
}

impl NewEntry {
    pub fn new(origin: Origin, record: Record) -> Self {
        NewEntry {
            origin,
            cause: None,
            run: None,
            record,
        }
    }

    pub fn cause(mut self, cause: impl Into<Option<Seq>>) -> Self {
        self.cause = cause.into();
        self
    }

    pub fn run(mut self, run: impl Into<Option<String>>) -> Self {
        self.run = run.into();
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("cannot encode entry: {0}")]
    Encode(String),
    #[error("cannot decode entry {seq}: {message}")]
    Decode { seq: Seq, message: String },
    #[error("cause {cause} must be an earlier entry than {seq}")]
    InvalidCause { seq: Seq, cause: Seq },
}

pub struct Journal {
    inner: Mutex<Inner>,
    events: broadcast::Sender<Arc<Entry>>,
    start: Instant,
}

struct Inner {
    next_seq: Seq,
    term: u32,
    ring: VecDeque<Arc<Entry>>,
    capacity: usize,
    store: Option<Store>,
}

impl Journal {
    /// A journal kept only in memory.
    pub fn in_memory(capacity: usize) -> Self {
        Self::build(1, 1, capacity, None)
    }

    /// A journal persisted to `store`, continuing after its last entry.
    pub fn with_store(store: Store, capacity: usize) -> Result<Self, JournalError> {
        let next_seq = store.last_journal_seq()?.map_or(1, |s| s + 1);
        let term = store
            .meta("term")?
            .and_then(|t| t.parse().ok())
            .unwrap_or(1);
        Ok(Self::build(next_seq, term, capacity, Some(store)))
    }

    fn build(next_seq: Seq, term: u32, capacity: usize, store: Option<Store>) -> Self {
        let (events, _) = broadcast::channel(1024);
        Journal {
            inner: Mutex::new(Inner {
                next_seq,
                term,
                ring: VecDeque::new(),
                capacity: capacity.max(1),
                store,
            }),
            events,
            start: Instant::now(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn term(&self) -> u32 {
        self.lock().term
    }

    /// Changes the leadership term (on promotion, ADR 0008).
    pub fn set_term(&self, term: u32) -> Result<(), JournalError> {
        let mut inner = self.lock();
        if let Some(store) = &inner.store {
            store.set_meta("term", &term.to_string())?;
        }
        inner.term = term;
        Ok(())
    }

    /// The newest entry's seq.
    pub fn last_seq(&self) -> Option<Seq> {
        self.lock().next_seq.checked_sub(1).filter(|&s| s > 0)
    }

    /// Live feed of appended entries.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Entry>> {
        self.events.subscribe()
    }

    /// Appends an entry and returns it. On error nothing is appended and the
    /// seq is not used, so the log stays gap-free.
    pub fn append(&self, new: NewEntry) -> Result<Arc<Entry>, JournalError> {
        let mut inner = self.lock();
        let seq = inner.next_seq;
        if let Some(cause) = new.cause
            && cause >= seq
        {
            return Err(JournalError::InvalidCause { seq, cause });
        }
        let entry = Entry {
            seq,
            term: inner.term,
            ts: clock::rfc3339_micros(SystemTime::now()),
            mono: self.start.elapsed().as_nanos() as u64,
            origin: new.origin,
            cause: new.cause,
            run: new.run,
            record: new.record,
        };
        if let Some(store) = &inner.store {
            store.append_journal(&to_row(&entry)?)?;
        }
        let entry = Arc::new(entry);
        inner.next_seq += 1;
        if inner.ring.len() == inner.capacity {
            inner.ring.pop_front();
        }
        inner.ring.push_back(entry.clone());
        drop(inner);
        let _ = self.events.send(entry.clone());
        Ok(entry)
    }

    pub fn get(&self, seq: Seq) -> Result<Option<Arc<Entry>>, JournalError> {
        let inner = self.lock();
        if let Some(front) = inner.ring.front()
            && seq >= front.seq
        {
            return Ok(inner.ring.get((seq - front.seq) as usize).cloned());
        }
        match &inner.store {
            Some(store) => store
                .journal_entry(seq)?
                .map(|row| from_row(&row).map(Arc::new))
                .transpose(),
            None => Ok(None),
        }
    }

    /// Up to `limit` entries with `seq >= from`, in order.
    pub fn range(&self, from: Seq, limit: usize) -> Result<Vec<Arc<Entry>>, JournalError> {
        let inner = self.lock();
        let in_ring = inner.ring.front().is_some_and(|front| from >= front.seq);
        match &inner.store {
            Some(store) if !in_ring => store
                .journal_range(from, limit)?
                .iter()
                .map(|row| from_row(row).map(Arc::new))
                .collect(),
            _ => Ok(inner
                .ring
                .iter()
                .skip_while(|e| e.seq < from)
                .take(limit)
                .cloned()
                .collect()),
        }
    }

    /// "Why did this fire?": the entry at `seq`, then its cause, and so on back
    /// to the root. Stops early if an entry has aged out of retention.
    pub fn why(&self, seq: Seq) -> Result<Vec<Arc<Entry>>, JournalError> {
        let mut chain = Vec::new();
        let mut next = Some(seq);
        while let Some(seq) = next {
            let Some(entry) = self.get(seq)? else { break };
            next = entry.cause;
            chain.push(entry);
        }
        Ok(chain)
    }
}

fn to_row(entry: &Entry) -> Result<JournalRow, JournalError> {
    let encode = |e: &dyn std::fmt::Display| JournalError::Encode(e.to_string());
    let tagged = to_value(&entry.record).map_err(|e| encode(&e))?;
    let data = tagged
        .as_map()
        .and_then(|m| m.iter().find(|(k, _)| k.as_str() == Some("data")))
        .map(|(_, v)| v)
        .ok_or_else(|| JournalError::Encode("record has no data".into()))?;
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, data).map_err(|e| encode(&e))?;
    Ok(JournalRow {
        seq: entry.seq,
        term: entry.term,
        ts: entry.ts.clone(),
        mono: entry.mono,
        origin: serde_json::to_string(&entry.origin).map_err(|e| encode(&e))?,
        cause: entry.cause,
        run: entry.run.clone(),
        kind: entry.record.kind().to_owned(),
        data: bytes,
    })
}

fn from_row(row: &JournalRow) -> Result<Entry, JournalError> {
    let decode = |message: String| JournalError::Decode {
        seq: row.seq,
        message,
    };
    let data =
        rmpv::decode::read_value(&mut row.data.as_slice()).map_err(|e| decode(e.to_string()))?;
    let tagged = Value::Map(vec![
        ("kind".into(), row.kind.as_str().into()),
        ("data".into(), data),
    ]);
    let record = from_value(&tagged).map_err(|e: ValueError| decode(e.to_string()))?;
    Ok(Entry {
        seq: row.seq,
        term: row.term,
        ts: row.ts.clone(),
        mono: row.mono,
        origin: serde_json::from_str(&row.origin).map_err(|e| decode(e.to_string()))?,
        cause: row.cause,
        run: row.run.clone(),
        record,
    })
}

#[cfg(test)]
mod tests {
    use covalence_protocol::ErrorCode;
    use covalence_protocol::state::PathSegment;

    use super::*;

    fn requested(call: &str) -> Record {
        Record::ActionRequested(ActionRequestedRecord {
            call: call.into(),
            conn: "c1".into(),
            action: "route".into(),
            params: Value::Map(vec![
                ("output".into(), 1.into()),
                ("input".into(), 3.into()),
            ]),
            deadline: None,
        })
    }

    fn user() -> Origin {
        Origin::User {
            user: "u1".into(),
            client: "api".into(),
        }
    }

    fn chain(journal: &Journal) -> Seq {
        let req = journal
            .append(NewEntry::new(user(), requested("01")))
            .unwrap();
        let disp = journal
            .append(
                NewEntry::new(
                    Origin::system(),
                    Record::ActionDispatched(ActionDispatchedRecord { call: "01".into() }),
                )
                .cause(req.seq),
            )
            .unwrap();
        journal
            .append(
                NewEntry::new(
                    Origin::conn("c1"),
                    Record::ActionResult(ActionResultRecord {
                        call: "01".into(),
                        ok: false,
                        result: None,
                        error: Some(ProtocolError::new(ErrorCode::ActionFailed, "nope")),
                        duration_ms: 3,
                    }),
                )
                .cause(disp.seq),
            )
            .unwrap()
            .seq
    }

    #[test]
    fn seq_is_gap_free_and_why_walks_causes() {
        let journal = Journal::in_memory(16);
        let last = chain(&journal);
        assert_eq!(last, 3);
        let kinds: Vec<_> = journal
            .why(last)
            .unwrap()
            .iter()
            .map(|e| e.record.kind())
            .collect();
        assert_eq!(
            kinds,
            ["action.result", "action.dispatched", "action.requested"]
        );
    }

    #[test]
    fn rejects_forward_causes_without_using_a_seq() {
        let journal = Journal::in_memory(16);
        let bad = NewEntry::new(Origin::system(), requested("x")).cause(5);
        assert!(matches!(
            journal.append(bad),
            Err(JournalError::InvalidCause { .. })
        ));
        let ok = journal
            .append(NewEntry::new(Origin::system(), requested("y")))
            .unwrap();
        assert_eq!(ok.seq, 1);
    }

    #[test]
    fn ring_drops_oldest_and_store_still_answers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.db");
        let journal = Journal::with_store(Store::open(&path).unwrap(), 2).unwrap();
        let last = chain(&journal);
        assert_eq!(journal.range(1, 10).unwrap().len(), 3);
        assert_eq!(journal.why(last).unwrap().len(), 3);
        let first = journal.get(1).unwrap().unwrap();
        assert_eq!(first.record, requested("01"));
        drop(journal);

        // Reopening continues the sequence.
        let journal = Journal::with_store(Store::open(&path).unwrap(), 2).unwrap();
        assert_eq!(journal.last_seq(), Some(3));
        let next = journal
            .append(NewEntry::new(Origin::system(), requested("02")))
            .unwrap();
        assert_eq!(next.seq, 4);
        assert_eq!(journal.why(3).unwrap().len(), 3);
    }

    #[test]
    fn every_kind_round_trips_through_the_store() {
        let path = StatePath::new(
            "c1",
            vec![PathSegment::Key("outputs".into()), PathSegment::Index(1)],
        );
        let records = vec![
            Record::StatePatch(StatePatchRecord {
                ops: vec![
                    JournalOp::Set {
                        path: path.clone(),
                        value: Value::Binary(vec![1, 2]),
                    },
                    JournalOp::Delete {
                        path: path.clone(),
                        delete: True,
                    },
                ],
                replaces: Some(StatePath::root("c1")),
            }),
            Record::StateQuality(StateQualityRecord {
                subtree: StatePath::root("c1"),
                quality: Quality::Stale,
            }),
            Record::Event(EventRecord {
                source: Source::Conn("c1".into()),
                event: "routeChanged".into(),
                payload: Some(1.into()),
            }),
            requested("a"),
            Record::ActionSuppressed(ActionSuppressedRecord {
                call: "a".into(),
                reason: SuppressReason::StaleTerm,
            }),
            Record::ConnStatus(ConnStatusRecord {
                conn: "c1".into(),
                status: ConnStatusKind::Disconnected,
                message: Some("gone".into()),
            }),
            Record::Log(LogRecord {
                level: LogLevel::Warn,
                message: "hi".into(),
                fields: None,
            }),
        ];
        let journal = Journal::with_store(Store::open_in_memory().unwrap(), 1).unwrap();
        for record in &records {
            journal
                .append(NewEntry::new(Origin::conn("c1"), record.clone()))
                .unwrap();
        }
        let back: Vec<Record> = journal
            .range(1, 100)
            .unwrap()
            .iter()
            .map(|e| e.record.clone())
            .collect();
        assert_eq!(back, records);
    }

    #[test]
    fn entry_json_has_the_spec_shape() {
        let journal = Journal::in_memory(4);
        let entry = journal
            .append(NewEntry::new(user(), requested("01")))
            .unwrap();
        let json = serde_json::to_value(&*entry).unwrap();
        assert_eq!(json["kind"], "action.requested");
        assert_eq!(json["data"]["call"], "01");
        assert_eq!(json["origin"]["user"], "u1");
        assert_eq!(json["term"], 1);
        // serde_json sorts object keys, so compare in JSON form.
        let back: Entry = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn flow_origins_decode_with_and_without_a_step() {
        for origin in [
            Origin::Flow {
                flow: "f1".into(),
                run: "r1".into(),
                step: None,
            },
            Origin::Flow {
                flow: "f1".into(),
                run: "r1".into(),
                step: Some("route1".into()),
            },
            user(),
        ] {
            let json = serde_json::to_string(&origin).unwrap();
            assert_eq!(
                serde_json::from_str::<Origin>(&json).unwrap(),
                origin,
                "{json}"
            );
        }
        assert_eq!(
            serde_json::to_value(Origin::Flow {
                flow: "f1".into(),
                run: "r1".into(),
                step: None,
            })
            .unwrap(),
            serde_json::json!({ "flow": "f1", "run": "r1" })
        );
    }
}
