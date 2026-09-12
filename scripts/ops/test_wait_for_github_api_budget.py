#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).with_name("wait_for_github_api_budget.py")
SPEC = importlib.util.spec_from_file_location("github_budget", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class GitHubBudgetTests(unittest.TestCase):
    def test_parse_valid_budget(self):
        budget = MODULE.parse_budget(
            {
                "resources": {
                    "core": {"remaining": 1810, "reset": 2_000_000_000},
                    "graphql": {"remaining": 400, "reset": 2_000_000_100},
                }
            }
        )
        self.assertEqual(budget.core_remaining, 1810)
        self.assertEqual(budget.graphql_remaining, 400)

    def test_parse_rejects_missing_negative_and_zero_reset_values(self):
        with self.assertRaisesRegex(MODULE.BudgetError, "missing core/graphql"):
            MODULE.parse_budget({})
        with self.assertRaisesRegex(MODULE.BudgetError, "must not be negative"):
            MODULE.parse_budget(
                {
                    "resources": {
                        "core": {"remaining": -1, "reset": 1},
                        "graphql": {"remaining": 1, "reset": 1},
                    }
                }
            )
        with self.assertRaisesRegex(MODULE.BudgetError, "reset epochs must be positive"):
            MODULE.parse_budget(
                {
                    "resources": {
                        "core": {"remaining": 1, "reset": 0},
                        "graphql": {"remaining": 1, "reset": 1},
                    }
                }
            )

    def test_sufficient_budget_returns_without_sleeping(self):
        ready = MODULE.RateBudget(1810, 2_000_000_000, 400, 2_000_000_100)
        with mock.patch.object(MODULE, "get_budget", return_value=ready), mock.patch.object(
            MODULE.time, "sleep"
        ) as sleep:
            self.assertEqual(
                MODULE.wait_for_budget(
                    min_core=1810,
                    min_graphql=400,
                    max_wait_seconds=100,
                    poll_seconds=10,
                ),
                ready,
            )
        sleep.assert_not_called()

    def test_low_budget_sleeps_once_then_recovers(self):
        low = MODULE.RateBudget(0, 1010, 20, 1010)
        ready = MODULE.RateBudget(1810, 2000, 400, 2000)
        monotonic_values = iter([0.0, 10.0])
        with mock.patch.object(MODULE, "get_budget", side_effect=[low, ready]), mock.patch.object(
            MODULE.time, "time", return_value=1000
        ), mock.patch.object(
            MODULE.time, "monotonic", side_effect=lambda: next(monotonic_values)
        ), mock.patch.object(MODULE.time, "sleep") as sleep:
            result = MODULE.wait_for_budget(
                min_core=1810,
                min_graphql=400,
                max_wait_seconds=100,
                poll_seconds=30,
            )
        self.assertEqual(result, ready)
        sleep.assert_called_once_with(20)

    def test_timeout_fails_closed_without_sleeping_past_bound(self):
        low = MODULE.RateBudget(0, 2000, 0, 2000)
        with mock.patch.object(MODULE, "get_budget", return_value=low), mock.patch.object(
            MODULE.time, "time", return_value=1000
        ), mock.patch.object(MODULE.time, "monotonic", return_value=95.0), mock.patch.object(
            MODULE.time, "sleep"
        ) as sleep:
            with self.assertRaisesRegex(MODULE.BudgetError, "did not recover"):
                MODULE.wait_for_budget(
                    min_core=1810,
                    min_graphql=400,
                    max_wait_seconds=100,
                    poll_seconds=30,
                )
        sleep.assert_not_called()

    def test_invalid_wait_arguments_fail_closed(self):
        with self.assertRaisesRegex(MODULE.BudgetError, "poll_seconds"):
            MODULE.wait_for_budget(
                min_core=1,
                min_graphql=1,
                max_wait_seconds=10,
                poll_seconds=0,
            )


if __name__ == "__main__":
    unittest.main()
