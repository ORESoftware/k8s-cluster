use std::{
    env, fmt, fs,
    path::{Component, Path, PathBuf},
};

const MAX_TOKEN_BYTES: usize = 4096;
const MIN_TOKEN_BYTES: usize = 20;

#[derive(Clone)]
pub enum TokenSource {
    Inline(String),
    File(PathBuf),
}

impl fmt::Debug for TokenSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(_) => formatter.write_str("Inline(<redacted>)"),
            Self::File(path) => formatter.debug_tuple("File").field(path).finish(),
        }
    }
}

impl TokenSource {
    pub fn from_env(inline_key: &str, file_key: &str) -> Result<Option<Self>, String> {
        Self::from_values(
            env::var(inline_key).ok(),
            env::var(file_key).ok(),
            inline_key,
            file_key,
        )
    }

    pub fn from_values(
        inline: Option<String>,
        file: Option<String>,
        inline_key: &str,
        file_key: &str,
    ) -> Result<Option<Self>, String> {
        let inline = normalized_optional(inline);
        let file = normalized_optional(file);
        match (inline, file) {
            (Some(_), Some(_)) => Err(format!(
                "configure exactly one of {inline_key} or {file_key}"
            )),
            (Some(token), None) => {
                validate_token(&token, inline_key)?;
                Ok(Some(Self::Inline(token)))
            }
            (None, Some(path)) => {
                let path = validate_file_path(&path, file_key)?;
                Ok(Some(Self::File(path)))
            }
            (None, None) => Ok(None),
        }
    }

    pub fn read(&self) -> Result<String, String> {
        match self {
            Self::Inline(token) => Ok(token.clone()),
            Self::File(path) => {
                let metadata = fs::metadata(path)
                    .map_err(|_| "GitHub token file is unavailable".to_string())?;
                if !metadata.is_file() {
                    return Err("GitHub token path is not a regular file".to_string());
                }
                if metadata.len() as usize > MAX_TOKEN_BYTES {
                    return Err("GitHub token file exceeds the byte limit".to_string());
                }
                let token = fs::read_to_string(path)
                    .map_err(|_| "GitHub token file could not be read".to_string())?;
                let token = token.trim().to_string();
                validate_token(&token, "GitHub token file")?;
                Ok(token)
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Inline(_) => "environment",
            Self::File(_) => "file",
        }
    }
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_file_path(value: &str, label: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute()
        || value.len() > 4096
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("{label} must be an absolute path without '..'"));
    }
    Ok(path.to_path_buf())
}

fn validate_token(token: &str, label: &str) -> Result<(), String> {
    if token.len() < MIN_TOKEN_BYTES || token.len() > MAX_TOKEN_BYTES {
        return Err(format!(
            "{label} must contain between {MIN_TOKEN_BYTES} and {MAX_TOKEN_BYTES} bytes"
        ));
    }
    if token.chars().any(char::is_whitespace) || token.chars().any(char::is_control) {
        return Err(format!("{label} must be a single printable token"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gha-clone-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[test]
    fn rejects_ambiguous_or_relative_configuration() {
        assert!(TokenSource::from_values(
            Some("ghs_inline_token_value_123456".into()),
            Some("/var/run/token".into()),
            "INLINE",
            "FILE",
        )
        .is_err());
        assert!(TokenSource::from_values(None, Some("relative/token".into()), "INLINE", "FILE")
            .is_err());
    }

    #[test]
    fn reloads_rotated_file_without_restarting() {
        let path = temporary_path("rotation");
        fs::write(&path, "ghs_first_installation_token_123456\n").expect("write first token");
        let source = TokenSource::from_values(
            None,
            Some(path.to_string_lossy().to_string()),
            "INLINE",
            "FILE",
        )
        .expect("token source")
        .expect("configured source");
        assert_eq!(source.kind(), "file");
        assert_eq!(source.read().expect("first read"), "ghs_first_installation_token_123456");

        fs::write(&path, "ghs_second_installation_token_654321\n")
            .expect("write rotated token");
        assert_eq!(
            source.read().expect("rotated read"),
            "ghs_second_installation_token_654321"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn redacts_inline_tokens_from_debug_output() {
        let source = TokenSource::Inline("ghs_secret_installation_token_123456".into());
        let rendered = format!("{source:?}");
        assert_eq!(rendered, "Inline(<redacted>)");
        assert!(!rendered.contains("ghs_secret"));
    }
}
