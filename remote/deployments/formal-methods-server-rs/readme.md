# `dd-formal-methods-server`

A small Rust HTTP service that runs **formal-methods style reasoning** over a
codebase. It accepts three flavours of input:

1. A git repo URL (analyze a whole codebase at a given ref).
2. Inline source (synchronous one-shot check, no clone, no job).
3. A **GitHub `pull_request` webhook** (clone the PR head, diff against the
   base, analyze only the changed files, then post a finding summary back as a
   PR comment).

In all three modes the actual reasoning is the same: extract a tiny annotation
DSL from source comments, compile each verification unit to SMT-LIB, and
discharge the resulting verification conditions with the
[Z3 SMT solver](https://github.com/Z3Prover/z3). Findings come back as
structured JSON, with **counterexample models** when Z3 can falsify a goal.

It is the formal/deductive counterpart to the lint/style tooling we already
run on the same checkouts: where a linter says *"this line looks fishy"*,
this service says **"under the declared assumptions there exist values
v₁ … vₙ for which your `@ensures` is false — here is one"**.

---

## Two main use cases

### 1. Analyzing a codebase

Point the service at any git repo (https / ssh / `git@`), optionally pin a
branch or tag, optionally filter to a sub-tree, optionally restrict to certain
file extensions. The service shallow-clones the repo and walks every source
file looking for annotation blocks and (optionally) plain `if (...)` lines
whose path conditions can be checked against declared variables.

```bash
curl -s -X POST http://<host>/analyses \
  -H "x-server-auth: $SERVER_AUTH_SECRET" \
  -H 'content-type: application/json' \
  -d '{
    "schemaVersion": "formal-methods.v1",
    "repoUrl": "https://github.com/example/app.git",
    "gitRef": "main",
    "paths": ["src/"],
    "languages": ["rs", "ts"]
  }'
```

Then poll the returned `id`:

```bash
curl -s -H "x-server-auth: $SERVER_AUTH_SECRET" \
  http://<host>/analyses/<id>
```

You get back the job record with `status`, `filesScanned`, `z3Queries`, and a
`findings[]` array of structured findings.

For a quick one-shot check with no clone:

```bash
curl -s -X POST http://<host>/validate \
  -H "x-server-auth: $SERVER_AUTH_SECRET" \
  -H 'content-type: application/json' \
  -d '{
    "filename": "inc.rs",
    "source": "// @var x: Int\n// @requires x > 0\n// @ensures x + 1 > 0\nfn inc(x: i64) -> i64 { x + 1 }\n"
  }'
```

### 2. Analyzing a pull request (GitHub webhook)

The service can be wired up as a **GitHub webhook target**. On any
`pull_request` event whose `action` is `opened`, `synchronize`, `reopened`, or
`ready_for_review`, it will:

1. Verify the `X-Hub-Signature-256` HMAC against `GITHUB_WEBHOOK_SECRET`.
2. Extract the PR's `head.sha`, `base.sha`, and `head.repo.clone_url`.
3. `git init` + `git fetch --depth 1 <head_sha>` + `git checkout <head_sha>`.
4. Best-effort `git fetch --depth N <base_sha>` and
   `git diff --name-only <base_sha>..<head_sha>` to discover changed files.
5. Run the same SMT-checked analysis, **scoped to those changed files**
   (controlled by `FORMAL_METHODS_PR_DIFF_ONLY`, default `true`).
6. If `GITHUB_API_TOKEN` is configured, **post a markdown summary back as a
   PR comment** via `POST /repos/{owner}/{repo}/issues/{number}/comments`.

#### GitHub side: configuring the webhook

1. In the target repository or organisation, go to
   **Settings → Webhooks → Add webhook**.
2. Set **Payload URL** to:

   ```
   https://<your-host>/webhooks/github
   ```

3. Set **Content type** to `application/json`.
4. Set **Secret** to a long random string; this is the value you will set as
   `GITHUB_WEBHOOK_SECRET` on the server side.
5. Under **Which events would you like to trigger this webhook?**, select
   **Let me select individual events** and check only **Pull requests**.
6. Click **Add webhook**. GitHub will immediately send a `ping` event that the
   server responds to with `{ "ok": true, "event": "ping" }` (200), which
   confirms the signature is wired up correctly.

You also need a fine-grained personal access token (or GitHub App
installation token) with `pull_requests: write` on the target repo if you
want the server to post PR comments back. Set it as `GITHUB_API_TOKEN` on the
server side. If unset, the server still analyzes the PR and stores findings,
it just doesn't comment back.

#### What ends up on the PR

The comment looks like:

> **dd-formal-methods-server** — PR #123 (octocat/example)
>
> - head: `9c0a4f…`, base: `b2e110…`, job: `formal-…-7`
> - files scanned: 12, Z3 queries: 47
> - analysis scope: 6 changed paths (base..head diff)
>
> 🔎 2 finding(s): 1 error · 1 warning · 0 info
>
> | Severity | Kind | File | Line | Message |
> | --- | --- | --- | --- | --- |
> | 🔴 error | `PostconditionViolation` | `src/lib.rs` | 14 | `@ensures` can be violated |
> | 🟠 warning | `TautologyAlwaysTrue` | `src/route.rs` | 41 | `if (n >= 0)` is always true |
>
> <details><summary>Counterexamples</summary>
>
> - **PostconditionViolation** at `src/lib.rs:14` (goal: `y * y > 0`):
>   - `y = 0`
>
> </details>

If you don't want a PR comment, set `FORMAL_METHODS_PR_COMMENT_ENABLED=false`.

#### Looking up PR jobs programmatically

```bash
curl -s -H "x-server-auth: $SERVER_AUTH_SECRET" \
  https://<host>/pulls/octocat/example/123
```

Returns `{ owner, repo, number, latest: <JobRecord>, jobs: [<JobRecord>...] }`
in newest-first order so you can see all analyses that have ever run for that
PR (e.g. one per `synchronize`).

#### Webhook setup walkthrough (manual `curl` test)

You can simulate a real GitHub webhook with `curl` + `openssl` before
configuring GitHub:

```bash
SECRET="$GITHUB_WEBHOOK_SECRET"
PAYLOAD=/tmp/pr.json

cat > "$PAYLOAD" <<'JSON'
{
  "action": "opened",
  "pull_request": {
    "number": 1,
    "html_url": "https://github.com/octocat/Hello-World/pull/1",
    "head": {
      "sha": "<head_sha>",
      "ref": "feature/x",
      "repo": { "clone_url": "https://github.com/octocat/Hello-World.git" }
    },
    "base": { "sha": "<base_sha>", "ref": "main" }
  },
  "repository": { "full_name": "octocat/Hello-World" },
  "sender": { "login": "octocat" }
}
JSON

SIG=$(openssl dgst -sha256 -hmac "$SECRET" "$PAYLOAD" | awk '{print $NF}')

curl -s -X POST http://<host>/webhooks/github \
  -H 'X-GitHub-Event: pull_request' \
  -H "X-Hub-Signature-256: sha256=$SIG" \
  -H 'content-type: application/json' \
  --data-binary @"$PAYLOAD"
```

Note: use `--data-binary @file`, not `--data @file`, otherwise curl strips
newlines and the HMAC will not match.

---

## Reasoning modes

| Mode                          | What the solver is asked                                                       | How a finding is produced                                            |
| ----------------------------- | ------------------------------------------------------------------------------ | -------------------------------------------------------------------- |
| Deduction (Hoare-style)       | `⋀ requires ∧ ⋀ assume ∧ ¬ goal`                                               | `sat` ⇒ counterexample bug; `unsat` ⇒ goal proved; `unknown` ⇒ info. |
| Induction (loop)              | `⋀ requires ∧ ⋀ assume ∧ ¬ invariant`                                          | `sat` ⇒ invariant fails on entry; lightweight termination check too. |
| Path-condition propagation    | conjunction of all enclosing `if (...)` guards plus the current guard          | `unsat` outer ⇒ guard means unreachable branch; tautology if outer ⊨ guard. |

These find exactly the kind of bugs that property-based and unit testing tend
to *miss* because they only check finitely many inputs.

## Annotation DSL

Parsed out of single-line comments (`//`, `#`, `--`, `;;`). Because comments
are inert in every supported language, the same block works inside Rust, Go,
TypeScript, Java, Python, Elixir, Lua, Bash, Gleam, etc.

```
// @var x: Int
// @var y: Int
// @var ratio: Real
// @var flag: Bool
// @assume y >= 1
// @requires x > 0
// @ensures (x + y) > x
// @invariant i <= n
// @variant n - i
// @assert flag || x > 0
```

Recognised directives:

| Directive       | Meaning                                                                            |
| --------------- | ---------------------------------------------------------------------------------- |
| `@var n: T`     | Declare a logical variable of sort `T ∈ {Int, Real, Bool}`. Default sort is `Int`. |
| `@assume e`     | Unconditional fact added to the context.                                           |
| `@requires e`   | Precondition. Added to the context of every goal in the same block.                |
| `@ensures e`    | Postcondition goal: prove `requires ∧ assume ⊢ e`.                                 |
| `@invariant e`  | Loop invariant goal: same base-step check as `@ensures`.                           |
| `@variant e`    | Termination measure: must be non-negative under the invariant.                     |
| `@assert e`     | Inline obligation: prove `requires ∧ assume ⊢ e`.                                  |

Expressions support comparisons, boolean connectives, integer/real arithmetic,
`min(a,b)`, `max(a,b)`, `abs(x)`, parentheses, and `true`/`false` literals.

Any contiguous sequence of `@`-comments forms one **verification unit**. A
non-comment line ends the unit. The plain-source heuristic checks
(`tautologyAlwaysTrue` / `tautologyAlwaysFalse` / `deadNestedBranch`) only
fire when every identifier inside the `if (...)` guard is already declared
via `@var` somewhere in the file.

## Practical examples: real-world functions with data and I/O

Most production functions don't *only* do math — they transform records, hit
databases, call HTTP services, and update process state. The DSL is still
just `Int` / `Real` / `Bool` plus arithmetic and booleans (deliberately small,
so a single Z3 query is microseconds, not seconds). The recipe for the
non-math case is always the same:

> Use **ghost variables**. Declare a `@var` for each value of interest
> (an input field, a read-back DB column, a computed intermediate, an
> aggregate over a list, an I/O return value). Capture the relationships
> you care about with `@assume`, and prove the high-level property with
> `@ensures` / `@assert`. The server checks that the *logical layer* is
> consistent; your test suite checks that the code matches the
> `@assume`d behaviour.

In other words: the FM layer reasons over a small **algebraic projection**
of your function. I/O calls are opaque, but their *results* — "the row that
came back has these fields", "the request body had this size", "this side
effect produced this delta" — are perfectly fine as `@assume`s.

### Node.js / NestJS

A pure data transformation in a NestJS service:

```ts
// @var item_count: Int
// @var unit_price: Int             // cents
// @var discount_bps: Int           // 0..10000
// @var subtotal: Int
// @var discount: Int
// @var total: Int
//
// @requires item_count > 0
// @requires unit_price > 0
// @requires discount_bps >= 0 && discount_bps <= 10000
// @assume subtotal == item_count * unit_price
// @assume discount == (subtotal * discount_bps) / 10000
// @assume total == subtotal - discount
//
// @ensures total >= 0
// @ensures total <= subtotal
// @ensures (subtotal - total) == discount
@Injectable()
export class PricingService {
  computeTotal(itemCount: number, unitPrice: number, discountBps: number): number {
    const subtotal = itemCount * unitPrice;
    const discount = Math.floor((subtotal * discountBps) / 10000);
    return subtotal - discount;
  }
}
```

The same pattern when there is real I/O. The `await` calls are opaque to
the analyser, but we *model the observed values* (`balance_before` after
the read, `balance_after` after the write) as ghost vars, and capture the
DB-level invariant as an `@assume`:

```ts
// @var requested: Int
// @var balance_before: Int          // observed value of users.balance read
// @var balance_after: Int           // value we will write back
//
// @requires requested > 0
// @assume balance_before >= 0       // schema invariant on users.balance
// @requires balance_before >= requested
// @assume balance_after == balance_before - requested
//
// @ensures balance_after >= 0
// @ensures balance_after < balance_before
async withdraw(userId: number, requested: number): Promise<number> {
  const user = await this.users.findOneOrFail({ where: { id: userId } });
  if (user.balance < requested) throw new ForbiddenException('insufficient');
  user.balance -= requested;
  await this.users.save(user);
  return user.balance;
}
```

`balance_before >= 0` is an `@assume`, not an `@ensures` — we are claiming
"*if* the DB column is non-negative (which the schema and migrations
enforce), then this function preserves that invariant". The non-negativity
of the column itself is the database's job; ours is to prove we never
violate it.

