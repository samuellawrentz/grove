use serde_json::Value;

use crate::error::GroveError;

/// Print a success response. In JSON mode, wraps data with `{ ok: true, ...data }`.
/// In human mode, prints the human string.
pub fn success(json_mode: bool, human: &str, data: Value) {
    if json_mode {
        let mut obj = match data {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        obj.insert("ok".to_string(), Value::Bool(true));
        // Ensure "ok" is first by rebuilding
        let mut ordered = serde_json::Map::new();
        ordered.insert("ok".to_string(), Value::Bool(true));
        for (k, v) in obj {
            if k != "ok" {
                ordered.insert(k, v);
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(ordered)).expect("JSON serialization failed")
        );
    } else {
        println!("{human}");
    }
}

/// Print an error response. In JSON mode, prints the JSON error contract.
/// In human mode, prints "Error: {message}" to stderr.
pub fn error(json_mode: bool, err: &GroveError) {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&err.to_json()).expect("JSON serialization failed"));
    } else {
        eprintln!("Error: {err}");
    }
}
