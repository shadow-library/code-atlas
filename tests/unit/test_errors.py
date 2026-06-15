"""Tests for the typed exception hierarchy."""

from __future__ import annotations

import pytest

from code_atlas.errors import (
    AgentError,
    CodeAtlasError,
    ConfigError,
    EvaluationError,
    IndexingError,
    IngestionError,
    ProviderError,
    RepositoryNotIndexed,
    RetrievalError,
)


def test_base_error_str_without_context() -> None:
    e = CodeAtlasError("boom")
    assert str(e) == "boom"


def test_base_error_str_with_context() -> None:
    e = CodeAtlasError("boom", context={"file": "x.py", "line": 7})
    rendered = str(e)
    assert "file" in rendered
    assert "x.py" in rendered


def test_repr_shape() -> None:
    e = CodeAtlasError("boom", context={"k": 1})
    rendered = repr(e)
    assert "CodeAtlasError" in rendered
    assert "boom" in rendered


def test_repository_not_indexed_is_indexing_error() -> None:
    e = RepositoryNotIndexed("missing")
    assert isinstance(e, IndexingError)
    assert isinstance(e, CodeAtlasError)


def test_all_subclasses_inherit_base() -> None:
    subclasses = (
        ConfigError,
        IngestionError,
        IndexingError,
        RepositoryNotIndexed,
        ProviderError,
        RetrievalError,
        AgentError,
        EvaluationError,
    )
    for cls in subclasses:
        assert issubclass(cls, CodeAtlasError)


def test_context_survives_raise_except() -> None:
    with pytest.raises(ProviderError) as exc_info:
        raise ProviderError("rate limited", context={"provider": "ollama", "retry_after": 30})
    e = exc_info.value
    assert e.context["provider"] == "ollama"
    assert e.context["retry_after"] == 30


def test_context_defaults_to_empty_dict() -> None:
    e1 = CodeAtlasError("a")
    e2 = CodeAtlasError("b")
    assert e1.context == {}
    assert e2.context == {}
    e1.context["x"] = 1
    assert e2.context == {}


def test_str_includes_repr_of_context() -> None:
    e = CodeAtlasError("boom", context={"n": 42, "s": "héllo"})
    rendered = str(e)
    assert repr({"n": 42, "s": "héllo"}) in rendered
