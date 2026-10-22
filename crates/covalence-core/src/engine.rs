//! The core: ties the journal, state tree, output gate and host together.
//!
//! An action goes `action.requested` → gate (`action.dispatched` or
//! `action.suppressed`) → `action.invoke` → `action.result`. Both the gate's
//! verdict and the result point their `cause` at the request, so `why` on the
//! result reaches whatever caused the request.

use std::future::Future;
use std::pin::Pin;
use std::sync::{RwLock, RwLockReadGuard};
use std::time::Instant;

use covalence_protocol::action::ActionInvoke;
use covalence_protocol::{ProtocolError, Value};

use crate::Shared;
use crate::gate::{GateInput, OutputGate, Verdict};
use crate::host::{Host, HostConfig};
use crate::journal::{
    ActionRequestedRecord, ActionResultRecord, JournalError, NewEntry, Origin, Record, Seq,
};
use crate::state::ConnId;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("unknown connection {0}")]
    UnknownConnection(ConnId),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Clone, Debug)]
pub struct ActionRequest {
    pub origin: Origin,
    /// The entry that caused this request, if any.
    pub cause: Option<Seq>,
    pub run: Option<String>,
    pub conn: ConnId,
    pub action: String,
    pub params: Value,
    /// Milliseconds.
    pub deadline: Option<u32>,
    /// The term the request was made under; defaults to the current term.
    pub term: Option<u32>,
}

impl ActionRequest {
    pub fn new(
        origin: Origin,
        conn: impl Into<ConnId>,
        action: impl Into<String>,
        params: Value,
    ) -> Self {
        ActionRequest {
            origin,
            cause: None,
            run: None,
            conn: conn.into(),
            action: action.into(),
            params,
            deadline: None,
            term: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionOutcome {
    pub call: String,
    /// The `action.requested` entry.
    pub requested: Seq,
    pub verdict: Verdict,
    /// The `action.result` entry, if the action was dispatched.
    pub result_seq: Option<Seq>,
    /// What the host answered, if the action was dispatched.
    pub result: Option<Result<Option<Value>, ProtocolError>>,
}

/// Carries out action requests: the [`Core`], or a stand-in in tests. The
/// flow runtime requests actions through this.
pub trait Actions: Send + Sync + 'static {
    fn request_action(
        &self,
        req: ActionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CoreError>> + Send + '_>>;

    /// The JSON Schema for an action's params, from its plugin's manifest, so
    /// params computed by expressions can be checked before the request
    /// (flow spec §8.3). `None` when it isn't known; the plugin still checks.
    fn params_schema<'a>(
        &'a self,
        _conn: &'a str,
        _action: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<serde_json::Value>> + Send + 'a>> {
        Box::pin(async { None })
    }
}

impl Actions for Core {
    fn request_action(
        &self,
        req: ActionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CoreError>> + Send + '_>> {
        Box::pin(Core::request_action(self, req))
    }
}

pub struct Core {
    shared: Shared,
    gate: RwLock<OutputGate>,
    host: Host,
}

impl Core {
    /// Starts the core and its plugin host. Must be called inside a tokio runtime.
    pub fn start(shared: Shared, host: HostConfig) -> Self {
        let host = Host::start(host, shared.clone());
        Core {
            shared,
            gate: RwLock::new(OutputGate::default()),
            host,
        }
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn gate(&self) -> RwLockReadGuard<'_, OutputGate> {
        self.gate.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_simulate(&self, simulate: bool) {
        self.gate
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .simulate = simulate;
    }

    /// Journals the request, passes it through the output gate and, if it is
    /// dispatched, invokes it and journals the result.
    pub async fn request_action(&self, req: ActionRequest) -> Result<ActionOutcome, CoreError> {
        let journal = &self.shared.journal;
        let conn_mode = self
            .host
            .conn_mode(&req.conn)
            .ok_or_else(|| CoreError::UnknownConnection(req.conn.clone()))?;
        let term = req.term.unwrap_or_else(|| journal.term());
        let call = ulid::Ulid::generate().to_string();
        let requested = journal
            .append(
                NewEntry::new(
                    req.origin,
                    Record::ActionRequested(ActionRequestedRecord {
                        call: call.clone(),
                        conn: req.conn.clone(),
                        action: req.action.clone(),
                        params: req.params.clone(),
                        deadline: req.deadline,
                    }),
                )
                .cause(req.cause)
                .run(req.run.clone()),
            )?
            .seq;
        let input = GateInput {
            call: &call,
            requested,
            run: req.run.as_deref(),
            conn_mode,
            term,
        };
        let verdict = self.gate().pass(journal, &input)?.0;
        let mut outcome = ActionOutcome {
            call: call.clone(),
            requested,
            verdict,
            result_seq: None,
            result: None,
        };
        if verdict != Verdict::Dispatch {
            return Ok(outcome);
        }

        let invoke = ActionInvoke {
            call: call.clone(),
            run: req.run.clone(),
            term,
            action: req.action,
            params: req.params,
            deadline: req.deadline,
        };
        let started = Instant::now();
        let result = self
            .host
            .invoke(&req.conn, &invoke, Some(requested))
            .await
            .map(|r| r.result);
        let duration_ms = started.elapsed().as_millis() as u64;
        let (ok, value, error) = match &result {
            Ok(value) => (true, value.clone(), None),
            Err(e) => (false, None, Some(e.clone())),
        };
        let entry = journal.append(
            NewEntry::new(
                Origin::conn(req.conn),
                Record::ActionResult(ActionResultRecord {
                    call,
                    ok,
                    result: value,
                    error,
                    duration_ms,
                }),
            )
            .cause(requested)
            .run(req.run),
        )?;
        outcome.result_seq = Some(entry.seq);
        outcome.result = Some(result);
        Ok(outcome)
    }

    /// Stops the plugin host.
    pub async fn shutdown(self) {
        self.host.shutdown().await;
    }
}
