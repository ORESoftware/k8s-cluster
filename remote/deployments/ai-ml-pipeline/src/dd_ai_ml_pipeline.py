#!/usr/bin/env python3
from __future__ import annotations

import json
import math
import os
import re
import socket
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from hmac import compare_digest
from dataclasses import dataclass, field
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Optional
from urllib.parse import urlparse


GENERATED_DOCS_DIR = Path(__file__).resolve().parents[1] / "generated"

# Source-of-truth NATS subject constants. The repo is mounted at
# /opt/dd-next-1 inside the pod, so the generated lib lives four parents up
# from this file (.../remote/libs/nats/subject-defs/generated/python).
# Injecting the path keeps the script standalone-runnable in CI/dev without
# requiring a pip install.
_NATS_SUBJECT_DEFS_DIR = (
    Path(__file__).resolve().parents[3]
    / "libs"
    / "nats"
    / "subject-defs"
    / "generated"
    / "python"
)
if str(_NATS_SUBJECT_DEFS_DIR) not in sys.path:
    sys.path.insert(0, str(_NATS_SUBJECT_DEFS_DIR))
from dd_nats_subject_defs import (  # noqa: E402 - sys.path setup above
    ML_DEAD_LETTER_SUBJECT,
    ML_FEATURES_SUBJECT,
    RUNTIME_EVENTS_SUBJECT,
    TELEMETRY_MDP_SUBJECT,
    TELEMETRY_RAW_QUEUE_GROUP,
    TELEMETRY_RAW_SUBJECT,
)
from otel import init_telemetry, instrument_handler  # noqa: E402 - sibling module in src/


def read_int_env(
    name: str,
    fallback: int,
    min_value: int = 1,
    max_value: Optional[int] = None,
) -> int:
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return fallback
    try:
        value = int(raw)
    except ValueError:
        return fallback
    if value < min_value:
        return fallback
    if max_value is not None and value > max_value:
        return max_value
    return value


def read_float_env(
    name: str,
    fallback: float,
    min_value: float,
    max_value: float,
) -> float:
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return fallback
    try:
        value = float(raw)
    except ValueError:
        return fallback
    if not math.isfinite(value):
        return fallback
    return max(min_value, min(max_value, value))


def read_bool_env(name: str, fallback: bool = False) -> bool:
    raw = os.getenv(name)
    if raw is None:
        return fallback
    normalized = raw.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    return fallback


SERVICE_NAME = "dd-ai-ml-pipeline"
MAX_TELEMETRY_WINDOW_MS = 24 * 60 * 60 * 1000
MAX_TOKEN_BYTES = 96
MAX_REQUEST_ID_BYTES = 128
MAX_ACTION_IMPACTS = 16
MAX_SIGNAL_WEIGHT = 100.0
MAX_BODY_BYTES = read_int_env("ML_MAX_BODY_BYTES", 256 * 1024, 1024, 1024 * 1024)
MAX_SIGNALS = read_int_env("ML_MAX_SIGNALS", 128, 1, 128)
DEFAULT_WINDOW_MS = read_int_env(
    "ML_DEFAULT_WINDOW_MS", 60_000, 1, MAX_TELEMETRY_WINDOW_MS
)
DEFAULT_ACTIONS = [
    "hold",
    "observe",
    "scale-up",
    "restart",
    "shed-load",
    "enable-fallback",
    "throttle-feature",
    "page-human",
]


def now_ms() -> int:
    return int(time.time() * 1000)


def clamp(value: float, low: float = 0.0, high: float = 1.0) -> float:
    return max(low, min(high, value))


def finite_float(value: Any, name: str) -> float:
    if isinstance(value, bool):
        raise ValueError(f"{name} must be a finite number")
    try:
        parsed = float(value)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{name} must be a finite number") from error
    if not math.isfinite(parsed):
        raise ValueError(f"{name} must be a finite number")
    return parsed


def optional_float(value: Any, name: str) -> Optional[float]:
    if value is None:
        return None
    return finite_float(value, name)


def env_value(key: str, fallback: str) -> str:
    value = os.getenv(key, "").strip()
    return value if value else fallback


def normalize_token(value: Any, fallback: str, max_bytes: int = MAX_TOKEN_BYTES) -> str:
    if isinstance(value, (dict, list, tuple, set)):
        raise ValueError("telemetry token must be a string")
    raw = str(value if value is not None else fallback).strip()
    if not raw:
        raw = fallback
    if has_control_characters(raw):
        raise ValueError("telemetry token must not contain control characters")
    if len(raw.encode("utf-8")) > max_bytes:
        raise ValueError(f"telemetry token must be at most {max_bytes} bytes")
    return raw


def metric_key(name: str) -> str:
    value = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    value = re.sub(r"[^a-zA-Z0-9]+", "_", value).strip("_").lower()
    return value or "unknown"


def infer_layer(name: str, fallback: Optional[str]) -> str:
    if fallback:
        normalized = str(fallback).strip().lower()
        if normalized in {"infra", "infrastructure", "platform", "messaging", "data"}:
            return "infra"
        if normalized in {"app", "application", "service"}:
            return "app"
        if normalized == "mixed":
            return "mixed"

    key = metric_key(name)
    infra_markers = ("cpu", "memory", "mem", "disk", "pod", "restart", "queue", "nats", "kafka")
    return "infra" if any(marker in key for marker in infra_markers) else "app"


def state_for_risk(risk: float) -> str:
    if risk >= 0.75:
        return "critical"
    if risk >= 0.50:
        return "degraded"
    if risk >= 0.25:
        return "watch"
    return "nominal"


def action_cost(action: str) -> float:
    return {
        "hold": 0.0,
        "observe": 0.02,
        "scale-up": 0.10,
        "throttle-feature": 0.12,
        "enable-fallback": 0.16,
        "disable-experiment": 0.18,
        "shed-load": 0.20,
        "restart": 0.22,
        "page-human": 0.42,
    }.get(action, 0.15)


def validate_window_ms(value: Any) -> int:
    if value is None:
        return DEFAULT_WINDOW_MS
    if isinstance(value, bool):
        raise ValueError("windowMs must be an integer")
    try:
        window_ms = int(value)
    except (TypeError, ValueError) as error:
        raise ValueError("windowMs must be an integer") from error
    if window_ms <= 0 or window_ms > MAX_TELEMETRY_WINDOW_MS:
        raise ValueError(f"windowMs must be in the range 1..{MAX_TELEMETRY_WINDOW_MS}")
    return window_ms


def normalize_action_name(value: Any) -> str:
    token = normalize_token(value, "observe").lower()
    if re.search(r"[^a-z0-9_.:-]", token):
        raise ValueError("action names may only contain letters, numbers, '-', '_', '.', or ':'")
    return token


