//! Host-routing security unit tests (no DB, no network -- runs in the default
//! `cargo test`). B2B chrome, auth redirects, and biz-only authorization hinge
//! on `Config::is_biz_host`, which must key off the *configured* canonical B2B
//! origin and never a `biz.`-looking Host supplied by the requester. Paired with
//! `host_allowed` (the `ALLOWED_HOSTS` gate).
use athleto_app_rs::Config;

fn config(biz: &str, allowed: Option<&[&str]>) -> Config {
    Config {
        biz_public_base_url: biz.to_string(),
        allowed_hosts: allowed.map(|hosts| hosts.iter().map(|h| h.to_string()).collect()),
        ..Config::default()
    }
}

#[test]
fn host_allowed_is_permissive_when_unset_and_strict_when_set() {
    let open = config("https://biz.athleto.store", None);
    assert!(
        open.host_allowed("anything.example.com"),
        "unset allowlist is permissive"
    );

    let strict = config(
        "https://biz.athleto.store",
        Some(&["app.athleto.store", "biz.athleto.store"]),
    );
    assert!(strict.host_allowed("app.athleto.store"));
    assert!(
        strict.host_allowed("biz.athleto.store:8145"),
        "port is ignored"
    );
    assert!(
        !strict.host_allowed("evil.example.com"),
        "unlisted host rejected"
    );
}

#[test]
fn is_biz_host_matches_only_the_configured_origin() {
    let c = config("https://biz.athleto.store", None);
    assert!(
        c.is_biz_host("biz.athleto.store"),
        "the configured biz origin"
    );
    assert!(c.is_biz_host("biz.athleto.store:8145"), "port is ignored");
    assert!(
        !c.is_biz_host("app.athleto.store"),
        "the consumer host is not biz"
    );
}

#[test]
fn is_biz_host_rejects_spoofed_biz_lookalikes() {
    let c = config("https://biz.athleto.store", None);
    // A requester cannot promote themselves to the B2B chrome by sending a
    // Host that merely *contains* or *extends* the biz origin.
    for spoof in [
        "biz.athleto.store.attacker.com",
        "evilbiz.athleto.store",
        "biz.athleto.store.evil",
        "notbiz.athleto.store",
        "xbiz.athleto.store",
    ] {
        assert!(
            !c.is_biz_host(spoof),
            "{spoof} must not count as the biz host"
        );
    }
}

#[test]
fn is_biz_host_requires_the_host_to_be_allowed_too() {
    // Even the genuine biz origin is rejected when it is not in a configured
    // ALLOWED_HOSTS list: is_biz_host is (host_allowed AND exact-origin-match).
    let c = config("https://biz.athleto.store", Some(&["app.athleto.store"]));
    assert!(
        !c.is_biz_host("biz.athleto.store"),
        "biz host absent from the allowlist is refused"
    );

    let ok = config("https://biz.athleto.store", Some(&["biz.athleto.store"]));
    assert!(
        ok.is_biz_host("biz.athleto.store"),
        "allowed + configured => biz"
    );
}