### Rust

A pure widening split (the kind of math that already bit at least one
Solana AMM):

```rust
// @var total: Int
// @var split_bps: Int           // 0..10000
// @var part_a: Int
// @var part_b: Int
//
// @requires total >= 0
// @requires split_bps >= 0 && split_bps <= 10000
// @assume part_a == (total * split_bps) / 10000
// @assume part_b == total - part_a
//
// @ensures part_a + part_b == total
// @ensures part_a >= 0 && part_b >= 0
// @ensures part_a <= total && part_b <= total
pub fn split(total: u64, split_bps: u16) -> (u64, u64) {
    let part_a = ((total as u128 * split_bps as u128) / 10_000) as u64;
    (part_a, total - part_a)
}
```

I/O-bound: a capped read on a `tokio::TcpStream`. The actual `.read(...)`
is opaque, but we constrain the **return-value semantics** by introducing
`bytes_read` as a ghost variable:

```rust
// @var buf_capacity: Int
// @var max_allowed: Int
// @var bytes_read: Int
//
// @requires buf_capacity > 0
// @requires max_allowed > 0
// @requires max_allowed <= buf_capacity
// @assume bytes_read >= 0
// @assume bytes_read <= max_allowed
//
// @ensures bytes_read <= buf_capacity
// @ensures bytes_read <= max_allowed
pub async fn read_capped(
    stream: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    max_allowed: usize,
) -> std::io::Result<usize> {
    let cap = buf.len().min(max_allowed);
    stream.read(&mut buf[..cap]).await
}
```

