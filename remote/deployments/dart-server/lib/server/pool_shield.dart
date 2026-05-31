/// Storm-dampening shield for the MDP pool-control loop.
///
/// The autotuner / optimizer explores the *full* size × density action grid,
/// including (size, density) combinations whose capacity is far below the
/// current live load — e.g. density 100 at 40K sessions, which would need
/// ~500 host isolates pod-wide (well past any sane per-pod ceiling, and well
/// past the pod's memory budget). Broadcast verbatim, such an exploratory pick
/// collapses every warm host's free-slot count to zero in a single control
/// tick, forces a synchronized cold-start / capacity-refusal storm on the
/// accept hot path, and wedges the per-shard response loop — the exact failure
/// observed at the top of the 50K WebSocket load-test ramp.
///
/// [shieldPoolDirective] clamps the *applied* directive into the physically
/// feasible, memory-bounded region **without constraining what the policy is
/// allowed to learn**: the learner still selects freely over the whole action
/// space and its Q-table / dashboards still reflect those choices; only the
/// setpoint actually broadcast to the shards is corrected. This is a classic
/// "shielded RL" safety layer — the policy optimizes inside a region the
/// shield guarantees is survivable.
///
/// The clamps, in order:
///
///   1. **Density-decrease slew** — cap how far per-host density may drop in
///      one tick. A big single-tick reduction strands every warm host over the
///      new cap at once (their free slots go to zero) and triggers a
///      synchronized re-spawn; rising density is always safe (it only relaxes
///      caps) so it is left unrestricted.
///   2. **Density feasibility floor** — density must be high enough that the
///      pod-wide host ceiling can physically hold the offered load. This is
///      also the lever that keeps the host-isolate count (and therefore
///      base-heap RAM) under the pod's memory budget: a lower ceiling pushes
///      the floor up, packing sessions onto fewer isolates.
///   3. **Host floor** — given the shielded density, request enough hosts to
///      hold the load with headroom so connections land on warm hosts instead
///      of cold-starting an isolate on their own accept hot path.
///   4. **Host ceiling** — never request more hosts than the pod can serve, so
///      the directive can't ask shards to fork past their hard cap.
library;

/// Result of [shieldPoolDirective]: the feasibility-corrected setpoint to
/// broadcast, plus whether any clamp actually moved the policy's choice.
class ShieldedDirective {
  const ShieldedDirective({
    required this.targetHosts,
    required this.sessionsPerHost,
    required this.engaged,
  });

  /// Pod-wide host-isolate target to broadcast (already feasibility-corrected
  /// and ceiling-clamped).
  final int targetHosts;

  /// Per-host session density to broadcast (slew- and floor-corrected).
  final int sessionsPerHost;

  /// True when at least one clamp moved the chosen directive — i.e. the shield
  /// "bit" this tick. Surfaced as `dart_pool_shield_engaged_total` so the
  /// effect is observable in Grafana.
  final bool engaged;
}

/// Clamp a chosen `(targetHosts, sessionsPerHost)` directive into the
/// physically feasible, memory-bounded region for the current live load.
///
/// Pure / side-effect free so it unit-tests deterministically and can be
/// reused by both the local-autotuner and remote-optimizer control paths.
///
///   * [chosenTargetHosts] / [chosenDensity] — what the policy picked.
///   * [lastDensity] — the density applied on the previous tick (slew anchor).
///   * [liveSessions] — current pod-wide live WebSocket sessions.
///   * [maxTotalHosts] — pod-wide host-isolate ceiling (`per-shard cap × live
///     shards`). 0 disables the feasibility floor + host ceiling.
///   * [headroom] — fractional spare capacity the applied directive must
///     provide above the live load (0.2 ⇒ size for 120% of current load).
///   * [densityMaxDrop] — max fraction density may fall in one tick (0.5 ⇒ at
///     most halve). 0 disables the slew.
///   * [minDensity] / [maxDensity] — absolute per-host density bounds the
///     supervisor enforces; the shield never broadcasts outside them.
ShieldedDirective shieldPoolDirective({
  required int chosenTargetHosts,
  required int chosenDensity,
  required int lastDensity,
  required int liveSessions,
  required int maxTotalHosts,
  double headroom = 0.2,
  double densityMaxDrop = 0.5,
  int minDensity = 1,
  int maxDensity = 2000,
}) {
  var hosts = chosenTargetHosts < 0 ? 0 : chosenTargetHosts;
  var density = chosenDensity.clamp(minDensity, maxDensity);

  // No live load → nothing to protect. Pass the policy choice through (still
  // honouring the absolute density clamp above).
  if (liveSessions <= 0) {
    return ShieldedDirective(
      targetHosts: hosts,
      sessionsPerHost: density,
      engaged: false,
    );
  }

  final h = headroom < 0 ? 0.0 : headroom;
  final demand = (liveSessions * (1.0 + h)).ceil();
  var engaged = false;

  // (1) Density-decrease slew (smoothing). Increases are unrestricted.
  if (densityMaxDrop > 0 && lastDensity > 0 && density < lastDensity) {
    final slewFloor = (lastDensity * (1.0 - densityMaxDrop)).floor();
    if (density < slewFloor) {
      density = slewFloor;
      engaged = true;
    }
  }

  // (2) Density feasibility floor: the host ceiling must be able to hold the
  //     load at this density (also the pod's memory governor).
  if (maxTotalHosts > 0) {
    final minFeasibleDensity = (demand / maxTotalHosts).ceil();
    if (density < minFeasibleDensity) {
      density = minFeasibleDensity;
      engaged = true;
    }
  }
  density = density.clamp(minDensity, maxDensity);

  // (3) Host floor: enough hosts to hold the load at the shielded density.
  if (density > 0) {
    final minHosts = (demand / density).ceil();
    if (hosts < minHosts) {
      hosts = minHosts;
      engaged = true;
    }
  }

  // (4) Host ceiling: never request more than the pod can serve.
  if (maxTotalHosts > 0 && hosts > maxTotalHosts) {
    hosts = maxTotalHosts;
  }

  return ShieldedDirective(
    targetHosts: hosts,
    sessionsPerHost: density,
    engaged: engaged,
  );
}
