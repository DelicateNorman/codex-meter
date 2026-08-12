from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path

from codex_meter.models import ParsedSession


@dataclass(frozen=True, slots=True)
class CollectorCapabilities:
    sessions: bool = False
    turns: bool = False
    per_llm_call: bool = False
    token_usage: bool = False
    cache_write: bool = False
    latency: bool = False
    tools: bool = False
    exact_usage: bool = False


class Collector(ABC):
    @property
    @abstractmethod
    def name(self) -> str: ...

    @property
    @abstractmethod
    def capabilities(self) -> CollectorCapabilities: ...

    @abstractmethod
    def collect_file(self, path: Path) -> ParsedSession: ...
