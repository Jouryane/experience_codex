"""Count Responses API requests for the M6 zero-LLM acceptance.

The server deliberately returns 500 for every non-health request. The
acceptance only passes when the log file stays empty, which proves the
turn-level Experience gate completed the task without sampling a model.
"""
from __future__ import annotations

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


LOG = Path(sys.argv[2])
PORT = int(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    def _log(self, method: str) -> None:
        payload = {
            "at": time.time(),
            "method": method,
            "path": self.path,
        }
        with LOG.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, ensure_ascii=False) + "\n")

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/health":
            body = b"ok"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self._log("GET")
        self.send_response(500)
        self.end_headers()

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", "0"))
        if length:
            self.rfile.read(length)
        self._log("POST")
        self.send_response(500)
        self.end_headers()

    def log_message(self, _format: str, *args) -> None:
        return


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
