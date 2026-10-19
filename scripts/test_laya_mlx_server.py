"""Protocol checks without importing MLX or downloading model weights."""

import json
import threading
import unittest
from http.client import HTTPConnection

from laya_mlx_server import DecisionServer


class FakeAgent:
    def __init__(self):
        self.calls = []

    def predict(self, state, questions):
        self.calls.append((state, questions))
        return {
            "answers": {
                "name_quality": {
                    "type": "choice",
                    "choice": "A",
                    "probabilities": {"A": 0.98, "B": 0.02},
                }
            },
            "usage": {"input_tokens": 10, "output_tokens": 0},
        }


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.agent = FakeAgent()
        self.server = DecisionServer(
            ("127.0.0.1", 0), self.agent, "test-model", "local-secret"
        )
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.worker.start()
        self.body = {
            "model": "test-model",
            "state": {"identifier": "userCount"},
            "questions": {
                "name_quality": {
                    "type": "choice",
                    "instructions": "Keep the name?",
                    "criteria": {"A": "keep", "B": "rename"},
                }
            },
        }

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join()

    def request(
        self, body=None, key="local-secret", path="/v1/systemone", method="POST"
    ):
        connection = HTTPConnection(*self.server.server_address, timeout=2)
        payload = json.dumps(self.body if body is None else body)
        headers = {"Content-Type": "application/json"}
        if key:
            headers["Authorization"] = f"Bearer {key}"
        connection.request(method, path, payload, headers)
        response = connection.getresponse()
        status, result = response.status, json.loads(response.read())
        connection.close()
        return status, result

    def test_forwards_state_and_questions_and_reports_resident_model(self):
        status, result = self.request()
        self.assertEqual(status, 200)
        self.assertEqual(result["model"], "test-model")
        self.assertEqual(result["answers"]["name_quality"]["probabilities"]["A"], 0.98)
        self.assertEqual(
            self.agent.calls, [(self.body["state"], self.body["questions"])]
        )

    def test_bad_or_missing_auth_does_not_run_inference(self):
        for key in (None, "wrong"):
            self.assertEqual(self.request(key=key)[0], 401)
        self.assertEqual(self.agent.calls, [])

    def test_local_key_is_optional(self):
        self.server.api_key = None
        self.assertEqual(self.request(key=None)[0], 200)

    def test_wrong_model_or_invalid_body_does_not_run_inference(self):
        for body in (
            {**self.body, "model": "another-checkpoint"},
            {"state": "code"},
            [],
            {**self.body, "questions": {}},
        ):
            self.assertEqual(self.request(body=body)[0], 422)
        self.assertEqual(self.agent.calls, [])

    def test_health_and_unknown_paths(self):
        self.assertEqual(
            self.request(path="/health", method="GET")[1],
            {"status": "ready", "model": "test-model"},
        )
        self.assertEqual(self.request(path="/v1/chat/completions")[0], 404)


if __name__ == "__main__":
    unittest.main()
