<!-- BEGIN do-not-work-here-notice -->
> [!CAUTION]
> **Do not do development in this checkout. Use `~/codes/canonical-cloud/` instead.**
>
> This repo is a *mirror*. The `canonical-interfaces/`,
> `canonical-marketing-site.web/`, `canonical-web-server.rs/` and
> `canonical-monorepo/` directories inside it are **not** git repositories and
> **not** submodules — they are plain copies of those repos' files, committed
> here as ordinary tracked files (see the `Mirror …` commits in `git log`).
> Editing them here does not touch the real repos, cannot be pushed to them,
> and silently diverges from upstream.
>
> Real source of truth, one clone per repo:
>
> ```
> ~/codes/canonical-cloud/canonical-monorepo/          <- superproject; deploy from here
> ~/codes/canonical-cloud/canonical-web-server.rs/
> ~/codes/canonical-cloud/canonical-marketing-site.web/
> ~/codes/canonical-cloud/canonical-interfaces/
> ~/codes/canonical-cloud/canonical-mcp-server.rs/
> ~/codes/canonical-cloud/canonical.cloud/             <- this repo (mirror only)
> ```
>
> `canonical-monorepo` is the one that wires the apps together *properly*, via
> real submodule gitlinks under `apps/`. `canonical-mcp-server.rs/` is the only
> genuine submodule in this repo.
>
> Note the two confusingly similar names: the **directory**
> `~/codes/canonical-cloud/` (a plain folder holding all the clones) vs. the
> **repo** `canonical.cloud` (this one, nested inside it).
<!-- END do-not-work-here-notice -->

<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/canonical-cloud` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/canonical-cloud/canonical.cloud` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/canonical-cloud`.
<!-- END k8s-cluster-submodule-notice -->