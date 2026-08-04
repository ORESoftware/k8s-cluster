from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


path = Path("remote/deployments/build-server-rs/src/http.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "        git_http_auth_header: None,",
    "        git_credential_source: None,",
    "build-server HTTP fixture",
)
path.write_text(text, encoding="utf-8")

path = Path("remote/deployments/build-server-rs/src/config.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''fn validate_git_token_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(
            "BUILD_SERVER_GIT_TOKEN_FILE must be an absolute path without '..'".to_string(),
        );
    }
    Ok(())
}''',
    '''fn validate_git_token_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.to_string_lossy().len() > 4096
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(
            "BUILD_SERVER_GIT_TOKEN_FILE must be a bounded absolute path without '..'"
                .to_string(),
        );
    }
    Ok(())
}''',
    "bounded build-server token path",
)
text = replace_once(
    text,
    '''    fn token_files_must_be_absolute_and_tokens_single_line() {
        assert!(validate_git_token_path(Path::new("relative/token")).is_err());
        assert!(validate_git_token("ghs_token_with whitespace_123456").is_err());
        assert!(validate_git_token("short").is_err());
    }''',
    '''    fn token_files_must_be_bounded_absolute_and_tokens_single_line() {
        assert!(validate_git_token_path(Path::new("relative/token")).is_err());
        assert!(validate_git_token_path(Path::new("/safe/../token")).is_err());
        let oversized = PathBuf::from(format!("/{}", "a".repeat(4097)));
        assert!(validate_git_token_path(&oversized).is_err());
        assert!(validate_git_token("ghs_token_with whitespace_123456").is_err());
        assert!(validate_git_token("short").is_err());
    }''',
    "build-server token boundary test",
)
path.write_text(text, encoding="utf-8")
