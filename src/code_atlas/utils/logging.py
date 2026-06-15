"""Structured logging configuration for code-atlas."""

from __future__ import annotations

import logging
import sys
from typing import TYPE_CHECKING, cast

import structlog

if TYPE_CHECKING:
    from typing import TextIO

__all__ = ["configure_logging", "get_logger"]


def configure_logging(level: str = "INFO", *, json: bool = True, stream: TextIO | None = None) -> None:
    """Configure structlog with JSON or console output; idempotent across calls."""
    resolved_level = logging.getLevelNamesMapping().get(level.upper(), logging.INFO)
    target = stream or sys.stderr

    root = logging.getLogger()
    root.setLevel(resolved_level)
    for handler in list(root.handlers):
        root.removeHandler(handler)
    root.addHandler(logging.StreamHandler(target))

    processors: list[structlog.types.Processor] = [
        structlog.contextvars.merge_contextvars,
        structlog.processors.add_log_level,
        structlog.processors.StackInfoRenderer(),
    ]
    if not json:
        processors.append(structlog.dev.set_exc_info)
    processors.append(structlog.processors.TimeStamper(fmt="iso", utc=True))
    if json:
        processors.append(structlog.processors.JSONRenderer())
    else:
        processors.append(structlog.dev.ConsoleRenderer(colors=False))

    structlog.configure(
        processors=processors,
        wrapper_class=structlog.make_filtering_bound_logger(resolved_level),
        logger_factory=structlog.PrintLoggerFactory(file=target),
        cache_logger_on_first_use=True,
    )


def get_logger(name: str | None = None) -> structlog.stdlib.BoundLogger:
    """Return a structlog bound logger; configure via `configure_logging` first."""
    return cast("structlog.stdlib.BoundLogger", structlog.get_logger(name))
