pub mod cloudflare;
pub mod docs;
pub mod domain;
pub mod github;
pub mod health;
pub mod k8s;

/// Render an error with its full source chain, e.g.
/// `error sending request: dns error: failed to lookup address`.
/// `std::fmt::Display` on reqwest errors alone hides the useful cause.
pub fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Outer(std::io::Error);

    impl std::fmt::Display for Outer {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "outer failed")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chain_includes_sources() {
        let error = Outer(std::io::Error::other("inner cause"));
        assert_eq!(error_chain(&error), "outer failed: inner cause");
    }
}