### Elixir

A `GenServer` state transition with a fee:

```elixir
# @var balance_before: Int
# @var balance_after: Int
# @var amount: Int
# @var fee: Int
#
# @requires amount > 0
# @requires fee >= 0
# @requires fee < amount
# @requires balance_before >= amount
# @assume balance_after == balance_before - amount
#
# @ensures balance_after >= 0
# @ensures balance_after < balance_before
# @ensures (balance_before - balance_after) == amount
def handle_call({:withdraw, amount, fee}, _from, %{balance: balance} = state) do
  cond do
    amount <= fee -> {:reply, {:error, :fee_too_high}, state}
    balance < amount -> {:reply, {:error, :insufficient}, state}
    true ->
      new_balance = balance - amount
      {:reply, {:ok, new_balance}, %{state | balance: new_balance}}
  end
end
```

A Phoenix-style rate limiter — `tokens_before` and `elapsed_ms` come from
I/O (the system clock and the persisted bucket), and we model the refill
+ admit decision as ghost-variable algebra:

```elixir
# @var tokens_before: Int
# @var refill_per_sec: Int
# @var elapsed_ms: Int
# @var max_tokens: Int
# @var refilled: Int
# @var tokens_after: Int
# @var cost: Int
#
# @requires tokens_before >= 0
# @requires max_tokens >= tokens_before
# @requires refill_per_sec >= 0
# @requires elapsed_ms >= 0
# @requires cost > 0
# @assume refilled == tokens_before + (refill_per_sec * elapsed_ms) / 1000
# @assume tokens_after == min(refilled, max_tokens)
#
# @ensures tokens_after >= tokens_before
# @ensures tokens_after <= max_tokens
# @ensures refilled >= tokens_before
def refill(bucket, elapsed_ms) do
  refilled = bucket.tokens + div(bucket.rate * elapsed_ms, 1000)
  %{bucket | tokens: min(refilled, bucket.max)}
end
```

