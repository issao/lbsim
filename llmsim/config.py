"""Scenario configuration.

TOML via the stdlib ``tomllib``, because this environment has no package manager
and TOML is typed, readable and diffable. Two features beyond plain parsing
matter for running experiments:

**Inheritance.** A scenario may declare ``extends = "base.toml"`` (a path
relative to its own directory) and override only what differs. A policy
comparison is then a handful of three-line files rather than copies of a large
config that drift apart.

**Overrides.** ``--set router.policy=p2c --set workload.rate_rps=40`` from the
command line, so a parameter sweep is a shell loop and no file is edited. The
resolved config is recorded with the results, so a run is always reproducible
from its own output.
"""

from __future__ import annotations

import copy
import tomllib
from pathlib import Path
from typing import Any, Iterable, Mapping

_MAX_EXTENDS_DEPTH = 16


class ConfigError(ValueError):
    """A config is missing a key, has a bad type, or has a broken `extends`."""


def deep_merge(base: Mapping[str, Any], over: Mapping[str, Any]) -> dict[str, Any]:
    """Recursive dict merge. ``over`` wins. Lists replace, they do not concatenate.

    Lists replace deliberately: if a base declares three replica pools and an
    override declares one, the intent is one pool, not four.
    """
    out = dict(copy.deepcopy(dict(base)))
    for key, value in over.items():
        prev = out.get(key)
        if isinstance(prev, dict) and isinstance(value, Mapping):
            out[key] = deep_merge(prev, value)
        else:
            out[key] = copy.deepcopy(value)
    return out


def _coerce(text: str) -> Any:
    """Parse a command-line override value using TOML's own value syntax.

    So ``x=1`` is an int, ``x=1.0`` a float, ``x=true`` a bool, ``x=[1,2]`` a
    list, and ``x=p2c`` falls back to a string. This avoids inventing a second,
    subtly different literal syntax.
    """
    try:
        return tomllib.loads(f"v = {text}")["v"]
    except tomllib.TOMLDecodeError:
        return text


class Config:
    """Read-only nested config with dotted access."""

    __slots__ = ("_data", "source")

    def __init__(self, data: Mapping[str, Any], source: str = "<memory>") -> None:
        self._data = copy.deepcopy(dict(data))
        self.source = source

    # -- construction --------------------------------------------------------

    @classmethod
    def load(
        cls, path: str | Path, overrides: Iterable[str] = (), _depth: int = 0
    ) -> "Config":
        """Load a TOML file, resolving ``extends``, then apply ``key=value`` overrides."""
        if _depth > _MAX_EXTENDS_DEPTH:
            raise ConfigError(f"`extends` chain deeper than {_MAX_EXTENDS_DEPTH}: {path}")
        p = Path(path)
        if not p.is_file():
            raise ConfigError(f"no such config file: {p}")
        with p.open("rb") as fh:
            data = tomllib.load(fh)
        parent_ref = data.pop("extends", None)
        if parent_ref is not None:
            if not isinstance(parent_ref, str):
                raise ConfigError(f"`extends` must be a string, got {parent_ref!r}")
            parent = cls.load(p.parent / parent_ref, _depth=_depth + 1)
            data = deep_merge(parent._data, data)
        cfg = cls(data, source=str(p))
        return cfg.with_overrides(overrides) if overrides else cfg

    def with_overrides(self, overrides: Iterable[str]) -> "Config":
        """Apply ``dotted.key=tomlvalue`` strings, returning a new Config."""
        data = copy.deepcopy(self._data)
        for item in overrides:
            if "=" not in item:
                raise ConfigError(f"override must be key=value, got {item!r}")
            key, _, raw = item.partition("=")
            parts = key.strip().split(".")
            node = data
            for part in parts[:-1]:
                nxt = node.get(part)
                if not isinstance(nxt, dict):
                    nxt = {}
                    node[part] = nxt
                node = nxt
            node[parts[-1]] = _coerce(raw.strip())
        return Config(data, source=f"{self.source}+overrides")

    # -- access --------------------------------------------------------------

    _MISSING = object()

    def get(self, dotted: str, default: Any = _MISSING) -> Any:
        node: Any = self._data
        for part in dotted.split("."):
            if not isinstance(node, Mapping) or part not in node:
                if default is Config._MISSING:
                    raise ConfigError(f"missing config key {dotted!r} in {self.source}")
                return default
            node = node[part]
        return copy.deepcopy(node) if isinstance(node, (dict, list)) else node

    def section(self, dotted: str) -> "Config":
        node = self.get(dotted, {})
        if not isinstance(node, Mapping):
            raise ConfigError(f"config key {dotted!r} is not a table")
        return Config(node, source=f"{self.source}[{dotted}]")

    def float_(self, dotted: str, default: Any = _MISSING) -> float:
        return float(self.get(dotted, default))

    def int_(self, dotted: str, default: Any = _MISSING) -> int:
        value = self.get(dotted, default)
        # TOML has no 1e6 integer literal, so configs write 1e6 and mean 1000000.
        as_float = float(value)
        as_int = int(round(as_float))
        if abs(as_float - as_int) > 1e-9:
            raise ConfigError(f"config key {dotted!r} = {value!r} is not an integer")
        return as_int

    def str_(self, dotted: str, default: Any = _MISSING) -> str:
        return str(self.get(dotted, default))

    def bool_(self, dotted: str, default: Any = _MISSING) -> bool:
        value = self.get(dotted, default)
        if isinstance(value, bool):
            return value
        raise ConfigError(f"config key {dotted!r} = {value!r} is not a boolean")

    def as_dict(self) -> dict[str, Any]:
        return copy.deepcopy(self._data)

    def __contains__(self, dotted: str) -> bool:
        # A distinct sentinel: passing Config._MISSING would mean "raise if absent".
        absent = object()
        return self.get(dotted, absent) is not absent

    def __repr__(self) -> str:
        return f"Config(source={self.source!r}, keys={sorted(self._data)})"
