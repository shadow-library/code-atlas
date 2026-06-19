"""Providers subsystem: chat + embedding seams, plus a name-keyed registry."""

from code_atlas.providers.base import (
    ChatChunk,
    ChatMessage,
    ChatResponse,
    ChatRole,
    EmbeddingProvider,
    LLMProvider,
    ToolCall,
    ToolSpec,
)
from code_atlas.providers.ollama_embeddings import OllamaEmbeddingProvider
from code_atlas.providers.ollama_llm import OllamaLLMProvider
from code_atlas.providers.registry import (
    EmbeddingFactory,
    LLMFactory,
    clear_registry,
    make_embedding,
    make_llm,
    register_embedding,
    register_llm,
    registered_embeddings,
    registered_llms,
)

__all__ = [
    "ChatChunk",
    "ChatMessage",
    "ChatResponse",
    "ChatRole",
    "EmbeddingFactory",
    "EmbeddingProvider",
    "LLMFactory",
    "LLMProvider",
    "OllamaEmbeddingProvider",
    "OllamaLLMProvider",
    "ToolCall",
    "ToolSpec",
    "clear_registry",
    "make_embedding",
    "make_llm",
    "register_embedding",
    "register_llm",
    "registered_embeddings",
    "registered_llms",
]