### Aggregates: lists, maps, batches

You don't reason over a list by spelling out every element — you reason
over its *aggregates*. Typical ghost variables for a collection:

- `count` — `Enum.count`, `.length`, `.size`.
- `sum`, `min`, `max` over a numeric field.
- `is_sorted` (`Bool`) — taken as an `@assume` from the producer, used as
  an invariant downstream.
- `is_non_empty` (`Bool`).

The same `count > 0 ⇒ sum / count > 0` style invariants then drop straight
into `@ensures` without ever modelling the list literal.

### Where the FM layer ends and tests begin

`@assume balance_after == balance_before - requested` is a *claim about the
code below*. The server checks the logical layer holds *given* the claim;
your test suite is what makes sure the claim is true. The division of
labour is intentional:

- The `@assume`s are usually small, mechanical, almost-obvious
  restatements of what the body does. Drift between code and assume is
  caught by ordinary unit tests, because a unit test exercising
  `withdraw(100, 30)` and checking the resulting balance is exactly
  testing `balance_after == balance_before - 30`.
- The `@ensures` are usually the cross-cutting business invariant — the
  thing nobody wrote a unit test for because "obviously it can't go
  negative". This is the layer SMT shines at: it does not care about
  finitely many inputs; it asks *"does any value exist that falsifies
  this?"* and produces one if so.

