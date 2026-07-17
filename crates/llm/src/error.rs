#[derive(Debug)]
pub enum LlmError {
    /// The provider was requested but its API key env var is not set.
    MissingApiKey(&'static str),
    Http(reqwest::Error),
    /// Non-2xx from the provider; body truncated for logs.
    Api {
        provider: &'static str,
        status: u16,
        body: String,
    },
    /// 2xx but the response JSON didn't have the expected shape.
    Parse {
        provider: &'static str,
        detail: String,
    },
    UnknownProvider(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::MissingApiKey(var) => write!(f, "missing API key env var {var}"),
            LlmError::Http(e) => write!(f, "http error: {e}"),
            LlmError::Api {
                provider,
                status,
                body,
            } => write!(f, "{provider} API returned {status}: {body}"),
            LlmError::Parse { provider, detail } => {
                write!(f, "unexpected {provider} response shape: {detail}")
            }
            LlmError::UnknownProvider(p) => {
                write!(
                    f,
                    "unknown provider '{p}' (expected openai|gemini|anthropic)"
                )
            }
        }
    }
}

impl std::error::Error for LlmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LlmError::Http(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        LlmError::Http(e)
    }
}
