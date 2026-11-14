#!/usr/bin/env python3
"""Join PaperPad, PaperSpoon, and optional Hammerspoon latency JSONL logs."""

from __future__ import annotations

import argparse
import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


TraceKey = tuple[str, int]


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            if not line.strip():
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError as error:
                raise SystemExit(f"{path}:{line_number}: invalid JSON: {error}") from error
            records.append(record)
    return records


def trace_key(record: dict[str, Any]) -> TraceKey | None:
    if "session" not in record or "trace" not in record:
        return None
    return str(record["session"]), int(record["trace"])


def index_traces(records: list[dict[str, Any]]) -> dict[TraceKey, dict[str, dict[str, Any]]]:
    indexed: dict[TraceKey, dict[str, dict[str, Any]]] = defaultdict(dict)
    for record in records:
        key = trace_key(record)
        stage = record.get("stage")
        if key is not None and isinstance(stage, str):
            indexed[key].setdefault(stage, record)
    return indexed


def index_frames(
    records: list[dict[str, Any]],
) -> dict[tuple[str, int], dict[str, dict[str, Any]]]:
    indexed: dict[tuple[str, int], dict[str, dict[str, Any]]] = defaultdict(dict)
    for record in records:
        if "frame_id" not in record:
            continue
        stage = record.get("stage")
        if isinstance(stage, str):
            frame_key = (str(record.get("session", "")), int(record["frame_id"]))
            indexed[frame_key].setdefault(stage, record)
    return indexed


def mono_delta(later: dict[str, Any] | None, earlier: dict[str, Any] | None) -> int | None:
    if later is None or earlier is None:
        return None
    return int(later["mono_us"]) - int(earlier["mono_us"])


def wall_delta(later: dict[str, Any] | None, earlier: dict[str, Any] | None) -> int | None:
    if later is None or earlier is None:
        return None
    return int(later["wall_us"]) - int(earlier["wall_us"])


def optional_number(value: float | int | None) -> str:
    if value is None:
        return ""
    return str(round(value, 1) if isinstance(value, float) else value)


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("empty percentile")
    index = max(0, min(len(ordered) - 1, round((len(ordered) - 1) * fraction)))
    return ordered[index]


