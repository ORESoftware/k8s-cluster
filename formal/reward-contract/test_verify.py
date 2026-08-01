from __future__ import annotations

import copy
import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("verify.py")
SPEC = importlib.util.spec_from_file_location("reward_contract_verify", MODULE_PATH)
assert SPEC and SPEC.loader
verify_module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = verify_module
SPEC.loader.exec_module(verify_module)
CONTRACT_PATH = Path(__file__).with_name("reward_contract.json")


class RewardContractTests(unittest.TestCase):
    def contract(self):
        return verify_module.load_contract(CONTRACT_PATH)

    def test_committed_contract_passes(self) -> None:
        result = verify_module.verify(self.contract())
        self.assertEqual(result["status"], "ok")
        self.assertEqual(result["componentCount"], 10)
        self.assertGreaterEqual(
            result["terminalDominanceMarginActual"],
            result["terminalDominanceMarginRequired"],
        )

    def test_duplicate_component_is_rejected(self) -> None:
        contract = self.contract()
        contract["components"].append(copy.deepcopy(contract["components"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate component"):
            verify_module.verify(contract)

    def test_positive_penalty_is_rejected(self) -> None:
        contract = self.contract()
        component = next(item for item in contract["components"] if item["id"] == "foul")
        component["weight"] = 2.0
        with self.assertRaisesRegex(ValueError, "penalty component can become positive"):
            verify_module.verify(contract)

    def test_shaping_cannot_overpower_goal(self) -> None:
        contract = self.contract()
        component = next(item for item in contract["components"] if item["id"] == "xg_delta")
        component["weight"] = 40.0
        with self.assertRaisesRegex(ValueError, "overpower terminal outcome"):
            verify_module.verify(contract)

    def test_out_of_range_runtime_value_is_rejected(self) -> None:
        contract = self.contract()
        with self.assertRaisesRegex(ValueError, "outside"):
            verify_module.aggregate_reward(contract, {"turnover": 2.0})

    def test_unknown_runtime_component_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unknown reward components"):
            verify_module.aggregate_reward(self.contract(), {"invented_bonus": 1.0})


if __name__ == "__main__":
    unittest.main()
