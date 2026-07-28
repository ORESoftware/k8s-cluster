# DEN-483 materialization failure

Workflow run: 30361917603
Source commit: dde17d0670001dc115e01019efcb07584a171c62
Exit status: 101

## Last 320 log lines
```text
[1m[92m  Downloaded[0m pin-project v1.1.13
[1m[92m  Downloaded[0m opentelemetry-semantic-conventions v0.16.0
[1m[92m  Downloaded[0m opentelemetry-otlp v0.26.0
[1m[92m  Downloaded[0m getrandom v0.2.17
[1m[92m  Downloaded[0m regex-syntax v0.8.10
[1m[92m  Downloaded[0m log v0.4.29
[1m[92m  Downloaded[0m tinystr v0.8.3
[1m[92m  Downloaded[0m serde_json v1.0.149
[1m[92m  Downloaded[0m rustls-webpki v0.103.13
[1m[92m  Downloaded[0m opentelemetry v0.26.0
[1m[92m  Downloaded[0m url v2.5.8
[1m[92m  Downloaded[0m tracing-subscriber v0.3.23
[1m[92m  Downloaded[0m memchr v2.8.0
[1m[92m  Downloaded[0m httparse v1.10.1
[1m[92m  Downloaded[0m bitflags v2.11.1
[1m[92m  Downloaded[0m untrusted v0.9.0
[1m[92m  Downloaded[0m tonic v0.12.3
[1m[92m  Downloaded[0m nu-ansi-term v0.50.3
[1m[92m  Downloaded[0m litemap v0.8.2
[1m[92m  Downloaded[0m http-body-util v0.1.3
[1m[92m  Downloaded[0m find-msvc-tools v0.1.9
[1m[92m  Downloaded[0m prost v0.13.5
[1m[92m  Downloaded[0m ipnet v2.12.0
[1m[92m  Downloaded[0m slab v0.4.12
[1m[92m  Downloaded[0m rustls-native-certs v0.8.4
[1m[92m  Downloaded[0m rand_core v0.6.4
[1m[92m  Downloaded[0m rand_chacha v0.9.0
[1m[92m  Downloaded[0m prost-derive v0.13.5
[1m[92m  Downloaded[0m ppv-lite86 v0.2.21
[1m[92m  Downloaded[0m pin-project-internal v1.1.13
[1m[92m  Downloaded[0m block-buffer v0.10.4
[1m[92m  Downloaded[0m utoipa-axum v0.2.0
[1m[92m  Downloaded[0m socket2 v0.6.3
[1m[92m  Downloaded[0m once_cell v1.21.4
[1m[92m  Downloaded[0m getrandom v0.3.4
[1m[92m  Downloaded[0m displaydoc v0.2.5
[1m[92m  Downloaded[0m quote v1.0.45
[1m[92m  Downloaded[0m icu_normalizer_data v2.2.0
[1m[92m  Downloaded[0m hyper-rustls v0.27.9
[1m[92m  Downloaded[0m fastrand v2.4.1
[1m[92m  Downloaded[0m mime v0.3.17
[1m[92m  Downloaded[0m tokio-macros v2.7.0
[1m[92m  Downloaded[0m potential_utf v0.1.5
[1m[92m  Downloaded[0m lazy_static v1.5.0
[1m[92m  Downloaded[0m serde_path_to_error v0.1.20
[1m[92m  Downloaded[0m openssl-probe v0.2.1
[1m[92m  Downloaded[0m futures-macro v0.3.32
[1m[92m  Downloaded[0m rustc-hash v2.1.2
[1m[92m  Downloaded[0m icu_normalizer v2.2.0
[1m[92m  Downloaded[0m httpdate v1.0.3
[1m[92m  Downloaded[0m futures-task v0.3.32
[1m[92m  Downloaded[0m signal-hook-registry v1.4.8
[1m[92m  Downloaded[0m rustls-pki-types v1.14.1
[1m[92m  Downloaded[0m matchit v0.8.4
[1m[92m  Downloaded[0m matchers v0.2.0
[1m[92m  Downloaded[0m icu_provider v2.2.0
[1m[92m  Downloaded[0m try-lock v0.2.5
[1m[92m  Downloaded[0m tinyvec v1.11.0
[1m[92m  Downloaded[0m rand_chacha v0.3.1
[1m[92m  Downloaded[0m lru-slab v0.1.2
[1m[92m  Downloaded[0m futures-sink v0.3.32
[1m[92m  Downloaded[0m webpki-roots v0.26.11
[1m[92m  Downloaded[0m tokio-rustls v0.26.4
[1m[92m  Downloaded[0m serde_core v1.0.228
[1m[92m  Downloaded[0m ryu v1.0.23
[1m[92m  Downloaded[0m quinn-udp v0.5.14
[1m[92m  Downloaded[0m digest v0.10.7
[1m[92m  Downloaded[0m tower-layer v0.3.3
[1m[92m  Downloaded[0m thiserror-impl v1.0.69
[1m[92m  Downloaded[0m thiserror v2.0.18
[1m[92m  Downloaded[0m thiserror v1.0.69
[1m[92m  Downloaded[0m regex-automata v0.4.14
[1m[92m  Downloaded[0m percent-encoding v2.3.2
[1m[92m  Downloaded[0m paste v1.0.15
[1m[92m  Downloaded[0m opentelemetry-http v0.26.0
[1m[92m  Downloaded[0m generic-array v0.14.7
[1m[92m  Downloaded[0m errno v0.3.14
[1m[92m  Downloaded[0m base64 v0.22.1
[1m[92m  Downloaded[0m proc-macro2 v1.0.106
[1m[92m  Downloaded[0m icu_properties_data v2.2.0
[1m[92m  Downloaded[0m futures-io v0.3.32
[1m[92m  Downloaded[0m futures-executor v0.3.32
[1m[92m  Downloaded[0m equivalent v1.0.2
[1m[92m  Downloaded[0m hex v0.4.3
[1m[92m  Downloaded[0m axum v0.8.9
[1m[92m  Downloaded[0m serde_urlencoded v0.7.1
[1m[92m  Downloaded[0m icu_locale_core v2.2.0
[1m[92m  Downloaded[0m async-trait v0.1.89
[1m[92m  Downloaded[0m http-body v1.0.1
[1m[92m  Downloaded[0m glob v0.3.3
[1m[92m  Downloaded[0m anyhow v1.0.102
[1m[92m  Downloaded[0m rand_core v0.9.5
[1m[92m  Downloaded[0m axum-macros v0.5.1
[1m[92m  Downloaded[0m utf8_iter v1.0.4
[1m[92m  Downloaded[0m futures-core v0.3.32
[1m[92m  Downloaded[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m proc-macro2 v1.0.106
[1m[92m   Compiling[0m unicode-ident v1.0.24
[1m[92m   Compiling[0m quote v1.0.45
[1m[92m   Compiling[0m libc v0.2.186
[1m[92m    Checking[0m itoa v1.0.18
[1m[92m    Checking[0m pin-project-lite v0.2.17
[1m[92m    Checking[0m once_cell v1.21.4
[1m[92m    Checking[0m futures-core v0.3.32
[1m[92m    Checking[0m bytes v1.11.1
[1m[92m    Checking[0m cfg-if v1.0.4
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m    Checking[0m memchr v2.8.0
[1m[92m    Checking[0m errno v0.3.14
[1m[92m    Checking[0m tracing-core v0.1.36
[1m[92m    Checking[0m signal-hook-registry v1.4.8
[1m[92m    Checking[0m mio v1.2.0
[1m[92m    Checking[0m socket2 v0.6.3
[1m[92m    Checking[0m stable_deref_trait v1.2.1
[1m[92m    Checking[0m futures-sink v0.3.32
[1m[92m    Checking[0m log v0.4.29
[1m[92m   Compiling[0m zmij v1.0.21
[1m[92m    Checking[0m getrandom v0.2.17
[1m[92m    Checking[0m http v1.4.0
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m    Checking[0m smallvec v1.15.1
[1m[92m    Checking[0m http-body v1.0.1
[1m[92m    Checking[0m futures-io v0.3.32
[1m[92m    Checking[0m slab v0.4.12
[1m[92m    Checking[0m futures-task v0.3.32
[1m[92m    Checking[0m tower-service v0.3.3
[1m[92m   Compiling[0m find-msvc-tools v0.1.9
[1m[92m    Checking[0m zeroize v1.8.2
[1m[92m    Checking[0m percent-encoding v2.3.2
[1m[92m   Compiling[0m shlex v1.3.0
[1m[92m    Checking[0m rustls-pki-types v1.14.1
[1m[92m   Compiling[0m cc v1.2.62
[1m[92m    Checking[0m futures-channel v0.3.32
[1m[92m   Compiling[0m httparse v1.10.1
[1m[92m    Checking[0m writeable v0.6.3
[1m[92m    Checking[0m litemap v0.8.2
[1m[92m    Checking[0m http-body-util v0.1.3
[1m[92m   Compiling[0m icu_normalizer_data v2.2.0
[1m[92m    Checking[0m subtle v2.6.1
[1m[92m    Checking[0m try-lock v0.2.5
[1m[92m    Checking[0m utf8_iter v1.0.4
[1m[92m   Compiling[0m icu_properties_data v2.2.0
[1m[92m    Checking[0m want v0.3.1
[1m[92m    Checking[0m atomic-waker v1.1.2
[1m[92m    Checking[0m tower-layer v0.3.3
[1m[92m    Checking[0m untrusted v0.9.0
[1m[92m    Checking[0m httpdate v1.0.3
[1m[92m   Compiling[0m zerocopy v0.8.48
[1m[92m    Checking[0m form_urlencoded v1.2.2
[1m[92m    Checking[0m sync_wrapper v1.0.2
[1m[92m   Compiling[0m thiserror v1.0.69
[1m[92m   Compiling[0m anyhow v1.0.102
[1m[92m   Compiling[0m ring v0.17.14
[1m[92m   Compiling[0m rustls v0.23.40
[1m[92m   Compiling[0m synstructure v0.13.2
[1m[92m   Compiling[0m aho-corasick v1.1.4
[1m[92m   Compiling[0m zerofrom-derive v0.1.7
[1m[92m   Compiling[0m tokio-macros v2.7.0
[1m[92m    Checking[0m zerofrom v0.1.8
[1m[92m   Compiling[0m yoke-derive v0.8.2
[1m[92m    Checking[0m tokio v1.52.3
[1m[92m    Checking[0m yoke v0.8.2
[1m[92m   Compiling[0m tracing-attributes v0.1.31
[1m[92m   Compiling[0m zerovec-derive v0.11.3
[1m[92m    Checking[0m tracing v0.1.44
[1m[92m    Checking[0m zerovec v0.11.6
[1m[92m   Compiling[0m displaydoc v0.2.5
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m futures-macro v0.3.32
[1m[92m    Checking[0m futures-util v0.3.32
[1m[92m    Checking[0m tinystr v0.8.3
[1m[92m    Checking[0m icu_locale_core v2.2.0
[1m[92m    Checking[0m potential_utf v0.1.5
[1m[92m    Checking[0m zerotrie v0.2.4
[1m[92m    Checking[0m icu_provider v2.2.0
[1m[92m    Checking[0m icu_collections v2.2.0
[1m[92m    Checking[0m hyper v1.9.0
[1m[92m    Checking[0m icu_properties v2.2.0
[1m[92m    Checking[0m icu_normalizer v2.2.0
[1m[92m    Checking[0m hyper-util v0.1.20
[1m[92m   Compiling[0m thiserror-impl v1.0.69
[1m[92m   Compiling[0m version_check v0.9.5
[1m[92m    Checking[0m ryu v1.0.23
[1m[92m    Checking[0m mime v0.3.17
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m    Checking[0m serde_urlencoded v0.7.1
[1m[92m   Compiling[0m generic-array v0.14.7
[1m[92m    Checking[0m idna_adapter v1.2.2
[1m[92m    Checking[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m async-trait v0.1.89
[1m[92m    Checking[0m webpki-roots v1.0.7
[1m[92m    Checking[0m rand_core v0.6.4
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m    Checking[0m base64 v0.22.1
[1m[92m   Compiling[0m either v1.16.0
[1m[92m    Checking[0m openssl-probe v0.2.1
[1m[92m   Compiling[0m itertools v0.14.0
[1m[92m    Checking[0m rustls-native-certs v0.8.4
[1m[92m   Compiling[0m uuid v1.23.1
[1m[92m    Checking[0m rand_chacha v0.3.1
[1m[92m    Checking[0m opentelemetry v0.26.0
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m    Checking[0m idna v1.1.0
[1m[92m    Checking[0m tokio-stream v0.1.18
[1m[92m    Checking[0m rustls-webpki v0.103.13
[1m[92m    Checking[0m equivalent v1.0.2
[1m[92m    Checking[0m hashbrown v0.17.1
[1m[92m    Checking[0m typenum v1.20.0
[1m[92m   Compiling[0m utoipa-gen v5.5.0
[1m[92m    Checking[0m indexmap v2.14.0
[1m[92m   Compiling[0m prost-derive v0.13.5
[1m[92m    Checking[0m url v2.5.8
[1m[92m    Checking[0m rand v0.8.6
[1m[92m    Checking[0m webpki-roots v0.26.11
[1m[92m    Checking[0m futures-executor v0.3.32
[1m[92m   Compiling[0m pin-project-internal v1.1.13
[1m[92m    Checking[0m rustls-pemfile v2.2.0
[1m[92m    Checking[0m tokio-rustls v0.26.4
[1m[92m    Checking[0m glob v0.3.3
[1m[92m    Checking[0m hyper-rustls v0.27.9
[1m[92m    Checking[0m ipnet v2.12.0
[1m[92m    Checking[0m reqwest v0.12.9
[1m[92m    Checking[0m pin-project v1.1.13
[1m[92m    Checking[0m opentelemetry_sdk v0.26.0
[1m[92m    Checking[0m prost v0.13.5
[1m[92m   Compiling[0m paste v1.0.15
[1m[92m    Checking[0m lazy_static v1.5.0
[1m[92m   Compiling[0m getrandom v0.4.2
[1m[92m    Checking[0m bitflags v2.11.1
[1m[92m    Checking[0m sharded-slab v0.1.7
[1m[92m    Checking[0m tonic v0.12.3
[1m[92m    Checking[0m crypto-common v0.1.7
[1m[92m    Checking[0m block-buffer v0.10.4
[1m[92m    Checking[0m axum-core v0.5.6
[1m[92m    Checking[0m tower v0.5.3
[1m[92m    Checking[0m matchers v0.2.0
[1m[92m    Checking[0m tracing-serde v0.2.0
[1m[92m   Compiling[0m axum-macros v0.5.1
[1m[92m    Checking[0m serde_path_to_error v0.1.20
[1m[92m    Checking[0m tracing-log v0.2.0
[1m[92m    Checking[0m thread_local v1.1.9
[1m[92m    Checking[0m nu-ansi-term v0.50.3
[1m[92m   Compiling[0m rustix v1.1.4
[1m[92m    Checking[0m matchit v0.8.4
[1m[92m    Checking[0m tracing-subscriber v0.3.23
[1m[92m    Checking[0m opentelemetry-proto v0.26.1
[1m[92m    Checking[0m utoipa v5.5.0
[1m[92m    Checking[0m digest v0.10.7
[1m[92m    Checking[0m axum v0.8.9
[1m[92m    Checking[0m opentelemetry-http v0.26.0
[1m[92m    Checking[0m linux-raw-sys v0.12.1
[1m[92m    Checking[0m opentelemetry-otlp v0.26.0
[1m[92m    Checking[0m tracing-opentelemetry v0.27.0
[1m[92m    Checking[0m tower-http v0.6.10
[1m[92m    Checking[0m cpufeatures v0.2.17
[1m[92m    Checking[0m opentelemetry-semantic-conventions v0.16.0
[1m[92m    Checking[0m dd-shared-interfaces v0.1.0 (/home/runner/work/k8s-cluster/k8s-cluster/remote/libs/interfaces/shared/generated/rust)
[1m[92m    Checking[0m fastrand v2.4.1
[1m[92m    Checking[0m dd-telemetry v0.1.0 (/home/runner/work/k8s-cluster/k8s-cluster/remote/libs/telemetry-rs)
[1m[92m    Checking[0m utoipa-scalar v0.3.0
[1m[92m    Checking[0m sha2 v0.10.9
[1m[92m    Checking[0m hmac v0.12.1
[1m[92m    Checking[0m hex v0.4.3
[1m[92m    Checking[0m tempfile v3.27.0
[1m[92m    Checking[0m utoipa-axum v0.2.0
[1m[92m    Checking[0m dd-runtime-config-client v0.1.0 (/home/runner/work/k8s-cluster/k8s-cluster/remote/libs/runtime-config-client-rs)
[1m[92m    Checking[0m dd-formal-methods-service v0.1.0 (/home/runner/work/k8s-cluster/k8s-cluster/remote/deployments/formal-methods-service-rs)
[1m[91merror[E0277][0m[1m: the trait bound `&str: Spec` is not satisfied[0m
   [1m[94m--> [0msrc/docs.rs:49:57
    [1m[94m|[0m
[1m[94m 49[0m [1m[94m|[0m             public_scalar_html: Bytes::from(Scalar::new("/openapi.json").to_html()),
    [1m[94m|[0m                                             [1m[94m-----------[0m [1m[91m^^^^^^^^^^^^^^^[0m [1m[91mthe trait `Spec` is not implemented for `&str`[0m
    [1m[94m|[0m                                             [1m[94m|[0m
    [1m[94m|[0m                                             [1m[94mrequired by a bound introduced by this call[0m
    [1m[94m|[0m
[1m[96mhelp[0m: the trait `Spec` [1m[35mis[0m implemented for `[1m[35mutoipa::openapi::OpenApi[0m`
   [1m[94m--> [0m/home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/utoipa-scalar-0.3.0/src/lib.rs:285:1
    [1m[94m|[0m
[1m[94m285[0m [1m[94m|[0m impl Spec for OpenApi {}
    [1m[94m|[0m [1m[96m^^^^^^^^^^^^^^^^^^^^^[0m
[1m[92mnote[0m: required by a bound in `Scalar::<S>::new`
   [1m[94m--> [0m/home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/utoipa-scalar-0.3.0/src/lib.rs:210:9
    [1m[94m|[0m
[1m[94m210[0m [1m[94m|[0m impl<S: Spec> Scalar<S> {
    [1m[94m|[0m         [1m[92m^^^^[0m [1m[92mrequired by this bound in `Scalar::<S>::new`[0m
[1m[94m...[0m
[1m[94m221[0m [1m[94m|[0m     pub fn new(openapi: S) -> Self {
    [1m[94m|[0m            [1m[94m---[0m [1m[94mrequired by a bound in this associated function[0m

[1m[91merror[E0599][0m[1m: the method `to_html` exists for struct `Scalar<&str>`, but its trait bounds were not satisfied[0m
  [1m[94m--> [0msrc/docs.rs:49:74
   [1m[94m|[0m
[1m[94m49[0m [1m[94m|[0m             public_scalar_html: Bytes::from(Scalar::new("/openapi.json").to_html()),
   [1m[94m|[0m                                                                          [1m[91m^^^^^^^[0m [1m[91mmethod cannot be called on `Scalar<&str>` due to unsatisfied trait bounds[0m
   [1m[94m|[0m
   [1m[94m= [0m[1mnote[0m: the following trait bounds were not satisfied:
           `&str: Spec`

[1m[91merror[E0277][0m[1m: the trait bound `&str: Spec` is not satisfied[0m
   [1m[94m--> [0msrc/docs.rs:49:45
    [1m[94m|[0m
[1m[94m 49[0m [1m[94m|[0m             public_scalar_html: Bytes::from(Scalar::new("/openapi.json").to_html()),
    [1m[94m|[0m                                             [1m[91m^^^^^^^^^^^^^^^^^^^^^^^^^^^^[0m [1m[91mthe trait `Spec` is not implemented for `&str`[0m
    [1m[94m|[0m
[1m[96mhelp[0m: the trait `Spec` [1m[35mis[0m implemented for `[1m[35mutoipa::openapi::OpenApi[0m`
   [1m[94m--> [0m/home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/utoipa-scalar-0.3.0/src/lib.rs:285:1
    [1m[94m|[0m
[1m[94m285[0m [1m[94m|[0m impl Spec for OpenApi {}
    [1m[94m|[0m [1m[96m^^^^^^^^^^^^^^^^^^^^^[0m
[1m[92mnote[0m: required by a bound in `Scalar`
   [1m[94m--> [0m/home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/utoipa-scalar-0.3.0/src/lib.rs:203:22
    [1m[94m|[0m
[1m[94m203[0m [1m[94m|[0m pub struct Scalar<S: Spec> {
    [1m[94m|[0m                      [1m[92m^^^^[0m [1m[92mrequired by this bound in `Scalar`[0m

[1mSome errors have detailed explanations: E0277, E0599.[0m
[1mFor more information about an error, try `rustc --explain E0277`.[0m
[1m[91merror[0m: could not compile `dd-formal-methods-service` (lib) due to 3 previous errors
```
