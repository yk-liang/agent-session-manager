"""Generate deterministic metadata-only logs for the isolated storage benchmark."""
import json
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
append = len(sys.argv) > 2 and sys.argv[2] == "--append"
base = 1_790_000_000_000


def usage(n):
    return {"input_tokens": 1000 * n, "output_tokens": 100 * n,
            "cached_input_tokens": 800 * n, "reasoning_output_tokens": 20 * n,
            "total_tokens": 1100 * n}


def codex_record(file, row):
    at = base + file * 10_000_000 + row * 1000
    return [
        {"type": "token_usage_record", "timestamp": at,
         "payload": {"response_id": f"response_{file:04}_{row:06}", "model": "fixture-model",
                     "usage": usage(1), "thread_token_usage": usage(row + 1)}},
        {"type": "event_msg", "timestamp": at + 1,
         "payload": {"type": "token_count", "info": {"total_token_usage": usage(row + 1), "last_token_usage": usage(1)}}},
    ]


if append:
    with (root / "codex" / "session-0000.jsonl").open("a") as out:
        for row in range(1000, 1010):
            for record in codex_record(0, row):
                out.write(json.dumps(record, separators=(",", ":")) + "\n")
else:
    if root.exists():
        raise SystemExit("Fixture destination must not already exist")
    root.mkdir(parents=True, mode=0o700)
    for agent, files, requests in [("codex", 50, 1000), ("claude", 80, 100), ("qoder", 32, 100), ("pi", 23, 100), ("opencode", 0, 0)]:
        directory = root / agent
        directory.mkdir()
        for file in range(files):
            with (directory / f"session-{file:04}.jsonl").open("w") as out:
                if agent == "codex":
                    out.write(json.dumps({"type": "session_meta", "timestamp": base + file * 10_000_000,
                                          "payload": {"id": f"session-{file:04}", "cwd": "/synthetic/project", "model": "fixture-model"}}) + "\n")
                for row in range(requests):
                    if agent == "codex":
                        records = codex_record(file, row)
                    elif agent == "pi":
                        records = [{"type": "message", "id": f"msg_{file}_{row}", "timestamp": base + file * 100000 + row * 1000,
                                    "message": {"role": "assistant", "model": "fixture-model", "usage": {"input": 1000, "output": 100, "cacheRead": 800}}}]
                    else:
                        records = [{"type": "assistant", "sessionId": f"session-{file:04}", "timestamp": base + file * 100000 + row * 1000,
                                    "message": {"id": f"msg_{agent}_{file}_{row}", "model": "fixture-model",
                                                "usage": {"input_tokens": 1000, "output_tokens": 100, "cache_read_input_tokens": 800}}}]
                    for record in records:
                        out.write(json.dumps(record, separators=(",", ":")) + "\n")
    print("Generated 63,500 synthetic requests; no user logs were read")
