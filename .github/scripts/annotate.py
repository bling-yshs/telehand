"""Emit a file as GitHub error annotations (chunked), so CI output is readable
from the check-run annotations API without downloading logs.

Usage: annotate.py <title> <file> [--tests]
With --tests, only the failing-test sections and the summary of cargo test
output are emitted."""

import re
import sys

MAX_ANNOTATIONS = 9
MAX_CHARS = 3500


def escape(text: str) -> str:
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def emit(title: str, text: str) -> None:
    print(f"::error title={title}::{escape(text)}")


def chunks(text: str):
    lines, size = [], 0
    for line in text.splitlines():
        if size + len(line) > MAX_CHARS and lines:
            yield "\n".join(lines)
            lines, size = [], 0
        lines.append(line[:MAX_CHARS])
        size += len(line) + 1
    if lines:
        yield "\n".join(lines)


def main() -> None:
    title, path = sys.argv[1], sys.argv[2]
    try:
        text = open(path, encoding="utf-8", errors="replace").read()
    except FileNotFoundError:
        return
    if "--tests" in sys.argv:
        sections = re.findall(
            r"^---- (\S+) stdout ----\n(.*?)(?=^---- |^failures:\s*$|\Z)", text, re.S | re.M
        )
        for name, body in sections[: MAX_ANNOTATIONS - 1]:
            emit(f"test {name}", body.strip()[-MAX_CHARS:])
        summary = "\n".join(
            line
            for line in text.splitlines()
            if line.startswith(("error", "test result", "     Running")) or "FAILED" in line
        )
        emit(f"{title} summary", summary[-MAX_CHARS:])
        return
    for i, chunk in enumerate(list(chunks(text))[:MAX_ANNOTATIONS]):
        emit(f"{title} {i + 1}", chunk)


if __name__ == "__main__":
    main()