def normalize_action_impacts(value: Any) -> list[dict[str, Any]]:
    if value is None:
        return []
    if not isinstance(value, list):
        raise ValueError("actionImpacts must be an array")
    if len(value) > MAX_ACTION_IMPACTS:
        raise ValueError(f"actionImpacts must include at most {MAX_ACTION_IMPACTS} entries")
    impacts: list[dict[str, Any]] = []
    for item in value:
        if not isinstance(item, dict):
            raise ValueError("actionImpacts entries must be objects")
        delta = finite_float(item.get("delta", 0.0), "action impact delta")
        if delta < -1.0 or delta > 1.0:
            raise ValueError("action impact delta must be between -1 and 1")
        confidence = clamp(finite_float(item.get("confidence", 1.0), "action impact confidence"))
        impacts.append(
            {
                "action": normalize_action_name(item.get("action", "observe")),
                "delta": delta,
                "confidence": confidence,
            }
        )
    return impacts


def validate_weight(value: Any, name: str) -> float:
    weight = finite_float(value, name)
    if weight < 0.0 or weight > MAX_SIGNAL_WEIGHT:
        raise ValueError(f"{name} must be between 0 and {MAX_SIGNAL_WEIGHT}")
    return weight


def has_control_characters(value: str) -> bool:
    return any(ord(character) < 32 or ord(character) == 127 for character in value)


def validate_nats_name(value: str, label: str, allow_wildcards: bool = False) -> str:
    token = value.strip()
    if not token:
        raise ValueError(f"{label} must not be empty")
    if has_control_characters(token) or any(character.isspace() for character in token):
        raise ValueError(f"{label} must not contain whitespace or control characters")
    if not allow_wildcards and ("*" in token or ">" in token):
        raise ValueError(f"{label} must not contain NATS wildcards")
    return token


def constant_time_equals(value: Optional[str], expected: str) -> bool:
    if value is None:
        return False
    if len(value) != len(expected):
        return False
    return compare_digest(value.encode("utf-8"), expected.encode("utf-8"))


