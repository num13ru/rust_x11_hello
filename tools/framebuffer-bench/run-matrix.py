#!/usr/bin/env python3
"""Run the host matrix and compare hypothetical per-frame encoding choices."""

import argparse
import csv
import itertools
import struct
import subprocess
import time
from pathlib import Path

FULL = (
    "raw", "rle", "lz4", "zstd-1", "zstd-3", "zstd-6", "deflate-fast",
    "deflate-default", "deflate-best", "png",
)
SCENARIOS = (
    "now-playing", "application-grid", "motion-mono-4hz", "motion-mono-10hz",
    "motion-gray-4hz", "motion-gray-10hz", "synthetic-mono", "synthetic-gray",
)
TILE_SIZES = (16, 32, 64, 128)
FPS = (0.5, 1, 2, 4, 8, 10)
RECORD = struct.Struct("<IQIIII")
ADAPTIVE = ("raw", "lz4", "zstd-1", "xor-lz4", "tiles-lz4", "rect-lz4")


def configurations():
    for codec in FULL:
        yield codec, 0, 0
    for codec in ("raw", "rle", "lz4", "zstd-1", "deflate-fast"):
        yield f"xor-{codec}", 0, 0
    for mode in ("rect", "rects"):
        for codec in ("raw", "lz4", "zstd-1"):
            yield f"{mode}-{codec}", 0, 0
    for size in TILE_SIZES:
        for codec in ("raw", "rle", "lz4", "zstd-1"):
            yield f"tiles-{codec}", size, 0
        yield "tiles-each-lz4", size, 0
        yield "tiles-xor-lz4", size, 0
    for mode, size in (("xor", 0), ("tiles", 32), ("rect", 0)):
        for interval in (10, 30, 60):
            yield f"{mode}-lz4", size, interval


def csv_result(command):
    output = subprocess.check_output(list(map(str, command)), text=True)
    return next(csv.DictReader(output.splitlines()))


def sizes(path):
    with path.open("rb") as source:
        header = source.read(20)
        count = int.from_bytes(header[16:20], "little")
        source.seek(header[6] + 4, 1)
        return [RECORD.size + read_size(source) for _ in range(count)]


def read_size(source):
    _, _, length, _, _, _ = RECORD.unpack(source.read(RECORD.size))
    source.seek(length, 1)
    return length


def heuristic(source, lengths):
    # Whole rows sampled every 16 scanlines. Each decision uses only this frame
    # and the previous decoded frame. The timer includes row comparison and
    # selection and excludes file I/O.
    previous = None
    chosen = []
    elapsed_ns = 0
    with source.open("rb") as corpus:
        header = corpus.read(20)
        stride = int.from_bytes(header[12:16], "little")
        height = int.from_bytes(header[10:12], "little")
        corpus.seek(header[6], 1)
        rows = list(range(0, stride * height, stride * 16))
        for index in range(len(lengths["lz4"])):
            corpus.seek(16, 1)
            current = corpus.read(stride * height)
            start = time.perf_counter_ns()
            if previous is None:
                selected = "lz4"
            else:
                changed = sum(
                    current[row:row + stride] != previous[row:row + stride]
                    for row in rows
                )
                if changed == 0:
                    selected = "tiles-lz4"
                elif changed * 4 <= len(rows):
                    selected = "rect-lz4"
                elif changed * 3 >= len(rows) * 2:
                    selected = "lz4"
                else:
                    selected = "xor-lz4"
            elapsed_ns += time.perf_counter_ns() - start
            chosen.append(lengths[selected][index] + 1)
            previous = current
    return sum(chosen) / len(chosen), elapsed_ns / len(chosen) / 1000


def bandwidth(scenario, codec, tile_size, keyframe, average):
    for fps in FPS:
        rate = average * fps
        yield {
            "scenario": scenario, "codec": codec, "tile_size": tile_size,
            "keyframe": keyframe, "fps": fps, "bytes_per_frame": average,
            "kib_per_s": rate / 1024, "mib_per_s": rate / 1024**2,
            "mbit_per_s": rate * 8 / 1_000_000,
            "mib_per_hour": rate * 3600 / 1024**2,
            "gib_per_hour": rate * 3600 / 1024**3,
        }


def write_csv(path, rows):
    with path.open("x", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


def run(args):
    args.output.mkdir()
    metrics = []
    adaptive = []
    projections = []
    for scenario in args.scenarios:
        corpus = args.corpora / f"{scenario}.fbcp"
        lengths = {}
        for codec, tile_size, keyframe in configurations():
            encoded = args.output / f"{scenario}-{codec}-t{tile_size}-k{keyframe}.fbce"
            options = ["--warmup", args.warmup, "--iterations", args.iterations, "--csv"]
            encode_options = options.copy()
            if tile_size:
                encode_options += ["--tile-size", tile_size]
            if keyframe:
                encode_options += ["--keyframe", keyframe]
            encode = csv_result([args.bench, "encode", corpus, codec, encoded, *encode_options])
            subprocess.run(
                [args.bench, "verify", corpus, encoded], check=True, stdout=subprocess.DEVNULL,
            )
            decode = csv_result([args.bench, "decode", encoded, *options])
            metrics.extend(({**encode, "keyframe": keyframe}, {**decode, "keyframe": keyframe}))
            projections.extend(bandwidth(scenario, codec, tile_size, keyframe, float(encode["avg_bytes"])))
            if codec in ADAPTIVE and keyframe == 0 and tile_size in (0, 32):
                lengths[codec] = sizes(encoded)
        oracle = [min(candidates) + 1 for candidates in zip(*(lengths[name] for name in ADAPTIVE), strict=True)]
        best_fixed = min(ADAPTIVE, key=lambda name: sum(lengths[name]))
        oracle_mean = sum(oracle) / len(oracle)
        heuristic_mean, heuristic_us = heuristic(corpus, lengths)
        adaptive.append({
            "scenario": scenario, "best_fixed": best_fixed,
            "fixed_mean_bytes": sum(lengths[best_fixed]) / len(oracle),
            "oracle_mean_bytes": oracle_mean, "heuristic_mean_bytes": heuristic_mean,
            "heuristic_mean_us_python": heuristic_us,
        })
        projections.extend(itertools.chain(
            bandwidth(scenario, "adaptive-oracle", 0, 0, oracle_mean),
            bandwidth(scenario, "adaptive-heuristic", 0, 0, heuristic_mean),
        ))
        print(f"Verified {scenario}", flush=True)
    write_csv(args.output / "metrics.csv", metrics)
    write_csv(args.output / "adaptive.csv", adaptive)
    write_csv(args.output / "bandwidth.csv", projections)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bench", type=Path)
    parser.add_argument("corpora", type=Path)
    parser.add_argument("output", type=Path, help="new directory for encoded sequences and CSVs")
    parser.add_argument("--scenarios", nargs="+", choices=SCENARIOS, default=SCENARIOS)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--iterations", type=int, default=3)
    run(parser.parse_args())
