from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


path = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "pub fn build_plan(\n",
    '''fn ambiguous_yaml_reason(root: &Value) -> Option<&'static str> {
    // Walk iteratively so a bounded but deeply nested workflow cannot consume
    // the Rust call stack while the compiler enforces its parser boundary.
    let mut pending = vec![root];
    while let Some(value) = pending.pop() {
        match value {
            Value::Tagged(_) => {
                return Some("workflowYaml must not use YAML tags");
            }
            Value::Mapping(mapping) => {
                for (key, child) in mapping {
                    if key.as_str() == Some("<<") {
                        return Some("workflowYaml must not use YAML merge keys");
                    }
                    pending.push(key);
                    pending.push(child);
                }
            }
            Value::Sequence(sequence) => pending.extend(sequence),
            _ => {}
        }
    }
    None
}

pub fn build_plan(
''',
    "ambiguity guard helper",
)

text = replace_once(
    text,
    '''    let workflow: Value = serde_yaml::from_str(&request.workflow_yaml)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    let root = workflow
''',
    '''    let workflow: Value = serde_yaml::from_str(&request.workflow_yaml)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    if let Some(reason) = ambiguous_yaml_reason(&workflow) {
        return Err(vec![reason.to_string()]);
    }
    let root = workflow
''',
    "ambiguity guard invocation",
)

path.write_text(text, encoding="utf-8")
