from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("model.py")
SPEC = importlib.util.spec_from_file_location("exploration_neighborhood_model", MODULE_PATH)
assert SPEC and SPEC.loader
model = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = model
SPEC.loader.exec_module(model)


class ExplorationNeighborhoodTests(unittest.TestCase):
    def test_default_verification_is_replayable(self) -> None:
        left = model.verify()
        right = model.verify()
        self.assertEqual(left, right)
        self.assertEqual(left["status"], "ok")
        self.assertEqual(left["samples"], 32)

    def test_mirror_is_an_involution(self) -> None:
        scenario = model.base_scenario()
        round_trip = model.mirror_scenario(model.mirror_scenario(scenario))
        self.assertEqual(round_trip.ball, scenario.ball)
        self.assertEqual(round_trip.ball_velocity, scenario.ball_velocity)
        self.assertEqual(
            [(p.role, p.position, p.velocity) for p in round_trip.players],
            [(p.role, p.position, p.velocity) for p in scenario.players],
        )

    def test_mirror_swaps_directional_actions(self) -> None:
        base = model.rank_actions(model.reference_scores(model.base_scenario()))
        mirrored = model.rank_actions(model.reference_scores(model.mirror_scenario(model.base_scenario())))
        self.assertEqual(
            model.remap_mirrored_ranking(base),
            tuple(item.action for item in mirrored),
        )

    def test_seed_changes_fixture_digest(self) -> None:
        self.assertNotEqual(model.verify(seed=1)["fixtureDigest"], model.verify(seed=2)["fixtureDigest"])

    def test_role_permutation_preserves_entity_geometry(self) -> None:
        base = model.base_scenario()
        permuted = model.equivalent_role_permutation(base)
        geometry = lambda scenario: sorted(
            (entry.team, entry.position.x, entry.position.y, entry.velocity.x, entry.velocity.y)
            for entry in scenario.players
        )
        self.assertEqual(geometry(base), geometry(permuted))


if __name__ == "__main__":
    unittest.main()