## Why we don't piggyback on the host type system (and what to use when you want to)

Your instinct — that the strongest formal-methods layer is one *fused* with
the type system — is correct, and it is exactly what the deepest verification
research has converged on:

| Approach                                       | Language                          | What lives in the type                                                            |
| ---------------------------------------------- | --------------------------------- | --------------------------------------------------------------------------------- |
| Refinement types                               | [LiquidHaskell][lh] / [Flux][flux] for Rust | `i32{v: 0 <= v && v < 100}` is a type the compiler checks at every use site. |
| Dependent types                                | Idris, Lean 4, Agda, Coq          | `Vec n A` literally encodes the length in the type.                               |
| Behavioural specs as annotations               | Dafny, [Why3][why3], Frama-C, [Prusti][prusti], [Creusot][creusot] | `requires` / `ensures` in a dedicated annotation language tied to the compiler.   |
| F* / Lean tactics                              | F\*, Lean                         | The proof itself is a program.                                                    |

[lh]: https://ucsd-progsys.github.io/liquidhaskell/
[flux]: https://flux-rs.github.io/flux/
[why3]: https://why3.lri.fr/
[prusti]: https://www.pm.inf.ethz.ch/research/prusti.html
[creusot]: https://github.com/creusot-rs/creusot

The hard problem is that **none of these are available in the standard
toolchain of the languages most production code is written in**: TypeScript,
Rust (stable), Go, Python, Java, Kotlin, Swift, Elixir, C++. Adding them
requires a compiler plugin or a dialect, and adopting them is a per-language
commitment.

`dd-formal-methods-server` deliberately picks the *other* side of that
trade-off:

1. **Comments are universal.** The same DSL works on a Rust crate, a
   NestJS service, an Elixir GenServer, a Bash deploy script, and a YAML
   policy — anything with line comments. One CI job; one PR check.
2. **Refactor-safe.** Rename `subtotal` to `pre_tax_total` and only the
   annotation block needs updating. The Z3 query is built from comments,
   not from a re-parse of the host language.
3. **Cheap.** Hundreds of milliseconds per PR for a typical service,
   because each query is `Int`/`Real`/`Bool` first-order arithmetic — the
   easy fragment for Z3.

What you lose, and how to recover it:

- **Tight binding to real types.** If the codebase already uses a
  refinement-typed language (Flux for Rust, LiquidHaskell), use that for
  shape *and* behaviour, and use this server only for cross-cutting
  invariants that span functions/services.
- **Aliasing, ownership, side-effect tracking.** For *real* Rust Hoare-style
  proofs over `&mut self`, use [Prusti][prusti] or [Creusot][creusot]
  alongside this server. They understand Rust ownership; we don't.
- **Panic / overflow / bounds in actual code paths.** For Rust, run
  [Kani][kani] (CBMC-backed bounded model checker) on `#[kani::proof]`
  harnesses. It will literally search the state space of your real code.

