#!/usr/bin/env python3
"""Compatibility checks for backend-neutral PaperPad input latency stages."""

from pathlib import Path
from runpy import run_path
from typing import Any, Callable, cast


MODULE = run_path(str(Path(__file__).with_name("analyze-latency.py")))
input_stage = cast(
    Callable[[dict[str, dict[str, Any]]], dict[str, Any] | None],
    MODULE["input_stage"],
)

legacy = {"x11_received": {"wall_us": 10}}
native = {"input_received": {"wall_us": 20}}
both = {**legacy, **native}

assert input_stage({}) is None
assert input_stage(legacy) is legacy["x11_received"]
assert input_stage(native) is native["input_received"]
assert input_stage(both) is native["input_received"]

print("latency analyzer input-stage compatibility checks passed")
