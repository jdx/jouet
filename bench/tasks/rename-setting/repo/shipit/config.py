"""Settings, from defaults, a file and the command line."""

import tomllib
from pathlib import Path

DEFAULTS = {"retry_limit": 3, "timeout_s": 30}


def load(path=None, overrides=None):
    """Return the settings: DEFAULTS, then `path` (TOML), then `overrides`."""
    settings = dict(DEFAULTS)
    if path and Path(path).exists():
        with open(path, "rb") as f:
            settings.update(tomllib.load(f))
    settings.update({k: v for k, v in (overrides or {}).items() if v is not None})
    validate(settings)
    return settings


def validate(settings):
    if settings["retry_limit"] < 1:
        raise ValueError(f"retry_limit must be at least 1, not {settings['retry_limit']}")
    if settings["timeout_s"] <= 0:
        raise ValueError("timeout_s must be positive")
