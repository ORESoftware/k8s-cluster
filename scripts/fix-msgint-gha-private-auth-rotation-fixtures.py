from pathlib import Path

path = Path("remote/deployments/build-server-rs/src/http.rs")
text = path.read_text(encoding="utf-8")
old = "        git_http_auth_header: None,"
new = "        git_credential_source: None,"
count = text.count(old)
if count != 1:
    raise RuntimeError(f"build-server HTTP fixture: expected one match, found {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
