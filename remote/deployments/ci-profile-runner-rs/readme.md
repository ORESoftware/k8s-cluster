# dd-ci-profile-runner

`dd-ci-profile-runner` is the narrow host-containerd execution boundary for fixed CI profiles that cannot safely be launched by the unprivileged `dd-build-server` pod.

It exists because a nested `nerdctl run` client inside `dd-build-server` cannot mount the host containerd overlay snapshot tree without `CAP_SYS_ADMIN` plus host containerd state mounts. Granting those capabilities to the general build server would collapse its current privilege separation.

## Contract

Authenticated `POST /run` accepts only `ci-profile-runner.v1`:

```json
{
  "schemaVersion": "ci-profile-runner.v1",
  "requestId": "gha:example",
  "repository": "discrete-event-systems-test/des-web-playwright-e2e",
  "revision": "1e1116ef6811c4e3e6be34ad3e1def39bc20ef59",
  "profile": "playwright"
}
```

Hard boundaries:

- `revision` must be an exact 40-hex commit SHA;
- `repository` must match an exact repository/profile binding in `CI_PROFILE_RUNNER_RULES_JSON`;
- only compiled `playwright` and `puppeteer` profiles exist;
- the caller cannot select a clone URL, runner image, shell, command, network, mount, resource limit, container name, or containerd namespace;
- Git clones disable `ext`, `file`, and `local` protocols, fetch no tags/submodules, and verify detached `HEAD` equals the requested SHA;
- runner containers use fixed CPU/memory/PID/shared-memory limits, `no-new-privileges`, and `cap-drop=ALL`;
- output is tail-bounded and work directories/containers are cleaned after every request;
- the HTTP service itself has no Kubernetes API token.

The privileged pod is intentional and isolated: it mounts the host containerd socket/root and nerdctl state exactly so `nerdctl` can perform snapshot mounts in the host mount namespace. `dd-build-server` remains unprivileged and delegates only the browser profiles to this service.

## DES bindings

The runtime manifest currently binds exactly:

- `discrete-event-systems-test/des-web-playwright-e2e` → `playwright`
- `discrete-event-systems-test/des-web-puppeteer-e2e` → `puppeteer`

Adding another repository is an explicit GitOps policy change.
