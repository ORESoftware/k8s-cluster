//! CLI flags → env at startup, driven by `.cli-flags.toml` via flags-2-env
//! (github.com/oresoftware/flags-2-env).
//!
//! Applied before config is read, so `--flag`, its `AUTH_*` env var, and the
//! `.cli-flags.toml` default all work and flags win. Best-effort: if the native
//! flags2env library is not present (e.g. a minimal container that ships only the
//! binary), we log at debug and fall back to plain env vars — the service still
//! runs entirely from its environment.

/// Parse this process's argv into env-var overrides and apply them.
pub fn apply_cli_flags() {
    let f2e = match unsafe { flags2env::Flags2Env::load(None) } {
        Ok(f) => f,
        Err(cause) => {
            tracing::debug!(%cause, "flags2env native lib not loaded; using plain env");
            return;
        }
    };
    let argv: Vec<String> = std::env::args().collect();
    match f2e.parse(&argv, None) {
        Ok(overrides) => {
            for (key, value) in overrides {
                std::env::set_var(key, value);
            }
        }
        Err(cause) => tracing::warn!(%cause, "flags2env parse failed; using plain env"),
    }
}
