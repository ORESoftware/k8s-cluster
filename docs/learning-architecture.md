# Soccer self-play learning architecture

How the engine learns: from source build, through the self-play loop and the
per-tick decision stack, into the learners, the Postgres-backed policy store, and
finally the live inference server. Grounded in the running continuous learner
(`main_soccer_learning_run`, generation ~362) and its live config.

## End-to-end flow

```mermaid
flowchart TB
    subgraph SRC["Source &amp; build (GitOps)"]
        GH["GitHub: soccer-sim-game-engine.rs @ main<br/>+ discrete-event-system.rs (des_engine)"]
        CW["commit-watcher pod<br/>(rebuilds main from feature/* branches)"]
        BUILD["learner entrypoint:<br/>git clone soccer + des -&gt; cargo build --release<br/>main_soccer_learning_run"]
        GH --> CW
        CW --> GH
        GH --> BUILD
    end

    subgraph LOOP["Self-play learning loop — per cycle, parallel_games=1"]
        CUR["Curriculum stage<br/>locomotion -&gt; ball-skills -&gt; duels -&gt; small-sided -&gt; team-shape -&gt; full-match"]
        OPP["Adversarial / self-play opponent<br/>(resumed policy, adversarial embedding)"]
        GAME["Self-play match (home policy vs away policy)"]
        REW["Per-step reward<br/>events: goals, completed passes, shots, chains<br/>+ tactical-learning shaping<br/>+ optional pitch-control x xT (gated)"]
        CUR --> GAME
        OPP --> GAME
        GAME --> REW
    end

    subgraph TICK["Per-tick decision stack (each player)"]
        POMDP["POMDP belief<br/>press intent / pass-trajectory anticipation"]
        WM["World model<br/>learned forward dynamics (receding horizon)"]
        LPF["Formation LP / sparse IPM<br/>team shape guidance"]
        POL["Policy head — ACTOR<br/>SoccerPolicyHead + role embedding"]
        PUCT["PUCT re-ranker<br/>shallow lookahead (off by default)"]
        MPC["Per-player MPC<br/>trajectory execution + feasibility veto"]
        ACT["Action: pass / shoot / dribble / move / tackle"]
        POMDP --> POL
        WM --> POL
        LPF --> POL
        POL --> PUCT
        PUCT --> ACT
        ACT --> MPC
    end

    GAME --> TICK
    TICK --> REW

    subgraph LEARN["Update / optimization"]
        NEU["Neural learner (threaded)<br/>actor-critic + GAE advantage, PPO clip (multi-epoch)<br/>replay 2048, batch 32, lr 0.015, hidden 24"]
        GA["Evolution / GA self-play<br/>every 5 games: mutate / crossover / elite (pop 16)"]
        GATE["Policy promotion gate<br/>min fitness, max conceded, play quality"]
    end

    subgraph AUX["Auxiliary learned models"]
        VAL["Value / critic head"]
        PASS["Pass-completion head<br/>(TRAINED, not yet consumed live)"]
        LINE["Line-depth heads — back four + midfield<br/>(merged, GATED off, corpus not yet wired)"]
        CFG["Config-similarity retrieval<br/>(HNSW over 22+ball embedding)"]
    end

    REW --> NEU
    GAME --> GA
    NEU --> GATE
    GA --> GATE
    REW -.-> VAL
    GAME -.-> PASS
    GAME -.-> LINE
    GAME -.-> CFG

    subgraph PGDB["Persistence — Postgres / RDS (ddremote)"]
        PV["policy_entries / policy_versions<br/>(active generation N)"]
        NN["neural-network snapshots<br/>(SoccerNeuralNetworkSnapshot)"]
        CORP["corpora: pass-outcome, tactical learning"]
        KPI["completed runs / KPIs<br/>(des_soccer_learning_runs)"]
    end

    GATE -->|promote| PV
    NEU --> NN
    PASS --> CORP
    GAME --> KPI
    PV -->|resume policy each game| OPP
    NN -->|resume network| BUILD

    subgraph SERVE["Serving / inference"]
        LIVE["live server (dd-soccer-rs / main_soccer_live)<br/>loads active gen from PG"]
        API["REST: /api/step, /api/inspect, shared-memory inspector"]
        LIVE --> API
    end

    PV -->|active generation| LIVE
    NN -->|active network| LIVE
```

