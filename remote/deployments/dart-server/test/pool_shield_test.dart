import 'dart:math';

import 'package:dd_dart_server/server/pool_shield.dart';
import 'package:test/test.dart';

void main() {
  group('shieldPoolDirective', () {
    test('passes a feasible choice through untouched', () {
      // 20K sessions, 40 hosts × 1000/host = 40K capacity, well above the
      // 24K demand (20K × 1.2). Nothing should move.
      final s = shieldPoolDirective(
        chosenTargetHosts: 40,
        chosenDensity: 1000,
        lastDensity: 1000,
        liveSessions: 20000,
        maxTotalHosts: 192,
      );
      expect(s.targetHosts, 40);
      expect(s.sessionsPerHost, 1000);
      expect(s.engaged, isFalse);
    });

    test('no live load passes through (only absolute density clamp)', () {
      final s = shieldPoolDirective(
        chosenTargetHosts: 20,
        chosenDensity: 100,
        lastDensity: 1000,
        liveSessions: 0,
        maxTotalHosts: 192,
      );
      expect(s.targetHosts, 20);
      expect(s.sessionsPerHost, 100);
      expect(s.engaged, isFalse);
    });

    test('floors an infeasible low density up so the ceiling can hold load',
        () {
      // density 100 at 40K would need ~500 hosts pod-wide — past the ceiling.
      final s = shieldPoolDirective(
        chosenTargetHosts: 50,
        chosenDensity: 100,
        lastDensity: 100, // no slew interference
        liveSessions: 40000,
        maxTotalHosts: 192,
        headroom: 0.2,
        densityMaxDrop: 0, // isolate the feasibility floor
      );
      final demand = (40000 * 1.2).ceil();
      // density must be >= ceil(demand / maxTotalHosts)
      expect(s.sessionsPerHost, greaterThanOrEqualTo((demand / 192).ceil()));
      // and the resulting capacity must hold the raw load
      expect(s.targetHosts * s.sessionsPerHost, greaterThanOrEqualTo(40000));
      expect(s.targetHosts, lessThanOrEqualTo(192));
      expect(s.engaged, isTrue);
    });

    test('density-decrease slew caps a single-tick drop to half', () {
      final s = shieldPoolDirective(
        chosenTargetHosts: 50,
        chosenDensity: 100,
        lastDensity: 1000,
        liveSessions: 5000, // low load: feasibility floor is tiny here
        maxTotalHosts: 192,
        headroom: 0.2,
        densityMaxDrop: 0.5,
      );
      // 1000 → at most halve → 500 this tick (not all the way to 100).
      expect(s.sessionsPerHost, 500);
      expect(s.engaged, isTrue);
    });

    test('rising density is never restricted by the slew', () {
      final s = shieldPoolDirective(
        chosenTargetHosts: 40,
        chosenDensity: 1000,
        lastDensity: 250,
        liveSessions: 10000,
        maxTotalHosts: 192,
        densityMaxDrop: 0.5,
      );
      expect(s.sessionsPerHost, 1000);
    });

    test('raises the host target to hold the load at the chosen density', () {
      // 30K sessions at density 1000 needs 36 hosts (30K×1.2/1000); a chosen
      // target of 20 is too small.
      final s = shieldPoolDirective(
        chosenTargetHosts: 20,
        chosenDensity: 1000,
        lastDensity: 1000,
        liveSessions: 30000,
        maxTotalHosts: 192,
        headroom: 0.2,
      );
      expect(s.targetHosts, (30000 * 1.2 / 1000).ceil());
      expect(s.targetHosts * s.sessionsPerHost, greaterThanOrEqualTo(30000));
      expect(s.engaged, isTrue);
    });

    test('never requests more hosts than the pod ceiling', () {
      final s = shieldPoolDirective(
        chosenTargetHosts: 1000,
        chosenDensity: 1000,
        lastDensity: 1000,
        liveSessions: 50000,
        maxTotalHosts: 192,
      );
      expect(s.targetHosts, lessThanOrEqualTo(192));
    });

    test('maxTotalHosts <= 0 disables the feasibility floor + ceiling', () {
      final s = shieldPoolDirective(
        chosenTargetHosts: 5,
        chosenDensity: 100,
        lastDensity: 100,
        liveSessions: 40000,
        maxTotalHosts: 0,
        densityMaxDrop: 0,
      );
      // Host floor still applies (density-based), but no density feasibility
      // floor and no ceiling clamp.
      expect(s.sessionsPerHost, 100);
      expect(s.targetHosts, (40000 * 1.2 / 100).ceil());
    });

    test('invariant: shielded capacity always holds the live load', () {
      final rng = Random(1234);
      const maxTotalHosts = 192;
      const maxDensity = 2000;
      for (var i = 0; i < 5000; i++) {
        // Keep load within what the pod can physically hold so the ceiling
        // clamp never has to under-provision.
        final live = rng.nextInt(maxTotalHosts * maxDensity ~/ 2);
        final s = shieldPoolDirective(
          chosenTargetHosts: rng.nextInt(120),
          chosenDensity: 1 + rng.nextInt(maxDensity),
          lastDensity: 1 + rng.nextInt(maxDensity),
          liveSessions: live,
          maxTotalHosts: maxTotalHosts,
          headroom: 0.2,
          densityMaxDrop: 0.5,
          maxDensity: maxDensity,
        );
        expect(s.sessionsPerHost, inInclusiveRange(1, maxDensity));
        expect(s.targetHosts, inInclusiveRange(0, maxTotalHosts));
        if (live > 0) {
          expect(
            s.targetHosts * s.sessionsPerHost,
            greaterThanOrEqualTo(live),
            reason: 'capacity must hold live=$live '
                '(got ${s.targetHosts}×${s.sessionsPerHost})',
          );
        }
      }
    });
  });
}
