#!/usr/bin/env python3

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


class FixtureHandler(BaseHTTPRequestHandler):
    fixture_directory = None
    plugin_build = None
    selected_variant = "typical"

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/__plugin__/full.html" and self.plugin_build:
            self.send_file(self.plugin_build / "full.html", "text/html; charset=utf-8")
            return

        parts = path.strip("/").split("/")
        if parts == ["seed"]:
            source = "seed"
            variant = self.selected_variant
        elif len(parts) == 2:
            source, variant = parts
        else:
            self.send_error(404)
            return
        fixture_path = self.fixture_directory / f"{variant}.json"
        if not fixture_path.is_file():
            self.send_error(404)
            return

        fixture = json.loads(fixture_path.read_text())
        if source == "seed":
            base = "http://host.docker.internal:8010"
            payload = {
                "fixture_now": fixture["now"],
                "fixture_sources": {
                    name: f"{base}/{name}/{variant}"
                    for name in ("calendar", "weather", "city", "hohenschoenhausen")
                },
            }
        elif source in fixture.get("errors", []):
            self.send_error(503)
            return
        elif source in ("calendar", "weather", "city", "hohenschoenhausen"):
            payload = fixture[source]
        else:
            self.send_error(404)
            return

        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_file(self, path, content_type):
        body = path.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, format, *args):
        return


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture_directory", type=Path)
    parser.add_argument("--plugin-build", type=Path)
    parser.add_argument("--port", type=int, default=8010)
    parser.add_argument("--variant", choices=("typical", "maximum", "degraded"), default="typical")
    arguments = parser.parse_args()
    FixtureHandler.fixture_directory = arguments.fixture_directory
    FixtureHandler.plugin_build = arguments.plugin_build
    FixtureHandler.selected_variant = arguments.variant
    ThreadingHTTPServer(("0.0.0.0", arguments.port), FixtureHandler).serve_forever()


if __name__ == "__main__":
    main()
