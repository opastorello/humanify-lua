#!/usr/bin/env python3
"""Serve an installed laya-mlx model through the Jev-compatible decision API.

Run with the Python interpreter that has laya-mlx installed. No web framework
is required. The model is loaded once and predictions run serially on MLX.
"""

import argparse
import hmac
import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer


class DecisionServer(HTTPServer):
    def __init__(self, address, agent, model, api_key=None):
        self.agent = agent
        self.model = model
        self.api_key = api_key
        super().__init__(address, DecisionHandler)


class DecisionHandler(BaseHTTPRequestHandler):
    def log_message(self, _format, *args):
        # Never log request contents or authorization headers.
        pass

    def respond(self, status, body):
        payload = json.dumps(body, allow_nan=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        try:
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError):
            pass  # The client may have reached its decision timeout.

    def do_GET(self):
        if self.path == "/health":
            self.respond(200, {"status": "ready", "model": self.server.model})
        else:
            self.respond(404, {"error": "unknown endpoint"})

    def do_POST(self):
        if self.path.rstrip("/") != "/v1/systemone":
            self.respond(404, {"error": "unknown endpoint"})
            return
        if self.server.api_key and not hmac.compare_digest(
            self.headers.get("Authorization", "").encode(),
            f"Bearer {self.server.api_key}".encode(),
        ):
            self.respond(401, {"error": "invalid decision API key"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self.respond(400, {"error": "invalid Content-Length"})
            return
        if not 0 < length <= 1024 * 1024:
            self.respond(413, {"error": "request must contain 1 to 1048576 bytes"})
            return
        try:
            self.connection.settimeout(10)
            body = json.loads(self.rfile.read(length))
        except (ValueError, OSError):
            self.respond(400, {"error": "invalid JSON request"})
            return
        if (
            not isinstance(body, dict)
            or not isinstance(body.get("state"), (str, dict, list))
            or not isinstance(body.get("questions"), dict)
            or not body["questions"]
        ):
            self.respond(422, {"error": "state and nonempty questions are required"})
            return
        # This is one resident checkpoint. Never silently route a requested model
        # to different weights, or reload weights for each identifier.
        if body.get("model") != self.server.model:
            self.respond(422, {"error": "model must match the server's --model"})
            return
        if len(body["questions"]) > 32:
            self.respond(413, {"error": "at most 32 questions per request"})
            return
        try:
            result = self.server.agent.predict(body["state"], body["questions"])
            result["model"] = self.server.model
            self.respond(200, result)
        except (ValueError, TypeError, LookupError):
            self.respond(422, {"error": "invalid decision question or input"})
        except (RuntimeError, ArithmeticError, MemoryError, OSError):
            self.respond(500, {"error": "local MLX inference failed"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="aac6fef/laya-multilingual-mlx")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--device", choices=("gpu", "cpu"), default="gpu")
    parser.add_argument(
        "--dtype", choices=("float16", "float32", "bfloat16"), default="float16"
    )
    parser.add_argument(
        "--offline", action="store_true", help="Use cached/local weights only"
    )
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("--port must be between 1 and 65535")
    if args.offline:
        os.environ["HF_HUB_OFFLINE"] = "1"

    # Keep imports lazy so the protocol can be tested without MLX or model weights.
    import laya_mlx

    print(f"Loading {args.model} on {args.device}...", flush=True)
    agent = laya_mlx.load(args.model, device=args.device, dtype=args.dtype)
    server = DecisionServer(
        ("127.0.0.1", args.port), agent, args.model, os.environ.get("LAYA_API_KEY")
    )
    print(f"Ready: http://127.0.0.1:{args.port}/v1/systemone", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