def input_stage(events: dict[str, dict[str, Any]]) -> dict[str, Any] | None:
    """Prefer backend-neutral traces while accepting historical X11 logs."""
    return events.get("input_received") or events.get("x11_received")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Join latency traces and emit one CSV row per pointer event."
    )
    parser.add_argument("paperpad", type=Path)
    parser.add_argument("paperspoon", type=Path)
    parser.add_argument("hammerspoon", nargs="?", type=Path)
    args = parser.parse_args()

    paperpad_records = load_jsonl(args.paperpad)
    paperspoon_records = load_jsonl(args.paperspoon)
    hammerspoon_records = load_jsonl(args.hammerspoon) if args.hammerspoon else []

    paperpad = index_traces(paperpad_records)
    paperspoon = index_traces(paperspoon_records)
    hammerspoon = index_traces(hammerspoon_records)
    paperpad_frames = index_frames(paperpad_records)

    columns = [
        "session",
        "trace",
        "phase",
        "input_to_queue_us",
        "queue_to_write_us",
        "paperspoon_process_us",
        "feedback_server_us",
        "transport_rtt_us",
        "symmetric_one_way_us",
        "clock_offset_us",
        "visual_submit_roundtrip_us",
        "frame_present_duration_us",
        "release_to_invoke_us",
        "open_duration_us",
        "release_to_hammerspoon_us",
        "hammerspoon_dispatch_us",
        "release_to_dispatch_return_us",
        "release_to_effect_us",
        "contact_hold_us",
        "down_to_hammerspoon_us",
        "down_to_dispatch_return_us",
        "down_to_effect_us",
        "frame_id",
    ]
    writer = csv.DictWriter(sys.stdout, fieldnames=columns)
    writer.writeheader()

    summary: dict[str, list[float]] = defaultdict(list)
    invalid_negative: dict[str, int] = defaultdict(int)
    signed_metrics = {"clock_offset_us"}
    last_down: dict[str, tuple[TraceKey, dict[str, Any]]] = {}

    ordered_keys = sorted(
        paperpad,
        key=lambda key: int((input_stage(paperpad[key]) or {}).get("wall_us", 0)),
    )
    for key in ordered_keys:
        pp = paperpad[key]
        ps = paperspoon.get(key, {})
        hs = hammerspoon.get(key, {})
        input_received = input_stage(pp)
        queued = pp.get("pointer_queued")
        write_done = pp.get("socket_write_done")
        received = ps.get("pointer_received")
        processed = ps.get("input_processed")
        feedback = ps.get("feedback_frame_sent")
        invoke_start = ps.get("action_invoke_start")
        invoke_return = ps.get("action_invoke_return")
        handler = hs.get("handler_received")
        dispatch_return = hs.get("dispatch_return")
        target_effect = hs.get("target_effect")
        phase = str(received.get("phase", "")) if received else ""
        frame_id = int(feedback["frame_id"]) if feedback and "frame_id" in feedback else None
        frame = paperpad_frames.get((key[0], frame_id), {}) if frame_id is not None else {}
        frame_received = frame.get("frame_received")
        frame_present_start = frame.get("frame_present_start")
        frame_present = frame.get("frame_present_return")

        device_turnaround = mono_delta(frame_received, write_done)
        server_turnaround = mono_delta(feedback, received)
        transport_rtt = (
            device_turnaround - server_turnaround
            if device_turnaround is not None and server_turnaround is not None
            else None
        )
        one_way = transport_rtt / 2 if transport_rtt is not None else None

        clock_offset = None
        if write_done and received and feedback and frame_received:
            clock_offset = (
                (int(received["wall_us"]) - int(write_done["wall_us"]))
                + (int(feedback["wall_us"]) - int(frame_received["wall_us"]))
            ) / 2

        release_to_hammerspoon = None
        if handler and input_received and clock_offset is not None:
            release_to_hammerspoon = int(handler["wall_us"]) - (
                int(input_received["wall_us"]) + clock_offset
            )
        release_to_dispatch_return = None
        if dispatch_return and input_received and clock_offset is not None:
            release_to_dispatch_return = int(dispatch_return["wall_us"]) - (
                int(input_received["wall_us"]) + clock_offset
            )
        release_to_effect = None
        if target_effect and input_received and clock_offset is not None:
            release_to_effect = int(target_effect["wall_us"]) - (
                int(input_received["wall_us"]) + clock_offset
            )

        contact_hold = None
        down_to_hammerspoon = None
        down_to_dispatch_return = None
        down_to_effect = None
        if phase == "down" and input_received:
            last_down[key[0]] = (key, input_received)
        elif phase == "up" and input_received:
            down_entry = last_down.pop(key[0], None)
            if down_entry:
                down_key, down_input = down_entry
                contact_hold = mono_delta(input_received, down_input)
                down_ps = paperspoon.get(down_key, {})
                down_feedback = down_ps.get("feedback_frame_sent")
                down_frame_id = (
                    int(down_feedback["frame_id"])
                    if down_feedback and "frame_id" in down_feedback
                    else None
                )
                down_frame_received = (
                    paperpad_frames.get((down_key[0], down_frame_id), {}).get("frame_received")
                    if down_frame_id is not None
                    else None
                )
                down_write = paperpad[down_key].get("socket_write_done")
                down_received = down_ps.get("pointer_received")
                down_offset = None
                if down_write and down_received and down_feedback and down_frame_received:
                    down_offset = (
                        (int(down_received["wall_us"]) - int(down_write["wall_us"]))
                        + (
                            int(down_feedback["wall_us"])
                            - int(down_frame_received["wall_us"])
                        )
                    ) / 2
                if handler and down_offset is not None:
                    down_to_hammerspoon = int(handler["wall_us"]) - (
                        int(down_input["wall_us"]) + down_offset
                    )
                if dispatch_return and down_offset is not None:
                    down_to_dispatch_return = int(dispatch_return["wall_us"]) - (
                        int(down_input["wall_us"]) + down_offset
                    )
                if target_effect and down_offset is not None:
                    down_to_effect = int(target_effect["wall_us"]) - (
                        int(down_input["wall_us"]) + down_offset
                    )

        metrics: dict[str, float | int | None] = {
            "input_to_queue_us": mono_delta(queued, input_received),
            "queue_to_write_us": mono_delta(write_done, queued),
            "paperspoon_process_us": mono_delta(processed, received),
            "feedback_server_us": server_turnaround,
            "transport_rtt_us": transport_rtt,
            "symmetric_one_way_us": one_way,
            "clock_offset_us": clock_offset,
            "visual_submit_roundtrip_us": mono_delta(frame_present, input_received),
            "frame_present_duration_us": mono_delta(frame_present, frame_present_start),
            "release_to_invoke_us": mono_delta(invoke_start, received),
            "open_duration_us": mono_delta(invoke_return, invoke_start),
            "release_to_hammerspoon_us": release_to_hammerspoon,
            "hammerspoon_dispatch_us": mono_delta(dispatch_return, handler),
            "release_to_dispatch_return_us": release_to_dispatch_return,
            "release_to_effect_us": release_to_effect,
            "contact_hold_us": contact_hold,
            "down_to_hammerspoon_us": down_to_hammerspoon,
            "down_to_dispatch_return_us": down_to_dispatch_return,
            "down_to_effect_us": down_to_effect,
        }
        row = {
            "session": key[0],
            "trace": key[1],
            "phase": phase,
            "frame_id": "" if frame_id is None else frame_id,
            **{name: optional_number(value) for name, value in metrics.items()},
        }
        writer.writerow(row)
        for name, value in metrics.items():
            if value is None:
                continue
            if value < 0 and name not in signed_metrics:
                invalid_negative[name] += 1
                continue
            summary[name].append(float(value))

    print("\nSummary (microseconds; p50/p95):", file=sys.stderr)
    for name in columns[3:-1]:
        values = summary.get(name, [])
        if values:
            print(
                f"  {name}: n={len(values)} "
                f"p50={statistics.median(values):.1f} p95={percentile(values, 0.95):.1f}",
                file=sys.stderr,
            )
    for name, count in sorted(invalid_negative.items()):
        print(
            f"warning: ignored {count} negative {name} sample(s); "
            "check trace-stage ordering",
            file=sys.stderr,
        )
    print(
        "symmetric_one_way_us assumes similar upstream/downstream delay; "
        "frame_present_return is not camera-verified visible completion.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
