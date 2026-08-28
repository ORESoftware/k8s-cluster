use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::identifiers::{validate_base_job_id, validate_instance_id};

const MAX_DEPENDENCIES: usize = 1_024;
const MAX_MATRIX_KEYS: usize = 32;
const MAX_MATRIX_JSON_BYTES: usize = 16 * 1_024;
const MAX_MATRIX_STRING_BYTES: usize = 512;

pub(super) fn validate_dependencies(
    job_instance_id: &str,
    dependencies: &[String],
) -> Result<(), String> {
    if dependencies.len() > MAX_DEPENDENCIES {
        return Err(format!(
            "needsInstances may contain at most {MAX_DEPENDENCIES} entries"
        ));
    }
    let mut unique = BTreeSet::new();
    for dependency in dependencies {
        validate_instance_id("needsInstances entry", dependency)?;
        if dependency == job_instance_id {
            return Err("needsInstances must not contain the current job".to_string());
        }
        if !unique.insert(dependency.as_str()) {
            return Err(format!(
                "needsInstances repeats concrete dependency {dependency:?}"
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_matrix(matrix: &BTreeMap<String, Value>) -> Result<(), String> {
    if matrix.len() > MAX_MATRIX_KEYS {
        return Err(format!("matrix may contain at most {MAX_MATRIX_KEYS} keys"));
    }
    for (key, value) in matrix {
        validate_base_job_id("matrix key", key)?;
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
            Value::String(value) => {
                if value.len() > MAX_MATRIX_STRING_BYTES || value.chars().any(char::is_control) {
                    return Err(format!(
                        "matrix value for {key:?} must be printable and at most {MAX_MATRIX_STRING_BYTES} bytes"
                    ));
                }
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(format!("matrix value for {key:?} must be a scalar"));
            }
        }
    }
    let encoded = serde_json::to_vec(matrix)
        .map_err(|error| format!("failed to size matrix metadata: {error}"))?;
    if encoded.len() > MAX_MATRIX_JSON_BYTES {
        return Err(format!(
            "matrix metadata is {} bytes; maximum is {MAX_MATRIX_JSON_BYTES}",
            encoded.len()
        ));
    }
    Ok(())
}