[kani]: https://model-checking.github.io/kani/

**The recommended stratification** (and what large verified systems like
seL4, IronFleet, and Project Everest all do in some form):

1. Compile-time **types** describe *shape* — what values inhabit a type.
2. Comment-based **specs** (this server) describe *behaviour* — invariants
   that span steps, conservation laws, monotonicity, bounded outputs.
3. A heavier **verifier** (Prusti / Kani / Flux / Certora / Dafny)
   describes *implementation* — that the body actually realises the spec.

Layer 1 is free. Layer 2 (us) is what runs on every PR. Layer 3 is what
runs before a release that matters. The three layers don't replace each
other; they catch different bugs.

## HTTP API

| Method | Path                                       | Auth header           | Purpose                                                       |
| ------ | ------------------------------------------ | --------------------- | ------------------------------------------------------------- |
| GET    | `/`                                        | —                     | Service descriptor.                                           |
| GET    | `/healthz`                                 | —                     | Liveness + config + `z3 in PATH` probe.                       |
| GET    | `/metrics`                                 | —                     | Prometheus scrape.                                            |
| POST   | `/analyses`                                | `x-server-auth`       | Submit a job for a repo or inline source.                     |
| GET    | `/analyses`                                | `x-server-auth`       | List recent jobs.                                             |
| GET    | `/analyses/{id}`                           | `x-server-auth`       | Status + findings for one job.                                |
| GET    | `/analyses/{id}/logs`                      | `x-server-auth`       | Tail the job log file.                                        |
| POST   | `/validate`                                | `x-server-auth`       | Synchronous validate of inline source (no spawned job).       |
| POST   | `/webhooks/github`                         | `X-Hub-Signature-256` | GitHub `pull_request` events. HMAC-verified, no auth header.  |
| GET    | `/pulls/{owner}/{repo}/{number}`           | `x-server-auth`       | All analyses ever queued for a given PR; latest first.        |

A finding has this shape:

```json
{
  "kind": "postconditionViolation",
  "severity": "error",
  "file": "src/lib.rs",
  "line": 14,
  "endLine": 14,
  "message": "@ensures can be violated under the declared assumptions",
  "goal": "y * y > 0",
  "counterexample": { "y": "0" },
  "solverStatus": "sat",
  "reasoning": "deduction: search for ⋀ assumptions ∧ ¬ ensures"
}
```

## Configuration

| Env                                        | Default                                       | Meaning                                                  |
| ------------------------------------------ | --------------------------------------------- | -------------------------------------------------------- |
| `HOST` / `PORT`                            | `0.0.0.0:8110`                                | Bind address.                                            |
| `FORMAL_METHODS_WORK_ROOT`                 | `/var/lib/dd-formal-methods-server/jobs`      | Per-job working dir, holds the clone + log.              |
| `FORMAL_METHODS_GIT_BIN`                   | `git`                                         | Git binary used for shallow clone.                       |
| `FORMAL_METHODS_Z3_BIN`                    | `z3`                                          | Z3 binary; piped SMT-LIB v2.                             |
| `FORMAL_METHODS_ALLOWED_REPO_PREFIXES`     | _empty_ (allow-all)                           | CSV allowlist of repo URL prefixes.                      |
| `FORMAL_METHODS_ALLOWED_EXTENSIONS`        | `rs,go,ts,tsx,js,jsx,mjs,cjs,py,...`          | Extensions scanned by `WalkDir`.                         |
| `FORMAL_METHODS_MAX_CONCURRENT`            | `2`                                           | Job semaphore.                                           |
| `FORMAL_METHODS_JOB_TIMEOUT_SECONDS`       | `900`                                         | Per-job wall-clock budget for `git clone` / `git fetch`. |
| `FORMAL_METHODS_Z3_TIMEOUT_SECONDS`        | `5`                                           | Per-SMT-query timeout.                                   |
| `FORMAL_METHODS_MAX_LOG_BYTES`             | `4194304`                                     | Cap on per-job log file.                                 |
| `FORMAL_METHODS_MAX_FILES`                 | `5000`                                        | Cap on files scanned per job.                            |
| `FORMAL_METHODS_MAX_FILE_BYTES`            | `524288`                                      | Skip files above this size.                              |
| `FORMAL_METHODS_MAX_FINDINGS_PER_JOB`      | `5000`                                        | Stop emitting once exceeded.                             |
| `FORMAL_METHODS_MAX_INLINE_SOURCE_BYTES`   | `262144`                                      | Cap on `inlineSource` size on `/analyses` and `/validate`. |
| `SERVER_AUTH_SECRET`                       | _unset_                                       | When unset, all auth-gated endpoints return 503.         |
| `GITHUB_WEBHOOK_SECRET`                    | _unset_                                       | HMAC secret for `/webhooks/github`. When unset returns 503. |
| `GITHUB_API_TOKEN`                         | _unset_                                       | Bearer token used to POST PR comments. When unset, no comment is posted. |
| `FORMAL_METHODS_GITHUB_API_BASE`           | `https://api.github.com`                      | Override for GitHub Enterprise.                          |
| `FORMAL_METHODS_PR_DIFF_ONLY`              | `true`                                        | If true, restrict PR analysis to files changed in the diff. |
| `FORMAL_METHODS_PR_COMMENT_ENABLED`        | `true` if `GITHUB_API_TOKEN` is set           | Toggle PR comment posting independently of the token.    |
| `FORMAL_METHODS_PR_COMMENT_MAX_ROWS`       | `25`                                          | Max findings rendered in the PR comment table.           |
| `FORMAL_METHODS_PR_BASE_FETCH_DEPTH`       | `200`                                         | Depth used when fetching `base_sha` for diffing.         |

