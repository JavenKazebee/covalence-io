//! The output gate (state spec §3, ADR 0007).
//!
//! Every side effect passes here after its `action.requested` entry. The
//! checks run in the spec's order and the first failure wins:
//!
//! 1. node role: a pass-through stub until redundancy lands (ADR 0008)
//! 2. simulate mode
//! 3. show lock and permissions: a pass-through stub
//! 4. term fencing
//!
//! The gate then journals `action.dispatched` or `action.suppressed`.

use std::sync::Arc;

use covalence_protocol::connection::ConnMode;

use crate::journal::{
    ActionDispatchedRecord, ActionSuppressedRecord, Entry, Journal, JournalError, NewEntry, Origin,
    Record, Seq, SuppressReason,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Active,
    Standby,
}

/// What the gate needs to know about one requested action.
#[derive(Clone, Debug)]
pub struct GateInput<'a> {
    pub call: &'a str,
    /// The `action.requested` entry.
    pub requested: Seq,
    pub run: Option<&'a str>,
    /// The target connection's mode. A simulated connection has no real side
    /// effects, so simulate mode lets actions through to it.
    pub conn_mode: ConnMode,
    /// The term the action was requested under.
    pub term: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Dispatch,
    Suppress(SuppressReason),
}

#[derive(Clone, Debug)]
pub struct OutputGate {
    pub role: Role,
    /// Simulate (rehearsal) mode: no live outputs.
    pub simulate: bool,
}

impl Default for OutputGate {
    fn default() -> Self {
        OutputGate {
            role: Role::Active,
            simulate: false,
        }
    }
}

impl OutputGate {
    /// Runs the checks without journaling. `current_term` is the node's term now.
    pub fn check(&self, input: &GateInput<'_>, current_term: u32) -> Verdict {
        if let Some(reason) = self.check_role() {
            return Verdict::Suppress(reason);
        }
        if self.simulate && input.conn_mode == ConnMode::Live {
            return Verdict::Suppress(SuppressReason::Simulate);
        }
        if let Some(reason) = self.check_show_lock(input) {
            return Verdict::Suppress(reason);
        }
        if input.term < current_term {
            return Verdict::Suppress(SuppressReason::StaleTerm);
        }
        Verdict::Dispatch
    }

    /// Stub: a single node is always active. Redundancy (ADR 0008) will return
    /// `Standby` here when `self.role` is not active.
    fn check_role(&self) -> Option<SuppressReason> {
        let _ = self.role;
        None
    }

    /// Stub: no show lock or per-user permissions yet.
    fn check_show_lock(&self, _input: &GateInput<'_>) -> Option<SuppressReason> {
        None
    }

    /// Runs the checks and journals the verdict, caused by the request entry.
    pub fn pass(
        &self,
        journal: &Journal,
        input: &GateInput<'_>,
    ) -> Result<(Verdict, Arc<Entry>), JournalError> {
        let verdict = self.check(input, journal.term());
        let call = input.call.to_owned();
        let record = match verdict {
            Verdict::Dispatch => Record::ActionDispatched(ActionDispatchedRecord { call }),
            Verdict::Suppress(reason) => {
                Record::ActionSuppressed(ActionSuppressedRecord { call, reason })
            }
        };
        let entry = journal.append(
            NewEntry::new(Origin::system(), record)
                .cause(input.requested)
                .run(input.run.map(str::to_owned)),
        )?;
        Ok((verdict, entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(conn_mode: ConnMode, term: u32) -> GateInput<'static> {
        GateInput {
            call: "c",
            requested: 1,
            run: None,
            conn_mode,
            term,
        }
    }

    #[test]
    fn dispatches_by_default() {
        let gate = OutputGate::default();
        assert_eq!(gate.check(&input(ConnMode::Live, 1), 1), Verdict::Dispatch);
    }

    #[test]
    fn simulate_mode_suppresses_live_connections_only() {
        let gate = OutputGate {
            simulate: true,
            ..OutputGate::default()
        };
        assert_eq!(
            gate.check(&input(ConnMode::Live, 1), 1),
            Verdict::Suppress(SuppressReason::Simulate)
        );
        assert_eq!(
            gate.check(&input(ConnMode::Simulate, 1), 1),
            Verdict::Dispatch
        );
    }

    #[test]
    fn stale_term_is_fenced() {
        let gate = OutputGate::default();
        assert_eq!(
            gate.check(&input(ConnMode::Live, 1), 2),
            Verdict::Suppress(SuppressReason::StaleTerm)
        );
    }

    #[test]
    fn simulate_is_checked_before_term() {
        let gate = OutputGate {
            simulate: true,
            ..OutputGate::default()
        };
        assert_eq!(
            gate.check(&input(ConnMode::Live, 1), 2),
            Verdict::Suppress(SuppressReason::Simulate)
        );
    }

    #[test]
    fn role_and_show_lock_are_pass_through_stubs() {
        let gate = OutputGate {
            role: Role::Standby,
            ..OutputGate::default()
        };
        assert_eq!(gate.check(&input(ConnMode::Live, 1), 1), Verdict::Dispatch);
    }

    #[test]
    fn journals_the_verdict_caused_by_the_request() {
        let journal = Journal::in_memory(8);
        journal
            .append(NewEntry::new(
                Origin::system(),
                Record::ActionDispatched(ActionDispatchedRecord { call: "x".into() }),
            ))
            .unwrap();
        journal.set_term(2).unwrap();
        let (verdict, entry) = OutputGate::default()
            .pass(&journal, &input(ConnMode::Live, 1))
            .unwrap();
        assert_eq!(verdict, Verdict::Suppress(SuppressReason::StaleTerm));
        assert_eq!(entry.cause, Some(1));
        assert!(matches!(
            &entry.record,
            Record::ActionSuppressed(r) if r.reason == SuppressReason::StaleTerm
        ));
    }
}