## Deployment topology

```mermaid
flowchart LR
    subgraph AWS["AWS EC2 k8s (kubeadm)"]
        AL["continuous neural learner<br/>dd-soccer-learning-rds-continuous (gen ~362)"]
        AT["nightly tournaments"]
        AR["live server (dd-soccer-rs)"]
        ACW["commit-watcher"]
    end
    subgraph HET["Hetzner k8s (5 nodes)"]
        HT["tournaments (~every 12 min)"]
        HR["live server (dd-soccer-rs)"]
        HCW["commit-watcher"]
        HX["ai-ml platform (evolution/routing/chaos)<br/>BROKEN - not soccer"]
    end
    subgraph LOC["localhost"]
        LR["live server only (main_soccer_live)"]
    end
    RDS[("AWS RDS: ddremote<br/>policy + snapshots + corpora")]

    AL <--> RDS
    AR --> RDS
    AT --> RDS
    HT -.-> HPG[("Hetzner PG (siloed)")]
    AL -.->|experiment: deterministic / formation-LP-off| AL
```

## How a generation advances (the loop in words)

1. **Build** — the learner pod clones `main` (soccer + des), `cargo build`s
   `main_soccer_learning_run`, and resumes the latest policy + neural network from
   Postgres.
2. **Curriculum** — each cycle picks a stage (locomotion → … → full-match) that
   reshapes the drill (players-per-team, duration, pitch).
3. **Self-play** — one match (`parallel_games=1`) of the current policy vs an
   adversarial/resumed opponent. Each tick, every player runs the decision stack:
   POMDP belief + world model + formation LP feed the **policy head (actor)**;
   the chosen action is executed through **per-player MPC**.
4. **Reward** — event rewards (goals, passes, shots, chains) + tactical-learning
   shaping, with an **optional** dense pitch-control × xT territorial term.
5. **Update** — the **neural learner** does actor-critic + GAE with PPO-clipped
   multi-epoch updates over a replay buffer; **GA evolution** (every 5 games)
   mutates/crosses/selects the population.
6. **Promotion gate** — a candidate policy is promoted to a new **generation**
   only if it clears fitness / goals-conceded / play-quality thresholds.
7. **Persist** — promoted policy versions, neural-network snapshots, and corpora
   are written to Postgres (RDS `ddremote`).
8. **Serve** — the live server loads the active generation from Postgres and
   answers `/api/step` for the game/UI. The next learner cycle resumes from the
   same store, closing the loop.

## Status legend

- **Live & advancing:** policy head (actor-critic + GAE + PPO-clip), GA evolution,
  curriculum, promotion gate, Postgres policy store, live serving — all running on
  AWS (gen ~362), the *deterministic / formation-LP-off* experiment.
- **Dashed = dormant / gated (capacity built, not yet learning):**
  - **Pass-completion head** — trained on the corpus but **not consumed live**
    (the analytic estimate still drives passes).
  - **Line-depth head** (back four) — now wired **end-to-end** (collect RL samples →
    territorial reward → reward-weighted train → live consumption once trained), but
    **gated off** (`DD_SOCCER_ENABLE_BACK_FOUR_LINE_MODEL`) and carried **in-memory**
    in the learner (resets on pod restart; cross-restart Postgres persistence is the
    remaining durability step). The midfield line keeps the analytic seed.
  - **Pitch-control × xT reward** — implemented, **gated off** by default.
  - **PUCT re-ranker** — present, off by default.
- **Not learning:** Hetzner runs tournaments + serving but **no neural learner**;
  localhost serves only. The AWS learner is **single-game** (`parallel_games=1`).

See [back-four-line-model.md](back-four-line-model.md) and
[learnability-conversion-roadmap.md](learnability-conversion-roadmap.md) for the
models being wired into this loop next.
