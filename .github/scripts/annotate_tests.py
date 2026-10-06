"""Turn failing cargo test output into GitHub error annotations, so failures
are readable from the check-run annotations API without downloading logs."""

import re
import sys

MAX_ANNOTATIONS = 10
MAX_CHARS = 3500


def escape(text: str) -> str:
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def main(path: str) -> None:
    try:
        log = open(path, encoding="utf-8", errors="replace").read()
    except FileNotFoundError:
        return
    sections = re.findall(r"^---- (\S+) stdout ----\n(.*?)(?=^---- |^failures:\s*$|\Z)", log, re.S | re.M)
    emitted = 0
    for name, body in sections:
        if emitted >= MAX_ANNOTATIONS - 1:
            break
        print(f"::error title=test {name}::{escape(body.strip()[-MAX_CHARS:])}")
        emitted += 1
    # Compile errors in tests, panics outside sections, and the summary.
    tail = "\n".join(line for line in log.splitlines() if line.startswith(("error", "test result", "     Running", "    Finished")) or "FAILED" in line)
    print(f"::error title=test summary::{escape(tail[-MAX_CHARS:])}")


if __name__ == "__main__":
    main(sys.argv[1])