## Kubernetes layout

* `k8s/dd-formal-methods-server.deployment.yaml` — Deployment that runs the
  Rust binary from the host-mounted checkout via `cargo run --release`. The
  init shell installs the `z3` apt package on first boot, mirroring the way
  `dd-build-server` bootstraps `nerdctl`.
  `GITHUB_WEBHOOK_SECRET` and `GITHUB_API_TOKEN` are read from the
  `dd-agent-secrets` Secret (both are optional).
* `k8s/dd-formal-methods-server.service.yaml` — ClusterIP Service on port 8110.
* `k8s/kustomization.yaml` — local kustomize entry point.
* `k8s/ec2/kustomization.yaml` — EC2 overlay consumed by Argo CD via
  `remote/argocd/apps/dd-formal-methods-server.application.yaml`.

To expose `/webhooks/github` to GitHub, route it via your ingress/gateway —
e.g. add a rule for `https://<gateway>/formal-methods/webhooks/github` that
strips the `/formal-methods` prefix and proxies to
`dd-formal-methods-server.default.svc:8110`. The webhook endpoint itself is
authentication-free (HMAC handles auth) so no auth header rewriting is
needed in the gateway.

## Local build

```bash
cd remote/deployments/formal-methods-server-rs
cargo test
cargo run --release
```

You also need the `z3` binary in `PATH`:

```bash
brew install z3        # macOS
apt-get install -y z3  # Debian / Ubuntu
```

## Why not bind libz3?

Shelling out to the `z3` CLI keeps the Rust crate dependency-free of native
libraries (`z3-sys` requires `libz3-dev` and a working C++ toolchain), makes
the deployment container ~20 MB smaller, and means individual queries cannot
crash the long-lived server process — each query is an isolated child with a
strict `tokio::time::timeout` budget.

## Security notes

* The only external commands invoked are `git` (a fixed allowlist of
  subcommands) and `z3 -in -smt2 -T:5`, both with a stripped environment and
  a strict timeout.
* `repoUrl` and the webhook's `pull_request.head.repo.clone_url` are
  restricted to `https://`, `ssh://`, or `git@` and matched against
  `FORMAL_METHODS_ALLOWED_REPO_PREFIXES`.
* `paths[]` filters are validated to stay inside the cloned repository
  (no `..`, no absolute paths).
* All endpoints that touch a job require either `x-server-auth:
  $SERVER_AUTH_SECRET` (the JSON API) or a valid `X-Hub-Signature-256` HMAC
  over the body (the GitHub webhook).
* Public introspection (`/`, `/healthz`, `/metrics`) is open.

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
