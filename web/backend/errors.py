"""Lightweight error helpers mirroring the Rust `anyhow::Result` usage.

Endpoints raise `AppError` to signal a user-facing failure (e.g. adb not
connected); the FastAPI layer converts it into a JSON 4xx/5xx response.
"""


class AppError(Exception):
    """Raised for expected, user-facing failures (adb errors, bad input)."""


def err(msg: str) -> AppError:
    """Construct an `AppError` (mirrors the `anyhow!` macro)."""
    return AppError(msg)
