# Submodules

Every directory listed below is a **git submodule** — a *secondary* checkout
vendored into this superproject for build/deploy. The **source of truth for each
is its own upstream repository**, not the copy under `remote/` here.

> [!IMPORTANT]
> Make changes in the upstream repo (or its standalone clone on disk), then bump
> the submodule pointer here. Editing files directly inside the submodule
> checkout is easy to lose and bypasses the source repo's history/CI.

The "Source clone on disk" column is the standalone working copy under
`~/codes/…` where day-to-day development happens. The submodule checkout itself
always lives at `~/codes/ores/k8s-cluster/<submodule path>`.

| Submodule path | Upstream repo | Branch | Source clone on disk |
|---|---|---|---|
| `remote/deployments/gcs/chat-vibe` | [ORESoftware/chat.vibe](https://github.com/ORESoftware/chat.vibe) | `master` | — _(submodule checkout only)_ |
| `remote/modules/github/oresoftware/json-logging` | [oresoftware/json-logging](https://github.com/oresoftware/json-logging) | `master` | `~/codes/ores/json-logging` |
| `remote/modules/github/oresoftware/cmd-line-parser` | [oresoftware/cmd-line-parser](https://github.com/oresoftware/cmd-line-parser) | `main` | — _(submodule checkout only)_ |
| `remote/modules/github/oresoftware/go-iterators` | [oresoftware/go-iterators](https://github.com/oresoftware/go-iterators) | `main` | — _(submodule checkout only)_ |
| `remote/submodules/live-mutex` | [ORESoftware/live-mutex](https://github.com/ORESoftware/live-mutex) | `dev` | `~/codes/ores/live-mutex` |
| `remote/submodules/rust-network-mutex-rs` | [ORESoftware/live-mutex-rs](https://github.com/ORESoftware/live-mutex-rs) | `dev` | `~/codes/ores/live-mutex-rs` |
| `remote/submodules/discrete-event-system` | [ORESoftware/discrete-event-system](https://github.com/ORESoftware/discrete-event-system) | `main` | `~/codes/ores/des-engine` |
| `remote/submodules/discrete-event-system.rs` | [ORESoftware/discrete-event-system.rs](https://github.com/ORESoftware/discrete-event-system.rs) | `main` | `~/codes/ores/discrete-event-system.rs` |
| `remote/deployments/mip-solver-node.rs` | [ORESoftware/mip-solver-node.rs](https://github.com/ORESoftware/mip-solver-node.rs) | `main` | `~/codes/ores/mip-solver-node.rs` |
| `remote/submodules/live-mutex-mills.rs` | [ORESoftware/live-mutex-mills.rs](https://github.com/ORESoftware/live-mutex-mills.rs) | `main` | `~/codes/ores/live-mutex-mills.rs` |
| `remote/submodules/live-mutex.distributed` | [ORESoftware/live-mutex.distributed](https://github.com/ORESoftware/live-mutex.distributed) | `main` | `~/codes/ores/live-mutex.distributed` |
| `remote/submodules/soccer-sim-game-engine.rs` | [ORESoftware/soccer-sim-game-engine.rs](https://github.com/ORESoftware/soccer-sim-game-engine.rs) | `main` | `~/codes/ores/soccer-sim-game-engine.rs` |
| `remote/deployments/dd-sound-recorder-rs` | [sonus-auris/sonus-auris-backend.rs](https://github.com/sonus-auris/sonus-auris-backend.rs) | `main` | `~/codes/sonus-auris/sonus-auris-backend.rs` |
| `remote/deployments/3fa-backend` | [ORESoftware/3fa-backend.rs](https://github.com/ORESoftware/3fa-backend.rs) | `main` | `~/codes/3FA-app/3fa-backend.rs` |
| `remote/libs` | [ORESoftware/k8s-libs-and-shared-defs](https://github.com/ORESoftware/k8s-libs-and-shared-defs) | `main` | `~/codes/ores/k8s-libs-and-shared-defs` |
| `remote/deployments/benefactor-backend-rs` | [benefactor-cc/backend.rs](https://github.com/benefactor-cc/backend.rs) | `main` | — _(submodule checkout only)_ |
| `remote/submodules/sonus-auris-site.web` | [sonus-auris/sonus-auris-site.web](https://github.com/sonus-auris/sonus-auris-site.web) | `main` | `~/codes/sonus-auris/sonus-auris-site.web` |
| `remote/submodules/sonus-auris.infra` | [sonus-auris/sonus-auris.infra](https://github.com/sonus-auris/sonus-auris.infra) | `main` | — _(submodule checkout only)_ |
| `remote/deployments/soccer-rs` | [akrion-sim/akrion-backend.rs](https://github.com/akrion-sim/akrion-backend.rs) | `main` | `~/codes/akrion-sim/akrion-backend.rs` |
| `remote/deployments/akrion-web-server-rs` | [akrion-sim/akrion-web-server.rs](https://github.com/akrion-sim/akrion-web-server.rs) | `main` | `~/codes/akrion-sim/akrion-web-server.rs` |
| `remote/deployments/canonical-cloud` | [canonical-cloud/canonical.cloud](https://github.com/canonical-cloud/canonical.cloud) | `main` | `~/codes/canonical.cloud` |
| `remote/deployments/fiducia-backend.rs` | [fiducia-cloud/fiducia-backend.rs](https://github.com/fiducia-cloud/fiducia-backend.rs) | `main` | `~/codes/fiducia.cloud/fiducia-backend.rs` |
| `remote/deployments/fiducia-ui.web` | [fiducia-cloud/fiducia-ui.web](https://github.com/fiducia-cloud/fiducia-ui.web) | `main` | `~/codes/fiducia.cloud/fiducia-ui.web` |

---

_Maintained from `.gitmodules` and the on-disk clones. See also the
"Submodules are secondary" note in [AGENTS.md](AGENTS.md)._
