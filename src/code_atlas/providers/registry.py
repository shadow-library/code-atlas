"""Name-keyed registry of embedding and LLM provider factories."""

from __future__ import annotations

from collections.abc import Callable

from code_atlas.config import Settings
from code_atlas.errors import ProviderError
from code_atlas.providers.base import EmbeddingProvider, LLMProvider
from code_atlas.utils import get_logger

log = get_logger(__name__)

__all__ = [
    "EmbeddingFactory",
    "LLMFactory",
    "clear_registry",
    "make_embedding",
    "make_llm",
    "register_embedding",
    "register_llm",
    "registered_embeddings",
    "registered_llms",
]

EmbeddingFactory = Callable[[Settings], EmbeddingProvider]
LLMFactory = Callable[[Settings], LLMProvider]

_EMBEDDINGS: dict[str, EmbeddingFactory] = {}
_LLMS: dict[str, LLMFactory] = {}


def _normalize(name: str) -> str:
    if not isinstance(name, str) or not name.strip():
        raise ProviderError("provider name must be a non-empty string", context={"name": name})
    return name.strip()


def register_embedding(name: str, factory: EmbeddingFactory) -> None:
    """Register an embedding factory under ``name``. Re-registration overwrites."""
    key = _normalize(name)
    _EMBEDDINGS[key] = factory
    log.info("provider.embedding_registered", name=key)


def register_llm(name: str, factory: LLMFactory) -> None:
    """Register an LLM factory under ``name``. Re-registration overwrites."""
    key = _normalize(name)
    _LLMS[key] = factory
    log.info("provider.llm_registered", name=key)


def make_embedding(settings: Settings) -> EmbeddingProvider:
    """Resolve the embedding provider named in ``settings.embeddings.provider``."""
    name = settings.embeddings.provider
    factory = _EMBEDDINGS.get(name)
    if factory is None:
        raise ProviderError(
            "embedding provider not registered",
            context={"name": name, "available": sorted(_EMBEDDINGS)},
        )
    try:
        return factory(settings)
    except ProviderError:
        raise
    except Exception as exc:
        log.warning("provider.factory_failed", kind="embedding", name=name, error=str(exc))
        raise ProviderError(
            "embedding factory failed",
            context={"name": name},
        ) from exc


def make_llm(settings: Settings) -> LLMProvider:
    """Resolve the LLM provider named in ``settings.chat.provider``."""
    name = settings.chat.provider
    factory = _LLMS.get(name)
    if factory is None:
        raise ProviderError(
            "llm provider not registered",
            context={"name": name, "available": sorted(_LLMS)},
        )
    try:
        return factory(settings)
    except ProviderError:
        raise
    except Exception as exc:
        log.warning("provider.factory_failed", kind="llm", name=name, error=str(exc))
        raise ProviderError(
            "llm factory failed",
            context={"name": name},
        ) from exc


def clear_registry() -> None:
    """Reset both registry maps. Intended for tests."""
    _EMBEDDINGS.clear()
    _LLMS.clear()


def registered_embeddings() -> list[str]:
    """Return sorted names of currently-registered embedding providers."""
    return sorted(_EMBEDDINGS)


def registered_llms() -> list[str]:
    """Return sorted names of currently-registered LLM providers."""
    return sorted(_LLMS)
