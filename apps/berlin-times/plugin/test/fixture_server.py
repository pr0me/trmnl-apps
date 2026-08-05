#!/usr/bin/env python3

import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


class CorsHandler(SimpleHTTPRequestHandler):
    plugin_build = None

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/__plugin__/full.html" and self.plugin_build:
            content = (self.plugin_build / "full.html").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)
            return
        super().do_GET()

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--plugin-build", type=Path)
    arguments = parser.parse_args()
    CorsHandler.plugin_build = arguments.plugin_build
    handler = partial(CorsHandler, directory=arguments.directory)
    ThreadingHTTPServer(("0.0.0.0", arguments.port), handler).serve_forever()


if __name__ == "__main__":
    main()
