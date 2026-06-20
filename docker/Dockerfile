# syntax=docker/dockerfile:1

FROM python:3.12-slim AS builder
COPY --from=ghcr.io/astral-sh/uv:latest /uv /bin/uv
ENV UV_COMPILE_BYTECODE=1 \
    UV_LINK_MODE=copy \
    UV_PYTHON_DOWNLOADS=0
WORKDIR /app

# Resolve third-party deps first so this layer caches unless the lock changes.
COPY pyproject.toml uv.lock README.md ./
RUN uv sync --frozen --no-dev --no-install-project

# Install the project itself (editable) once sources are present.
COPY src ./src
RUN uv sync --frozen --no-dev

FROM python:3.12-slim AS runtime
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PATH="/app/.venv/bin:$PATH"
RUN useradd --create-home --uid 1000 appuser
WORKDIR /app

# The editable install records /app/src in a .pth — copy it to the same path.
COPY --from=builder --chown=appuser:appuser /app/.venv /app/.venv
COPY --from=builder --chown=appuser:appuser /app/src /app/src

USER appuser
EXPOSE 8000

# CMD (not ENTRYPOINT): default run starts the API, but `docker run <img> code-atlas --help` overrides cleanly.
CMD ["uvicorn", "code_atlas.api.app:app", "--host", "0.0.0.0", "--port", "8000"]
