//! Checks values against the subset of JSON Schema that the SDK's builders
//! produce (`packages/sdk/schema.ts`): `type`, `properties`, `required`,
//! `additionalProperties: false`, `enum`, `minLength`, `minimum` and `maximum`.
//! Anything else in a schema is ignored, as the SDK's `validate` ignores it.

use covalence_protocol::state::{Path, PathSegment};
use serde_json::Value;

/// One problem with a value: where it is, relative to the value, and what is wrong.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub path: Path,
    pub message: String,
}

/// Every problem with `value`, or none if it matches `schema`.
pub fn check(schema: &Value, value: &Value) -> Vec<Mismatch> {
    let mut out = Vec::new();
    check_at(schema, value, &mut Vec::new(), &mut out);
    out
}

fn check_at(schema: &Value, value: &Value, at: &mut Path, out: &mut Vec<Mismatch>) {
    let mut fail = |message: String| {
        out.push(Mismatch {
            path: at.clone(),
            message,
        })
    };
    let where_ = describe(at);
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let Some(obj) = value.as_object() else {
                return fail(format!("{where_} must be an object"));
            };
            let props = schema.get("properties").and_then(Value::as_object);
            let required = schema.get("required").and_then(Value::as_array);
            for key in required.into_iter().flatten().filter_map(Value::as_str) {
                if obj.get(key).is_none_or(Value::is_null) {
                    at.push(PathSegment::Key(key.to_owned()));
                    out.push(Mismatch {
                        path: at.clone(),
                        message: format!("{} is required", describe(at)),
                    });
                    at.pop();
                }
            }
            let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
            for (key, v) in obj {
                at.push(PathSegment::Key(key.clone()));
                match props.and_then(|p| p.get(key)) {
                    Some(sub) => check_at(sub, v, at, out),
                    None if closed => out.push(Mismatch {
                        path: at.clone(),
                        message: format!("{} is not allowed", describe(at)),
                    }),
                    None => {}
                }
                at.pop();
            }
        }
        Some("string") => {
            let Some(s) = value.as_str() else {
                return fail(format!("{where_} must be a string"));
            };
            if let Some(choices) = schema.get("enum").and_then(Value::as_array)
                && !choices.contains(value)
            {
                let names: Vec<&str> = choices.iter().filter_map(Value::as_str).collect();
                return fail(format!("{where_} must be one of {}", names.join(", ")));
            }
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && (s.chars().count() as u64) < min
            {
                fail(format!("{where_} is too short"));
            }
        }
        Some(kind @ ("integer" | "number")) => {
            let Some(n) = value.as_f64() else {
                return fail(format!("{where_} must be a number"));
            };
            if kind == "integer" && n.fract() != 0.0 {
                return fail(format!("{where_} must be an integer"));
            }
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
                && n < min
            {
                return fail(format!("{where_} must be at least {min}"));
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
                && n > max
            {
                fail(format!("{where_} must be at most {max}"));
            }
        }
        Some("boolean") if !value.is_boolean() => fail(format!("{where_} must be a boolean")),
        _ => {}
    }
}

/// `output`, `a.b[2]`, or `value` for the root, as the SDK words it.
fn describe(at: &Path) -> String {
    if at.is_empty() {
        return "value".into();
    }
    let mut out = String::new();
    for seg in at {
        match seg {
            PathSegment::Key(k) if out.is_empty() => out.push_str(k),
            PathSegment::Key(k) => {
                out.push('.');
                out.push_str(k);
            }
            PathSegment::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn route_params() -> Value {
        json!({
            "type": "object",
            "properties": {
                "output": { "type": "integer", "minimum": 0 },
                "input": { "type": "integer", "minimum": 0 },
                "mode": { "type": "string", "enum": ["cut", "mix"] },
            },
            "required": ["output", "input"],
            "additionalProperties": false,
        })
    }

    fn messages(value: Value) -> Vec<String> {
        check(&route_params(), &value)
            .into_iter()
            .map(|m| m.message)
            .collect()
    }

    #[test]
    fn matches_the_sdk_wording() {
        assert!(messages(json!({ "output": 1, "input": 3 })).is_empty());
        assert_eq!(messages(json!({ "output": 1 })), ["input is required"]);
        assert_eq!(
            messages(json!({ "output": -1, "input": 0 })),
            ["output must be at least 0"]
        );
        assert_eq!(
            messages(json!({ "output": 1.5, "input": 0 })),
            ["output must be an integer"]
        );
        assert_eq!(
            messages(json!({ "output": 1, "input": 0, "x": 1 })),
            ["x is not allowed"]
        );
        assert_eq!(
            messages(json!({ "output": 1, "input": 0, "mode": "wipe" })),
            ["mode must be one of cut, mix"]
        );
        assert_eq!(messages(json!("x")), ["value must be an object"]);
    }

    #[test]
    fn reports_every_problem_with_its_path() {
        let found = check(&route_params(), &json!({ "output": "1", "y": true }));
        let paths: Vec<Path> = found.into_iter().map(|m| m.path).collect();
        let key = |k: &str| vec![PathSegment::Key(k.into())];
        assert_eq!(paths, [key("input"), key("output"), key("y")]);
    }
}
