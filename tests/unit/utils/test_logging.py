"""Tests for structlog configuration."""

from __future__ import annotations

import io
import json

import pytest

from code_atlas.utils.logging import configure_logging, get_logger


def test_json_output_is_parseable() -> None:
    buf = io.StringIO()
    configure_logging(level="INFO", json=True, stream=buf)
    get_logger("t").info("hello", key="value")
    record = json.loads(buf.getvalue())
    assert record["event"] == "hello"
    assert record["key"] == "value"
    assert record["level"] == "info"


def test_console_output_is_human_readable() -> None:
    buf = io.StringIO()
    configure_logging(level="INFO", json=False, stream=buf)
    get_logger("t").info("hello", key="value")
    output = buf.getvalue()
    assert "hello" in output
    assert "key" in output
    with pytest.raises(json.JSONDecodeError):
        json.loads(output)


def test_iso_timestamp_in_json() -> None:
    buf = io.StringIO()
    configure_logging(level="INFO", json=True, stream=buf)
    get_logger("t").info("ping")
    record = json.loads(buf.getvalue())
    assert "timestamp" in record
    ts = record["timestamp"]
    assert len(ts) >= 19 and ts[:2] in {"19", "20"}


def test_get_logger_returns_bound_logger() -> None:
    configure_logging(level="INFO", json=True, stream=io.StringIO())
    log = get_logger("x")
    assert hasattr(log, "info")
    assert hasattr(log, "bind")


def test_level_filter_drops_below_threshold() -> None:
    buf = io.StringIO()
    configure_logging(level="WARNING", json=True, stream=buf)
    log = get_logger("t")
    log.info("low")
    log.warning("high")
    lines = [line for line in buf.getvalue().splitlines() if line.strip()]
    assert len(lines) == 1
    assert "high" in lines[0]
    assert "low" not in lines[0]


def test_idempotent_reconfigure() -> None:
    first = io.StringIO()
    second = io.StringIO()
    configure_logging(level="INFO", json=True, stream=first)
    configure_logging(level="INFO", json=False, stream=second)
    get_logger("t").info("after")
    assert first.getvalue() == ""
    assert "after" in second.getvalue()
