//! Parameter definitions (composition spec §1, §2.1): the one shape for a flow's
//! `inputs` and, later, a template's `params`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

use crate::source::Source;

/// A declared parameter: a state schema node (`type`, `unit`, `kind`,
/// `minimum` … , state spec §1.2) with a default and whether it must be given.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamDef {
    /// Used when nothing is bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Source<Json>>,
    /// With no default, a value has to be given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// Everything else: the schema a value is checked against.
    #[serde(flatten)]
    pub schema: Map<String, Json>,
}

impl ParamDef {
    /// The schema node, as `schema::check` takes it.
    pub fn schema(&self) -> Json {
        Json::Object(self.schema.clone())
    }

    /// Required and without a default: something has to bind it.
    pub fn needs_value(&self) -> bool {
        self.required == Some(true) && self.default.is_none()
    }
}
