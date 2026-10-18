//! The Covalence IO engine: state tree, runtime journal, output gate and host supervision.

use std::sync::Arc;

pub mod clock;
pub mod engine;
pub mod gate;
pub mod host;
pub mod journal;
pub mod state;

pub use covalence_protocol::PROTOCOL_VERSION;
pub use engine::{ActionOutcome, ActionRequest, Core, CoreError};

/// The journal and state tree, shared by every subsystem.
#[derive(Clone)]
pub struct Shared {
    pub journal: Arc<journal::Journal>,
    pub state: Arc<state::LiveState>,
}

impl Shared {
    pub fn new(journal: journal::Journal) -> Self {
        Shared {
            journal: Arc::new(journal),
            state: Arc::new(state::LiveState::new()),
        }
    }
}