@dataclass
class Config:
    host: str = field(default_factory=lambda: env_value("HOST", "0.0.0.0"))
    port: int = field(default_factory=lambda: read_int_env("PORT", 8099, 1, 65535))
    server_auth_secret: Optional[str] = field(default_factory=lambda: os.getenv("SERVER_AUTH_SECRET"))
    allow_unauthenticated: bool = field(
        default_factory=lambda: read_bool_env("ML_ALLOW_UNAUTHENTICATED", False)
    )
    nats_url: str = field(
        default_factory=lambda: env_value(
            "NATS_URL", "nats://dd-nats.messaging.svc.cluster.local:4222"
        )
    )
    raw_subject: str = field(
        default_factory=lambda: env_value("ML_RAW_TELEMETRY_SUBJECT", TELEMETRY_RAW_SUBJECT)
    )
    queue_group: str = field(
        default_factory=lambda: env_value("ML_QUEUE_GROUP", TELEMETRY_RAW_QUEUE_GROUP)
    )
    feature_subject: str = field(
        default_factory=lambda: env_value("ML_FEATURE_SUBJECT", ML_FEATURES_SUBJECT)
    )
    mdp_subject: str = field(
        default_factory=lambda: env_value("ML_MDP_TELEMETRY_SUBJECT", TELEMETRY_MDP_SUBJECT)
    )
    event_subject: str = field(
        default_factory=lambda: env_value("ML_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT)
    )
    dead_letter_subject: str = field(
        default_factory=lambda: env_value("ML_DEAD_LETTER_SUBJECT", ML_DEAD_LETTER_SUBJECT)
    )
    min_samples_for_zscore: int = field(
        default_factory=lambda: read_int_env("ML_MIN_SAMPLES_FOR_ZSCORE", 4, 1, 10_000)
    )
    ewma_alpha: float = field(
        default_factory=lambda: read_float_env("ML_EWMA_ALPHA", 0.22, 0.01, 1.0)
    )
    max_tracked_series: int = field(
        default_factory=lambda: read_int_env("ML_MAX_TRACKED_SERIES", 4096, 128, 1_000_000)
    )
    max_transition_keys: int = field(
        default_factory=lambda: read_int_env("ML_MAX_TRANSITION_KEYS", 2048, 128, 1_000_000)
    )
    max_publish_bytes: int = field(
        default_factory=lambda: read_int_env("ML_MAX_PUBLISH_BYTES", 512 * 1024, 1024, 2 * 1024 * 1024)
    )

    def __post_init__(self) -> None:
        validate_nats_name(self.raw_subject, "ML_RAW_TELEMETRY_SUBJECT", allow_wildcards=True)
        validate_nats_name(self.queue_group, "ML_QUEUE_GROUP")
        validate_nats_name(self.feature_subject, "ML_FEATURE_SUBJECT")
        validate_nats_name(self.mdp_subject, "ML_MDP_TELEMETRY_SUBJECT")
        validate_nats_name(self.event_subject, "ML_EVENT_SUBJECT")
        validate_nats_name(self.dead_letter_subject, "ML_DEAD_LETTER_SUBJECT")


class Metrics:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._values: dict[str, int] = {
            "requests_total": 0,
            "ingest_requests_total": 0,
            "analyses_total": 0,
            "errors_total": 0,
            "features_total": 0,
            "nats_messages_total": 0,
            "nats_publish_errors_total": 0,
            "dead_letters_total": 0,
            "published_features_total": 0,
            "published_mdp_total": 0,
            "published_events_total": 0,
            "published_dead_letters_total": 0,
            "auth_failures_total": 0,
            "dropped_series_total": 0,
            "dropped_transitions_total": 0,
        }

    def inc(self, name: str, amount: int = 1) -> None:
        with self._lock:
            self._values[name] = self._values.get(name, 0) + amount

    def snapshot(self) -> dict[str, int]:
        with self._lock:
            return dict(self._values)

    def prometheus(self) -> str:
        values = self.snapshot()
        lines = [
            "# HELP dd_ai_ml_pipeline_requests_total HTTP requests handled.",
            "# TYPE dd_ai_ml_pipeline_requests_total counter",
            f"dd_ai_ml_pipeline_requests_total {values['requests_total']}",
            "# HELP dd_ai_ml_pipeline_ingest_requests_total Telemetry ingest requests handled.",
            "# TYPE dd_ai_ml_pipeline_ingest_requests_total counter",
            f"dd_ai_ml_pipeline_ingest_requests_total {values['ingest_requests_total']}",
            "# HELP dd_ai_ml_pipeline_analyses_total Telemetry analyses completed.",
            "# TYPE dd_ai_ml_pipeline_analyses_total counter",
            f"dd_ai_ml_pipeline_analyses_total {values['analyses_total']}",
            "# HELP dd_ai_ml_pipeline_errors_total Request or analysis errors.",
            "# TYPE dd_ai_ml_pipeline_errors_total counter",
            f"dd_ai_ml_pipeline_errors_total {values['errors_total']}",
            "# HELP dd_ai_ml_pipeline_features_total Features emitted by the online model.",
            "# TYPE dd_ai_ml_pipeline_features_total counter",
            f"dd_ai_ml_pipeline_features_total {values['features_total']}",
            "# HELP dd_ai_ml_pipeline_nats_messages_total Raw telemetry messages read from NATS.",
            "# TYPE dd_ai_ml_pipeline_nats_messages_total counter",
            f"dd_ai_ml_pipeline_nats_messages_total {values['nats_messages_total']}",
            "# HELP dd_ai_ml_pipeline_nats_publish_errors_total NATS publish failures.",
            "# TYPE dd_ai_ml_pipeline_nats_publish_errors_total counter",
            f"dd_ai_ml_pipeline_nats_publish_errors_total {values['nats_publish_errors_total']}",
            "# HELP dd_ai_ml_pipeline_dead_letters_total Rejected NATS telemetry messages.",
            "# TYPE dd_ai_ml_pipeline_dead_letters_total counter",
            f"dd_ai_ml_pipeline_dead_letters_total {values['dead_letters_total']}",
            "# HELP dd_ai_ml_pipeline_published_messages_total Published pipeline messages by subject role.",
            "# TYPE dd_ai_ml_pipeline_published_messages_total counter",
            'dd_ai_ml_pipeline_published_messages_total{role="features"} '
            f"{values['published_features_total']}",
            'dd_ai_ml_pipeline_published_messages_total{role="mdp"} '
            f"{values['published_mdp_total']}",
            'dd_ai_ml_pipeline_published_messages_total{role="events"} '
            f"{values['published_events_total']}",
            'dd_ai_ml_pipeline_published_messages_total{role="dead-letter"} '
            f"{values['published_dead_letters_total']}",
            "# HELP dd_ai_ml_pipeline_auth_failures_total Rejected HTTP requests with missing or invalid auth.",
            "# TYPE dd_ai_ml_pipeline_auth_failures_total counter",
            f"dd_ai_ml_pipeline_auth_failures_total {values['auth_failures_total']}",
            "# HELP dd_ai_ml_pipeline_dropped_series_total New time series dropped after the in-memory cap.",
            "# TYPE dd_ai_ml_pipeline_dropped_series_total counter",
            f"dd_ai_ml_pipeline_dropped_series_total {values['dropped_series_total']}",
            "# HELP dd_ai_ml_pipeline_dropped_transitions_total New transition counters dropped after the in-memory cap.",
            "# TYPE dd_ai_ml_pipeline_dropped_transitions_total counter",
            f"dd_ai_ml_pipeline_dropped_transitions_total {values['dropped_transitions_total']}",
            "",
        ]
        return "\n".join(lines)


@dataclass
class FeatureStats:
    count: int = 0
    mean: float = 0.0
    m2: float = 0.0
    ewma: Optional[float] = None
    last_value: Optional[float] = None
    last_seen_ms: Optional[int] = None

    @property
    def variance(self) -> float:
        if self.count < 2:
            return 0.0
        return self.m2 / (self.count - 1)

    @property
    def stddev(self) -> float:
        return math.sqrt(max(self.variance, 0.0))

    def observe(self, value: float, alpha: float) -> None:
        self.count += 1
        delta = value - self.mean
        self.mean += delta / self.count
        delta2 = value - self.mean
        self.m2 += delta * delta2
        self.ewma = value if self.ewma is None else alpha * value + (1.0 - alpha) * self.ewma
        self.last_value = value
        self.last_seen_ms = now_ms()


@dataclass
class ServiceMemory:
    state: str
    action: str
    risk: float
    seen_ms: int


@dataclass(frozen=True)
class NatsTarget:
    host: str
    port: int
    user: Optional[str] = None
    password: Optional[str] = None
    token: Optional[str] = None


def threshold_defaults(name: str) -> dict[str, Any]:
    key = metric_key(name)
    defaults: dict[str, Any] = {"higherIsBetter": False, "weight": 1.0}
    if "availability" in key or "success_rate" in key:
        defaults.update({"target": 0.999, "warning": 0.995, "critical": 0.98, "higherIsBetter": True, "weight": 1.8})
    elif "error" in key or "failure" in key:
        defaults.update({"target": 0.005, "warning": 0.02, "critical": 0.08, "weight": 1.7})
    elif "latency" in key:
        defaults.update({"target": 250.0, "warning": 750.0, "critical": 2500.0, "weight": 1.4})
    elif "queue" in key or "pending" in key or "lag" in key:
        defaults.update({"target": 10.0, "warning": 50.0, "critical": 250.0, "weight": 1.3})
    elif "cpu" in key:
        defaults.update({"target": 0.55, "warning": 0.75, "critical": 0.90, "weight": 1.2})
    elif "memory" in key or key == "mem":
        defaults.update({"target": 0.60, "warning": 0.80, "critical": 0.92, "weight": 1.2})
    elif "restart" in key or "crash" in key:
        defaults.update({"target": 0.0, "warning": 1.0, "critical": 3.0, "weight": 1.5})
    elif "saturation" in key or "risk" in key:
        defaults.update({"target": 0.20, "warning": 0.50, "critical": 0.75, "weight": 1.0})
    return defaults


def risk_from_thresholds(
    value: float,
    warning: Optional[float],
    critical: Optional[float],
    target: Optional[float],
    baseline: Optional[float],
    higher_is_better: bool,
) -> float:
    if warning is not None and critical is not None:
        if abs(warning - critical) <= 1e-12:
            if higher_is_better:
                return 1.0 if value <= critical else 0.0
            return 1.0 if value >= critical else 0.0
        if higher_is_better:
            safe = max(warning, critical)
            bad = min(warning, critical)
            return clamp((safe - value) / (safe - bad))
        safe = min(warning, critical)
        bad = max(warning, critical)
        return clamp((value - safe) / (bad - safe))

    reference = target if target is not None else baseline
    if reference is None:
        return 0.0
    denominator = max(abs(reference), 1.0)
    if higher_is_better:
        return clamp((reference - value) / denominator)
    return clamp((value - reference) / denominator)


def default_action_impacts(name: str, risk: float, layer: str) -> list[dict[str, Any]]:
    key = metric_key(name)
    impacts = [{"action": "observe", "delta": 0.06, "confidence": 0.70}]
    if any(marker in key for marker in ("cpu", "memory", "queue", "lag", "pending", "saturation")):
        impacts.append({"action": "scale-up", "delta": 0.34, "confidence": 0.82})
    if any(marker in key for marker in ("latency", "queue", "error", "saturation")):
        impacts.append({"action": "shed-load", "delta": 0.28, "confidence": 0.74})
    if any(marker in key for marker in ("error", "failure", "restart", "crash")):
        impacts.append({"action": "restart", "delta": 0.24, "confidence": 0.72})
    if layer == "app" and any(marker in key for marker in ("latency", "error", "availability")):
        impacts.append({"action": "enable-fallback", "delta": 0.30, "confidence": 0.78})
        impacts.append({"action": "throttle-feature", "delta": 0.22, "confidence": 0.68})
    if risk >= 0.75:
        impacts.append({"action": "page-human", "delta": 0.38, "confidence": 0.88})
    return impacts


class OnlineTelemetryModel:
    def __init__(self, config: Config, metrics: Metrics) -> None:
        self.config = config
        self.metrics = metrics
        self._lock = threading.Lock()
        self._stats: dict[str, FeatureStats] = {}
        self._memory: dict[str, ServiceMemory] = {}
        self._transition_counts: dict[tuple[str, str, str], int] = {}

    def analyze(self, payload: dict[str, Any], source: str) -> dict[str, Any]:
        if not isinstance(payload, dict):
            raise ValueError("telemetry payload must be a JSON object")

        body = payload.get("telemetry") if isinstance(payload.get("telemetry"), dict) else payload
        request_id = normalize_token(
            body.get("requestId") or payload.get("requestId"),
            f"ml-{uuid.uuid4()}",
            MAX_REQUEST_ID_BYTES,
        )
        service = normalize_token(body.get("service") or payload.get("service"), "unknown-service")
        scope = infer_layer("scope", body.get("scope") or payload.get("scope"))
        window_ms = validate_window_ms(body.get("windowMs") or payload.get("windowMs"))
        action = normalize_action_name(
            metric_key(str(body.get("action") or body.get("lastAction") or "observe")).replace("_", "-")
        )

        raw_signals = self._extract_signals(body)
        if not raw_signals:
            raise ValueError("telemetry payload must include signals or metrics")
        if len(raw_signals) > MAX_SIGNALS:
            raise ValueError(f"telemetry payload must include at most {MAX_SIGNALS} signals")

        with self._lock:
            features = [self._score_signal(service, scope, signal) for signal in raw_signals]
            total_weight = sum(max(feature["weight"], 0.0) for feature in features)
            risk = (
                sum(feature["risk"] * max(feature["weight"], 0.0) for feature in features) / total_weight
                if total_weight > 0
                else 0.0
            )
            state = state_for_risk(risk)
            memory_key = f"{service}:{scope}"
            previous = self._memory.get(memory_key)
            if previous:
                transition_key = (previous.state, previous.action, state)
                if (
                    transition_key in self._transition_counts
                    or len(self._transition_counts) < self.config.max_transition_keys
                ):
                    self._transition_counts[transition_key] = (
                        self._transition_counts.get(transition_key, 0) + 1
                    )
                else:
                    self.metrics.inc("dropped_transitions_total")
            self._memory[memory_key] = ServiceMemory(state=state, action=action, risk=risk, seen_ms=now_ms())
            transition_estimates = self._transition_estimates()
            reward_estimate = self._reward_estimate(risk, action, previous)

        anomalies = [
            {
                "service": feature["service"],
                "layer": feature["layer"],
                "signal": feature["name"],
                "risk": feature["risk"],
                "zScore": feature["zScore"],
                "state": feature["state"],
                "reason": feature["reason"],
            }
            for feature in features
            if feature["risk"] >= 0.50 or abs(feature["zScore"]) >= 2.5
        ]
        anomalies.sort(key=lambda item: item["risk"], reverse=True)

        mdp_signals = [self._mdp_signal(feature) for feature in features]
        mdp_request = {
            "requestId": f"{request_id}:ml-features",
            "scope": scope,
            "windowMs": window_ms,
            "signals": mdp_signals,
            "actions": DEFAULT_ACTIONS,
            "gamma": 0.82,
            "tolerance": 1e-8,
            "maxIterations": 2000,
        }

        self.metrics.inc("analyses_total")
        self.metrics.inc("features_total", len(features))
        return {
            "ok": True,
            "requestId": request_id,
            "kind": "ml.telemetry-feature-pipeline",
            "source": source,
            "service": service,
            "scope": scope,
            "windowMs": window_ms,
            "state": state,
            "risk": risk,
            "recommendedNext": self._next_action(features, risk),
            "features": features,
            "anomalies": anomalies,
            "rewardEstimate": reward_estimate,
            "transitionModel": transition_estimates,
            "mdpTelemetry": mdp_request,
            "published": {"features": False, "mdp": False, "events": False},
            "generatedAtMs": now_ms(),
        }

    def _extract_signals(self, body: dict[str, Any]) -> list[dict[str, Any]]:
        signals: list[dict[str, Any]] = []
        raw_signals = body.get("signals")
        if isinstance(raw_signals, list):
            for item in raw_signals:
                if isinstance(item, dict):
                    signals.append(dict(item))
        metrics = body.get("metrics") or body.get("values")
        if isinstance(metrics, dict):
            for key, value in metrics.items():
                if isinstance(value, dict):
                    signal = dict(value)
                    signal.setdefault("name", key)
                else:
                    signal = {"name": key, "value": value}
                signals.append(signal)
        return signals

    def _score_signal(self, service: str, scope: str, signal: dict[str, Any]) -> dict[str, Any]:
        name = normalize_token(signal.get("name"), "unknown")
        value = finite_float(signal.get("value"), f"signal {name} value")
        layer = infer_layer(name, signal.get("layer") or scope)
        key = f"{service}:{layer}:{metric_key(name)}"
        stats = self._stats.get(key)
        if stats is None:
            if len(self._stats) < self.config.max_tracked_series:
                stats = FeatureStats()
                self._stats[key] = stats
            else:
                self.metrics.inc("dropped_series_total")
                stats = FeatureStats()
        defaults = threshold_defaults(name)
        higher_is_better = bool(signal.get("higherIsBetter", defaults.get("higherIsBetter", False)))
        baseline = optional_float(signal.get("baseline"), f"signal {name} baseline")
        if baseline is None:
            baseline = stats.ewma if stats.ewma is not None else (stats.mean if stats.count else None)
        target = optional_float(signal.get("target", defaults.get("target")), f"signal {name} target")
        warning = optional_float(signal.get("warning", defaults.get("warning")), f"signal {name} warning")
        critical = optional_float(signal.get("critical", defaults.get("critical")), f"signal {name} critical")
        weight = validate_weight(signal.get("weight", defaults.get("weight", 1.0)), f"signal {name} weight")

        threshold_risk = risk_from_thresholds(value, warning, critical, target, baseline, higher_is_better)
        z_score = 0.0
        z_risk = 0.0
        if stats.count >= self.config.min_samples_for_zscore and stats.stddev > 1e-9:
            z_score = (value - stats.mean) / stats.stddev
            directional_z = -z_score if higher_is_better else z_score
            z_risk = clamp((directional_z - 1.0) / 3.0)

        risk = clamp(max(threshold_risk, z_risk))
        state = state_for_risk(risk)
        trend = None if stats.last_value is None else value - stats.last_value
        stats.observe(value, self.config.ewma_alpha)

        impacts = signal.get("actionImpacts")
        if impacts is None:
            impacts = default_action_impacts(name, risk, layer)
        else:
            impacts = normalize_action_impacts(impacts)

        return {
            "name": name,
            "service": normalize_token(signal.get("service"), service),
            "layer": "infra" if layer == "mixed" else layer,
            "value": value,
            "baseline": baseline,
            "target": target,
            "warning": warning,
            "critical": critical,
            "higherIsBetter": higher_is_better,
            "weight": weight,
            "risk": risk,
            "state": state,
            "zScore": z_score,
            "mean": stats.mean,
            "ewma": stats.ewma,
            "stddev": stats.stddev,
            "sampleCount": stats.count,
            "trend": trend,
            "actionImpacts": impacts,
            "reason": f"{name} maps to {state} risk at {risk:.3f}",
        }

    def _mdp_signal(self, feature: dict[str, Any]) -> dict[str, Any]:
        result = {
            "name": feature["name"],
            "service": feature["service"],
            "layer": feature["layer"],
            "value": feature["value"],
            "weight": feature["weight"],
            "higherIsBetter": feature["higherIsBetter"],
            "actionImpacts": feature["actionImpacts"],
        }
        for key in ("baseline", "target", "warning", "critical"):
            if feature.get(key) is not None:
                result[key] = feature[key]
        return result

    def _next_action(self, features: list[dict[str, Any]], risk: float) -> str:
        scores: dict[str, float] = {action: 0.0 for action in DEFAULT_ACTIONS}
        for feature in features:
            for impact in feature["actionImpacts"]:
                action = metric_key(str(impact.get("action", "observe"))).replace("_", "-")
                delta = finite_float(impact.get("delta", 0.0), "action impact delta")
                confidence = clamp(finite_float(impact.get("confidence", 1.0), "action impact confidence"))
                scores[action] = scores.get(action, 0.0) + delta * confidence * max(feature["risk"], 0.1)
        return max(scores, key=lambda action: scores[action] - action_cost(action) - risk * 0.03)

    def _reward_estimate(self, risk: float, action: str, previous: Optional[ServiceMemory]) -> float:
        improvement = 0.0 if previous is None else previous.risk - risk
        return (1.0 - risk) + improvement * 1.5 - action_cost(action)

    def _transition_estimates(self) -> list[dict[str, Any]]:
        totals: dict[tuple[str, str], int] = {}
        for previous_state, action, _next_state in self._transition_counts:
            totals[(previous_state, action)] = totals.get((previous_state, action), 0) + self._transition_counts[
                (previous_state, action, _next_state)
            ]
        estimates = []
        for (previous_state, action, next_state), count in self._transition_counts.items():
            total = totals[(previous_state, action)]
            estimates.append(
                {
                    "state": previous_state,
                    "action": action,
                    "nextState": next_state,
                    "count": count,
                    "probability": count / total if total else 0.0,
                }
            )
        estimates.sort(key=lambda item: (item["state"], item["action"], -item["probability"]))
        return estimates[:32]


def parse_nats_url(url: str) -> Optional[NatsTarget]:
    if not url:
        return None
    parsed = urlparse(url)
    if parsed.scheme != "nats" or not parsed.hostname:
        return None
    if parsed.username and parsed.password:
        return NatsTarget(parsed.hostname, parsed.port or 4222, parsed.username, parsed.password)
    if parsed.username:
        return NatsTarget(parsed.hostname, parsed.port or 4222, token=parsed.username)
    return NatsTarget(parsed.hostname, parsed.port or 4222)


def redact_url(url: str) -> str:
    parsed = urlparse(url)
    if not parsed.username and not parsed.password:
        return url
    host = parsed.hostname or ""
    if parsed.port:
        host = f"{host}:{parsed.port}"
    return f"{parsed.scheme}://<redacted>@{host}"


def nats_connect(target: NatsTarget) -> socket.socket:
    sock = socket.create_connection((target.host, target.port), timeout=10)
    sock.settimeout(30)
    connect = {
        "verbose": False,
        "pedantic": False,
        "lang": "python",
        "version": SERVICE_NAME,
    }
    if target.user and target.password:
        connect["user"] = target.user
        connect["pass"] = target.password
    elif target.token:
        connect["auth_token"] = target.token
    sock.sendall(f"CONNECT {json.dumps(connect, separators=(',', ':'))}\r\nPING\r\n".encode())
    return sock


def wait_for_nats_pong(stream: Any, sock: socket.socket) -> None:
    while True:
        line = stream.readline()
        if not line:
            raise OSError("nats connection closed before PONG")
        if line.startswith(b"PING"):
            sock.sendall(b"PONG\r\n")
            continue
        if line.startswith(b"-ERR"):
            raise OSError(line.decode("utf-8", "replace").strip())
        if line.startswith(b"INFO") or line.startswith(b"+OK"):
            continue
        if line.startswith(b"PONG"):
            return


class NatsPublisher:
    def __init__(self, config: Config, metrics: Metrics) -> None:
        self.config = config
        self.metrics = metrics

    def publish_json(self, subject: str, payload: dict[str, Any]) -> bool:
        target = parse_nats_url(self.config.nats_url)
        if target is None:
            return False
        body = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode()
        if len(body) > self.config.max_publish_bytes:
            self.metrics.inc("nats_publish_errors_total")
            print(
                f"nats publish rejected oversize payload subject={subject} bytes={len(body)}",
                flush=True,
            )
            return False
        try:
            with nats_connect(target) as sock:
                stream = sock.makefile("rb")
                wait_for_nats_pong(stream, sock)
                command = f"PUB {subject} {len(body)}\r\n".encode()
                sock.sendall(command + body + b"\r\nPING\r\n")
                wait_for_nats_pong(stream, sock)
            return True
        except OSError as error:
            self.metrics.inc("nats_publish_errors_total")
            print(f"nats publish failed subject={subject}: {error}", flush=True)
            return False


class PipelineApp:
    def __init__(self, config: Config) -> None:
        self.config = config
        self.metrics = Metrics()
        self.model = OnlineTelemetryModel(config, self.metrics)
        self.publisher = NatsPublisher(config, self.metrics)
        self._nats_thread: Optional[threading.Thread] = None

    def descriptor(self) -> dict[str, Any]:
        return {
            "service": SERVICE_NAME,
            "kind": "python3.ai-ml-data-pipeline",
            "description": "Online telemetry feature engineering, anomaly scoring, and MDP feature publishing.",
            "endpoints": {
                "ingest": "POST /ingest",
                "analyze": "POST /analyze",
                "mdpFeatures": "POST /mdp/features",
                "status": "GET /status",
                "healthz": "GET /healthz",
                "readyz": "GET /readyz",
                "metrics": "GET /metrics",
            },
            "dataFlow": [
                self.config.raw_subject,
                "online EWMA/z-score feature model",
                self.config.feature_subject,
                self.config.mdp_subject,
                "dd-mdp-optimizer",
            ],
            "nats": {
                "url": redact_url(self.config.nats_url),
                "rawSubject": self.config.raw_subject,
                "queueGroup": self.config.queue_group,
                "featureSubject": self.config.feature_subject,
                "mdpTelemetrySubject": self.config.mdp_subject,
                "eventSubject": self.config.event_subject,
                "deadLetterSubject": self.config.dead_letter_subject,
            },
            "authRequired": bool(self.config.server_auth_secret),
        }

    def readiness(self) -> dict[str, Any]:
        auth_ready = bool(self.config.server_auth_secret) or self.config.allow_unauthenticated
        nats_ready = parse_nats_url(self.config.nats_url) is not None
        return {
            "ok": auth_ready and nats_ready,
            "service": SERVICE_NAME,
            "authReady": auth_ready,
            "natsConfigured": nats_ready,
            "generatedAtMs": now_ms(),
        }

    def status(self) -> dict[str, Any]:
        return {
            "ok": True,
            "service": SERVICE_NAME,
            "natsConfigured": parse_nats_url(self.config.nats_url) is not None,
            "natsUrl": redact_url(self.config.nats_url),
            "authRequired": bool(self.config.server_auth_secret),
            "metrics": self.metrics.snapshot(),
            "generatedAtMs": now_ms(),
        }

    def is_http_authorized(self, headers: Any) -> bool:
        if not self.config.server_auth_secret:
            return self.config.allow_unauthenticated
        values = []
        for name in ("X-Server-Auth", "Auth"):
            values.extend(headers.get_all(name, []))
        return any(
            constant_time_equals(value, self.config.server_auth_secret)
            for value in values
        )

    def analyze(self, payload: dict[str, Any], source: str, publish: bool) -> dict[str, Any]:
        result = self.model.analyze(payload, source)
        if publish:
            feature_event = {
                "type": "ml.features",
                "requestId": result["requestId"],
                "service": result["service"],
                "scope": result["scope"],
                "risk": result["risk"],
                "state": result["state"],
                "features": result["features"],
                "anomalies": result["anomalies"],
                "generatedAtMs": result["generatedAtMs"],
            }
            result["published"]["features"] = self.publisher.publish_json(
                self.config.feature_subject, feature_event
            )
            result["published"]["mdp"] = self.publisher.publish_json(
                self.config.mdp_subject, result["mdpTelemetry"]
            )
            runtime_event = {
                "type": "ml.pipeline.analyzed",
                "service": SERVICE_NAME,
                "requestId": result["requestId"],
                "risk": result["risk"],
                "state": result["state"],
                "recommendedNext": result["recommendedNext"],
                "generatedAtMs": result["generatedAtMs"],
            }
            result["published"]["events"] = self.publisher.publish_json(
                self.config.event_subject, runtime_event
            )
            if result["published"]["features"]:
                self.metrics.inc("published_features_total")
            if result["published"]["mdp"]:
                self.metrics.inc("published_mdp_total")
            if result["published"]["events"]:
                self.metrics.inc("published_events_total")
        return result

    def publish_dead_letter(self, reason: str, error: str, byte_length: int) -> None:
        self.metrics.inc("dead_letters_total")
        payload = {
            "type": "ml.pipeline.dead-letter",
            "service": SERVICE_NAME,
            "reason": reason,
            "error": error[:300],
            "byteLength": byte_length,
            "generatedAtMs": now_ms(),
        }
        if self.publisher.publish_json(self.config.dead_letter_subject, payload):
            self.metrics.inc("published_dead_letters_total")

    def start_nats_consumer(self) -> None:
        if parse_nats_url(self.config.nats_url) is None:
            print("ai/ml pipeline nats loop disabled: NATS_URL is not configured", flush=True)
            return
        self._nats_thread = threading.Thread(target=self._run_nats_loop, daemon=True)
        self._nats_thread.start()

    def _run_nats_loop(self) -> None:
        while True:
            try:
                self._subscribe_once()
            except Exception as error:  # noqa: BLE001 - top-level service guard
                print(f"ai/ml pipeline nats loop error: {error}", flush=True)
                time.sleep(5)

    def _subscribe_once(self) -> None:
        target = parse_nats_url(self.config.nats_url)
        if target is None:
            time.sleep(30)
            return
        print(
            "ai/ml pipeline nats loop starting: "
            f"subject={self.config.raw_subject} queue_group={self.config.queue_group}",
            flush=True,
        )
        with nats_connect(target) as sock:
            stream = sock.makefile("rb")
            sid = "1"
            sock.sendall(
                f"SUB {self.config.raw_subject} {self.config.queue_group} {sid}\r\n".encode()
            )
            while True:
                line = stream.readline()
                if not line:
                    raise EOFError("nats connection closed")
                if line.startswith(b"PING"):
                    sock.sendall(b"PONG\r\n")
                    continue
                if line.startswith(b"+OK") or line.startswith(b"INFO") or line.startswith(b"PONG"):
                    continue
                if not line.startswith(b"MSG "):
                    continue
                parts = line.decode("utf-8", "replace").strip().split()
                size = int(parts[-1])
                if size > MAX_BODY_BYTES:
                    stream.read(size + 2)
                    self.metrics.inc("errors_total")
                    self.publish_dead_letter("oversize-nats-message", "payload exceeded ML_MAX_BODY_BYTES", size)
                    print(f"rejected oversize nats telemetry payload bytes={size}", flush=True)
                    continue
                body = stream.read(size)
                stream.read(2)
                self.metrics.inc("nats_messages_total")
                try:
                    payload = json.loads(body)
                    self.analyze(payload, source="nats", publish=True)
                except Exception as error:  # noqa: BLE001 - reject one bad message, keep loop alive
                    self.metrics.inc("errors_total")
                    self.publish_dead_letter("invalid-nats-message", str(error), len(body))
                    print(f"invalid nats telemetry message: {error}", flush=True)


class RuntimeConfigClient:
    """Process-wide receiver for dd-runtime-config push messages.

    Mirrors `remote/libs/runtime-config-client-rs` and the equivalent Node
    helper. Backed by an in-memory dict guarded by a threading.RLock so the
    snapshot can be served and mutated from the stdlib ThreadingHTTPServer
    handler pool without races. Registration with the control plane runs on
    a daemon thread that retries with exponential backoff (15s → 5min cap).
    """

    SNAPSHOT_ROUTE = "/internal/runtime-config"
    APPLY_ROUTE = "/internal/update-runtime-config"
    RESET_ROUTE = "/internal/runtime-config/reset"

    _REGISTER_BACKOFF_SECONDS = 15.0
    _REGISTER_MAX_BACKOFF_SECONDS = 5 * 60.0
    _REGISTER_TIMEOUT_SECONDS = 10.0

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._snapshot_version: int = 0
        self._entries: dict[str, Any] = {}
        self._applied_at: Optional[str] = None
        self._last_push_id: Optional[str] = None
        self._last_reason: Optional[str] = None
        self._server_secret = os.getenv("RUNTIME_CONFIG_SERVER_SECRET", "").strip() or None
        self._allow_unauthenticated = _runtime_config_bool_env("RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED")
        self._service_name = os.getenv("RUNTIME_CONFIG_SERVICE_NAME", "").strip() or SERVICE_NAME
        self._scope = os.getenv("RUNTIME_CONFIG_SCOPE", "").strip() or self._service_name
        self._env_label = (os.getenv("RUNTIME_CONFIG_ENV") or "stage").strip() or "stage"
        self._register_url = os.getenv("RUNTIME_CONFIG_REGISTER_URL", "").strip() or None
        self._apply_url = os.getenv("RUNTIME_CONFIG_APPLY_URL", "").strip() or None

    @property
    def snapshot_version(self) -> int:
        with self._lock:
            return self._snapshot_version

    def get(self, key: str, default: Any = None) -> Any:
        with self._lock:
            return self._entries.get(key, default)

    def snapshot(self) -> dict[str, Any]:
        with self._lock:
            return {
                "service": self._service_name,
                "scope": self._scope,
                "env": self._env_label,
                "snapshotVersion": self._snapshot_version,
                "appliedAt": self._applied_at,
                "entries": dict(self._entries),
                "lastPushId": self._last_push_id,
                "lastReason": self._last_reason,
            }

    def reset(self) -> None:
        with self._lock:
            self._snapshot_version = 0
            self._entries = {}
            self._applied_at = None
            self._last_push_id = None
            self._last_reason = None

    def apply(self, payload: Any) -> dict[str, Any]:
        if not isinstance(payload, dict):
            raise ValueError("apply payload must be a JSON object")
        snapshot = payload.get("snapshot")
        if not isinstance(snapshot, dict):
            raise ValueError("snapshot is required")
        entries_in: list[Any] = []
        applied_version = 0
        raw_version = snapshot.get("snapshotVersion")
        if isinstance(raw_version, int):
            applied_version = raw_version
        entries_field = snapshot.get("entries")
        if isinstance(entries_field, list):
            entries_in = entries_field
        new_entries: dict[str, Any] = {}
        for entry in entries_in:
            if isinstance(entry, dict) and isinstance(entry.get("key"), str):
                new_entries[entry["key"]] = entry.get("value")
        applied_at = _iso_now()
        push_id = payload.get("pushId") if isinstance(payload.get("pushId"), str) else None
        reason = payload.get("reason") if isinstance(payload.get("reason"), str) else None
        with self._lock:
            previous_version = self._snapshot_version
            if applied_version < previous_version:
                return {
                    "ok": True,
                    "service": self._service_name,
                    "appliedAt": self._applied_at or applied_at,
                    "appliedVersion": previous_version,
                    "previousVersion": previous_version,
                    "stale": True,
                    "ignoredVersion": applied_version,
                }
            self._snapshot_version = applied_version
            self._entries = new_entries
            self._applied_at = applied_at
            self._last_push_id = push_id
            self._last_reason = reason
        return {
            "ok": True,
            "service": self._service_name,
            "appliedAt": applied_at,
            "appliedVersion": applied_version,
            "previousVersion": previous_version,
        }

    def check_server_auth(self, header_value: Optional[str]) -> bool:
        if self._server_secret is None:
            return self._allow_unauthenticated
        if not header_value:
            return False
        return compare_digest(header_value, self._server_secret)

    def start_registration_thread(self) -> None:
        if not self._register_url or not self._apply_url:
            print(
                "[runtime-config] RUNTIME_CONFIG_REGISTER_URL or _APPLY_URL not set; skipping registration",
                flush=True,
            )
            return
        thread = threading.Thread(
            target=self._register_loop,
            name="dd-runtime-config-register",
            daemon=True,
        )
        thread.start()

    def _register_loop(self) -> None:
        assert self._register_url is not None
        assert self._apply_url is not None
        body = json.dumps(
            {
                "env": self._env_label,
                "name": self._service_name,
                "scope": self._scope,
                "applyUrl": self._apply_url,
            }
        ).encode("utf-8")
        headers = {"content-type": "application/json"}
        if self._server_secret:
            headers["x-server-auth"] = self._server_secret
        delay = self._REGISTER_BACKOFF_SECONDS
        while True:
            try:
                request_obj = urllib.request.Request(
                    self._register_url,
                    data=body,
                    headers=headers,
                    method="POST",
                )
                with urllib.request.urlopen(
                    request_obj, timeout=self._REGISTER_TIMEOUT_SECONDS
                ) as response:
                    if 200 <= response.status < 300:
                        print(
                            f"[runtime-config] registered with control plane at {self._register_url}",
                            flush=True,
                        )
                        return
                    print(
                        f"[runtime-config] register returned HTTP {response.status}; retrying in {int(delay)}s",
                        flush=True,
                    )
            except urllib.error.HTTPError as error:
                print(
                    f"[runtime-config] register returned HTTP {error.code}; retrying in {int(delay)}s",
                    flush=True,
                )
            except (urllib.error.URLError, OSError, ConnectionError) as error:
                print(
                    f"[runtime-config] register transport error: {error}; retrying in {int(delay)}s",
                    flush=True,
                )
            time.sleep(delay)
            delay = min(delay * 2.0, self._REGISTER_MAX_BACKOFF_SECONDS)


RUNTIME_CONFIG_PATHS = frozenset(
    {
        RuntimeConfigClient.SNAPSHOT_ROUTE,
        RuntimeConfigClient.APPLY_ROUTE,
        RuntimeConfigClient.RESET_ROUTE,
    }
)


def _iso_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".000Z"


def _runtime_config_bool_env(name: str) -> bool:
    return (os.getenv(name) or "").strip() in {"1", "true", "TRUE", "yes", "YES"}


class PipelineHTTPServer(ThreadingHTTPServer):
    def __init__(self, server_address: tuple[str, int], handler: type[BaseHTTPRequestHandler], app: PipelineApp) -> None:
        super().__init__(server_address, handler)
        self.app = app
        self.runtime_config = RuntimeConfigClient()


@instrument_handler
class Handler(BaseHTTPRequestHandler):
    server: PipelineHTTPServer
    server_version = SERVICE_NAME
    sys_version = ""

    def setup(self) -> None:
        super().setup()
        self.connection.settimeout(15)

    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"{self.address_string()} {fmt % args}", flush=True)

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        self.server.app.metrics.inc("requests_total")
        path = self._normalized_path()
        if path == RuntimeConfigClient.SNAPSHOT_ROUTE:
            self._json(HTTPStatus.OK, self.server.runtime_config.snapshot())
            return
        if path in {"/", ""}:
            if not self._is_authorized():
                return
            self._json(HTTPStatus.OK, self.server.app.descriptor())
        elif path == "/healthz":
            self._json(HTTPStatus.OK, {"ok": True, "service": SERVICE_NAME})
        elif path == "/readyz":
            readiness = self.server.app.readiness()
            status = HTTPStatus.OK if readiness["ok"] else HTTPStatus.SERVICE_UNAVAILABLE
            self._json(status, readiness)
        elif path == "/status":
            if not self._is_authorized():
                return
            self._json(HTTPStatus.OK, self.server.app.status())
        elif path == "/metrics":
            self._text(HTTPStatus.OK, self.server.app.metrics.prometheus(), "text/plain; version=0.0.4")
        elif path in {"/docs/api", "/api/docs"}:
            self._text(
                HTTPStatus.OK,
                (GENERATED_DOCS_DIR / "api-docs.html").read_text(encoding="utf-8"),
                "text/html; charset=utf-8",
            )
        elif path == "/api/docs.json":
            self._text(
                HTTPStatus.OK,
                (GENERATED_DOCS_DIR / "api-docs.json").read_text(encoding="utf-8"),
                "application/json; charset=utf-8",
            )
        else:
            self._json(HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})

    def do_POST(self) -> None:  # noqa: N802 - stdlib handler API
        self.server.app.metrics.inc("requests_total")
        path = self._normalized_path()
        if path in RUNTIME_CONFIG_PATHS:
            self._handle_runtime_config_post(path)
            return
        try:
            if not self._is_authorized():
                return
            payload = self._read_json()
            if path == "/ingest":
                self.server.app.metrics.inc("ingest_requests_total")
                self._json(HTTPStatus.OK, self.server.app.analyze(payload, source="http", publish=True))
            elif path == "/analyze":
                self._json(HTTPStatus.OK, self.server.app.analyze(payload, source="http", publish=False))
            elif path == "/mdp/features":
                result = self.server.app.analyze(payload, source="http", publish=False)
                self._json(HTTPStatus.OK, result["mdpTelemetry"])
            else:
                self._json(HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})
        except ValueError as error:
            self.server.app.metrics.inc("errors_total")
            self._json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": str(error)})
        except Exception as error:  # noqa: BLE001 - HTTP boundary
            self.server.app.metrics.inc("errors_total")
            print(f"internal http error path={path}: {error}", flush=True)
            self._json(HTTPStatus.INTERNAL_SERVER_ERROR, {"ok": False, "error": "internal server error"})

    def _normalized_path(self) -> str:
        path = self.path.split("?", 1)[0]
        if path == "/ml":
            return "/"
        if path.startswith("/ml/"):
            return path[3:]
        return path

    def _handle_runtime_config_post(self, path: str) -> None:
        client = self.server.runtime_config
        if not client.check_server_auth(self.headers.get("x-server-auth")):
            self._json(HTTPStatus.UNAUTHORIZED, {"ok": False, "error": "unauthorized"})
            return
        if path == RuntimeConfigClient.RESET_ROUTE:
            client.reset()
            self._json(HTTPStatus.OK, {"ok": True})
            return
        try:
            payload = self._read_json()
        except ValueError as error:
            self._json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": str(error)})
            return
        try:
            response = client.apply(payload)
        except ValueError as error:
            self._json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": str(error)})
            return
        self._json(HTTPStatus.OK, response)

    def _read_json(self) -> dict[str, Any]:
        content_type = self.headers.get("Content-Type", "")
        if content_type and "json" not in content_type.lower():
            raise ValueError("Content-Type must be application/json")
        raw_length = self.headers.get("Content-Length")
        if raw_length is None:
            raise ValueError("Content-Length is required")
        length = int(raw_length)
        if length < 0:
            raise ValueError("Content-Length must be non-negative")
        if length > MAX_BODY_BYTES:
            raise ValueError(f"request body must be at most {MAX_BODY_BYTES} bytes")
        body = self.rfile.read(length)
        try:
            payload = json.loads(body)
        except json.JSONDecodeError as error:
            raise ValueError("request body must be valid JSON") from error
        if not isinstance(payload, dict):
            raise ValueError("request body must be a JSON object")
        return payload

    def _is_authorized(self) -> bool:
        if self.server.app.is_http_authorized(self.headers):
            return True
        self.server.app.metrics.inc("auth_failures_total")
        self._json(HTTPStatus.UNAUTHORIZED, {"ok": False, "error": "unauthorized"})
        return False

    def _json(self, status: HTTPStatus, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, indent=2, sort_keys=True).encode()
        self.send_response(status.value)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _text(self, status: HTTPStatus, body: str, content_type: str) -> None:
        encoded = body.encode()
        self.send_response(status.value)
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


def main() -> None:
    init_telemetry(SERVICE_NAME)
    config = Config()
    if not config.server_auth_secret and not config.allow_unauthenticated:
        raise RuntimeError("SERVER_AUTH_SECRET is required unless ML_ALLOW_UNAUTHENTICATED=true")
    if not config.server_auth_secret and config.allow_unauthenticated:
        print(
            "ML_ALLOW_UNAUTHENTICATED=true; HTTP analysis endpoints will accept unauthenticated requests",
            flush=True,
        )
    app = PipelineApp(config)
    app.start_nats_consumer()
    server = PipelineHTTPServer((config.host, config.port), Handler, app)
    server.runtime_config.start_registration_thread()
    print(f"{SERVICE_NAME} listening on {config.host}:{config.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
