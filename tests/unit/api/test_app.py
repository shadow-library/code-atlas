"""Offline API tests: dependency overrides keep the lifespan and real I/O out of scope."""

from __future__ import annotations

from collections.abc import AsyncIterator, Iterator

import pytest
from fastapi.testclient import TestClient

from code_atlas import __version__
from code_atlas.agent.qa import StreamEvent
from code_atlas.api.app import app
from code_atlas.api.routes import get_agent_factory, get_ingest_runner
from code_atlas.domain.answer import Answer, Citation

_FAKE_TEXT = "the handler lives in routes"


class FakeAgent:
    """A QAAgent stand-in returning a canned grounded answer."""

    async def ask(self, question: str) -> Answer:
        citation = Citation(
            path="src/code_atlas/api/routes.py", start_line=1, end_line=2, snippet="router = APIRouter()"
        )
        return Answer(text=_FAKE_TEXT, citations=[citation])

    async def ask_stream(self, question: str) -> AsyncIterator[StreamEvent]:
        answer = await self.ask(question)
        for token in answer.text.split():
            yield StreamEvent(type="token", text=token)
        yield StreamEvent(type="done", answer=answer)


@pytest.fixture
def ingest_calls() -> Iterator[list[tuple[str, str]]]:
    """Record (repo_path, repo_id) pairs the ingest runner is invoked with."""
    calls: list[tuple[str, str]] = []
    yield calls


@pytest.fixture
def client(ingest_calls: list[tuple[str, str]]) -> Iterator[TestClient]:
    """A TestClient with both real-I/O dependencies overridden; no lifespan runs."""
    fake_agent = FakeAgent()
    app.dependency_overrides[get_agent_factory] = lambda: lambda repo_id: fake_agent
    app.dependency_overrides[get_ingest_runner] = lambda: (
        lambda repo_path, repo_id: ingest_calls.append((repo_path, repo_id))
    )
    # No `with`: the lifespan (which opens real stores) must not run.
    yield TestClient(app)
    app.dependency_overrides.clear()


def test_health(client: TestClient) -> None:
    response = client.get("/health")
    assert response.status_code == 200
    assert response.json() == {"status": "ok", "version": __version__}


def test_ask_returns_answer(client: TestClient) -> None:
    response = client.post("/ask", json={"repo_id": "r", "question": "where?"})
    assert response.status_code == 200
    body = response.json()
    assert body["text"] == _FAKE_TEXT
    assert body["citations"]


def test_ingest_accepts_and_schedules(client: TestClient, ingest_calls: list[tuple[str, str]]) -> None:
    response = client.post("/ingest", json={"repo_path": "/tmp/x", "repo_id": "r"})
    assert response.status_code == 202
    body = response.json()
    assert body["job_id"]
    assert body["status"]
    # TestClient runs background tasks synchronously after the response is sent.
    assert ingest_calls == [("/tmp/x", "r")]


def test_ask_stream_sse(client: TestClient) -> None:
    response = client.get("/ask/stream", params={"repo_id": "r", "question": "where"})
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/event-stream")
    assert "data:" in response.text
    assert "event: done" in response.text


def test_ask_validation_error(client: TestClient) -> None:
    response = client.post("/ask", json={"repo_id": "r", "question": ""})
    assert response.status_code == 422
