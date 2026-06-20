"""Citation grounding: verify each cited (file, line range, snippet) exists in the indexed source."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pydantic import BaseModel, ConfigDict

from code_atlas.domain.answer import Citation

if TYPE_CHECKING:
    from code_atlas.domain.answer import Answer
    from code_atlas.indexing.metadata_store import MetadataStore

__all__ = ["GroundingReport", "UngroundedCitation", "check_grounding"]


class UngroundedCitation(BaseModel):
    """A citation that failed one or more grounding checks, with the reasons it failed."""

    model_config = ConfigDict(frozen=True)

    citation: Citation
    file_exists: bool
    line_range_valid: bool
    snippet_present: bool
    reasons: list[str]


class GroundingReport(BaseModel):
    """Aggregate grounding outcome over an answer's citations."""

    model_config = ConfigDict(frozen=True)

    total: int
    grounded: int
    ungrounded_citations: list[UngroundedCitation]

    @property
    def is_fully_grounded(self) -> bool:
        return self.total == self.grounded


def check_grounding(answer: Answer, metadata_store: MetadataStore, *, repo_id: str) -> GroundingReport:
    """Verify every citation points at indexed source: file exists, line range is covered, snippet appears."""
    grounded = 0
    ungrounded: list[UngroundedCitation] = []

    for citation in answer.citations:
        chunks = metadata_store.find_by_path(repo_id, citation.path)
        file_exists = bool(chunks)
        containing = [c for c in chunks if c.start_line <= citation.start_line and c.end_line >= citation.end_line]
        line_range_valid = bool(containing)
        # Empty snippet is vacuously present: there is nothing the answer could have fabricated.
        snippet_present = True if citation.snippet == "" else any(citation.snippet in c.content for c in containing)

        if file_exists and line_range_valid and snippet_present:
            grounded += 1
            continue

        reasons: list[str] = []
        if not file_exists:
            reasons.append("file not indexed")
        if not line_range_valid:
            reasons.append("line range outside known chunks")
        if not snippet_present:
            reasons.append("snippet not found in cited chunk")
        ungrounded.append(
            UngroundedCitation(
                citation=citation,
                file_exists=file_exists,
                line_range_valid=line_range_valid,
                snippet_present=snippet_present,
                reasons=reasons,
            )
        )

    return GroundingReport(total=len(answer.citations), grounded=grounded, ungrounded_citations=ungrounded)
