//! Exercise the actual CLI against a local HTTP server, without external API keys.
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use assert_cmd::Command;
use serde_json::{json, Value};

const BINARY: &str = "humanify-lua";
const LANGUAGE: &str = "Lua";
const ONE_BINDING: &str = "local userCount = 1; print(userCount);";
const TWO_BINDINGS: &str = "local a = 1; local b = 2; print(a, b);";

struct Request {
    path: String,
    authorization: Option<String>,
    body: Value,
}

struct MockApi {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockApi {
    fn start(decisions: Vec<(u16, Value)>) -> Self {
        Self::with_delay(decisions, Duration::ZERO)
    }

    fn with_delay(decisions: Vec<(u16, Value)>, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorded = Arc::clone(&requests);
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut decisions: VecDeque<_> = decisions.into();
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Poll accepts without blocking, then wait for request data.
                        // Accepted sockets may inherit the listener's nonblocking mode.
                        stream.set_nonblocking(false).unwrap();
                        let request = read_request(&mut stream);
                        let (status, body) = if request.path == "/v1/systemone" {
                            thread::sleep(delay);
                            decisions.pop_front().unwrap_or((500, json!({})))
                        } else {
                            assert_eq!(request.path, "/v1/chat/completions");
                            (
                                200,
                                json!({"choices": [{"message": {"content": "{\"name\":\"descriptiveName\"}"}}]}),
                            )
                        };
                        recorded.lock().unwrap().push(request);
                        let body = body.to_string();
                        let _ = write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock HTTP server: {error}"),
                }
            }
        });
        Self {
            base,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn command(&self, provider: &str) -> Command {
        let mut command = Command::cargo_bin(BINARY).unwrap();
        command
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("LAYA_API_KEY")
            .args([
                "openai",
                "-",
                "--base-url",
                &format!("{}/v1", self.base),
                "--json-mode",
                "prompt",
                "--api-key",
                "generator-secret",
                "--decision-provider",
                provider,
                "--decision-base-url",
                &format!("{}/v1", self.base),
            ])
            .timeout(Duration::from_secs(15));
        command
    }
}

impl Drop for MockApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "incomplete HTTP headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let header = |name: &str| {
        headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
    };
    let content_length: usize = header("content-length").unwrap().parse().unwrap();
    while bytes.len() < header_end + content_length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "incomplete HTTP body");
        bytes.extend_from_slice(&buffer[..count]);
    }
    assert!(headers.starts_with("POST "));
    Request {
        path: headers.split_whitespace().nth(1).unwrap().to_string(),
        authorization: header("authorization"),
        body: serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap(),
    }
}

fn decision(choice: &str, a: f64, b: f64) -> Value {
    json!({"answers": {"name_quality": {"type": "choice", "choice": choice, "probabilities": {"A": a, "B": b}, "confidence": 0.0}}})
}

#[test]
fn jev_keep_skips_generator_and_uses_separate_bearer_key() {
    let api = MockApi::start(vec![(200, decision("A", 0.98, 0.02))]);
    let result = api
        .command("jev")
        .args(["--decision-api-key", "decision-secret", "--verbose"])
        .write_stdin(ONE_BINDING)
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("skipping generative call"));
    assert!(!stderr.contains("decision-secret"));
    assert!(!stderr.contains("generator-secret"));
    assert!(String::from_utf8_lossy(&result.get_output().stdout).contains("userCount"));
    let requests = api.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "generator must not be called");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer decision-secret")
    );
    assert_eq!(requests[0].body["model"], "jev-latest");
    assert_eq!(requests[0].body["state"]["language"], LANGUAGE);
    assert_eq!(requests[0].body["state"]["identifier"], "userCount");
    assert_eq!(requests[0].body["state"]["surrounding_code"], ONE_BINDING);
    assert_eq!(
        requests[0].body["questions"]["name_quality"]["type"],
        "choice"
    );
}

#[test]
fn laya_accepts_full_endpoint_and_no_key() {
    let api = MockApi::start(vec![(200, decision("A", 0.98, 0.02))]);
    let mut command = Command::cargo_bin(BINARY).unwrap();
    command
        .env_remove("LAYA_API_KEY")
        .env("TYPESAFE_API_KEY", "must-not-reach-laya")
        .args([
            "ollama",
            "-",
            "--decision-provider",
            "laya",
            "--decision-base-url",
            &format!("{}/v1/systemone/", api.base),
        ])
        .write_stdin(ONE_BINDING)
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    let requests = api.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].authorization.is_none());
    assert_eq!(requests[0].body["model"], "english");
}

#[test]
fn custom_laya_model_key_and_threshold_are_honored() {
    let api = MockApi::start(vec![(200, decision("A", 0.92, 0.08))]);
    api.command("laya")
        .env("LAYA_API_KEY", "local-key")
        .args([
            "--decision-model",
            "multilingual",
            "--decision-threshold",
            "0.9",
        ])
        .write_stdin(ONE_BINDING)
        .assert()
        .success();
    let requests = api.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["model"], "multilingual");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer local-key")
    );
}

#[test]
fn unclear_names_and_low_probability_use_the_generator() {
    for response in [decision("B", 0.01, 0.99), decision("A", 0.70, 0.30)] {
        let api = MockApi::start(vec![(200, response)]);
        let result = api
            .command("laya")
            .write_stdin(ONE_BINDING)
            .assert()
            .success();
        assert!(String::from_utf8_lossy(&result.get_output().stdout).contains("descriptiveName"));
        let requests = api.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer generator-secret")
        );
    }
}

#[test]
fn failed_or_malformed_decision_disables_gate_and_keeps_processing() {
    for (status, body) in [
        (401, json!({"error": "remote-secret"})),
        (200, json!({"answers": {}})),
        (200, decision("A", 1.1, -0.1)),
    ] {
        let api = MockApi::start(vec![(status, body)]);
        let result = api
            .command("laya")
            .write_stdin(TWO_BINDINGS)
            .assert()
            .success();
        let stderr = String::from_utf8_lossy(&result.get_output().stderr);
        assert_eq!(stderr.matches("decision API unavailable").count(), 1);
        assert!(!stderr.contains("remote-secret"));
        let requests = api.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            3,
            "one failed decision and two generative calls"
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.path == "/v1/systemone")
                .count(),
            1
        );
    }
}

#[test]
fn disabled_by_default_and_flags_available_for_all_providers() {
    let api = MockApi::start(vec![]);
    Command::cargo_bin(BINARY)
        .unwrap()
        .args([
            "openai",
            "-",
            "--base-url",
            &format!("{}/v1", api.base),
            "--json-mode",
            "prompt",
        ])
        .write_stdin(ONE_BINDING)
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    assert_eq!(api.requests.lock().unwrap()[0].path, "/v1/chat/completions");
    for help in ["-h", "--help"] {
        let result = Command::cargo_bin(BINARY)
            .unwrap()
            .arg(help)
            .assert()
            .success();
        let text = String::from_utf8_lossy(&result.get_output().stdout);
        assert!(text.contains("Optional Jev/Laya decisions are disabled by default"));
        assert!(text.contains("Generate names with an OpenAI-compatible API"));
        assert!(!text.contains("Shared by every generative provider"));
    }
    for provider in [
        "openai",
        "anthropic",
        "gemini",
        "ollama",
        "openrouter",
        "requesty",
    ] {
        for help in ["-h", "--help"] {
            let result = Command::cargo_bin(BINARY)
                .unwrap()
                .args([provider, help])
                .assert()
                .success();
            let text = String::from_utf8_lossy(&result.get_output().stdout);
            for expected in [
                "--decision-provider",
                "Optional Jev/Laya decisions:",
                "0.95",
                "TYPESAFE_API_KEY",
                "LAYA_API_KEY",
                "Examples:",
            ] {
                assert!(
                    text.contains(expected),
                    "{provider} {help} is missing {expected}"
                );
            }
            assert!(!text.contains("Shared by every generative provider"));
            assert!(
                text.contains("Arguments:"),
                "ordinary arguments must retain their help heading"
            );
        }
    }
}

#[test]
fn bad_configuration_is_a_usage_error_before_processing() {
    for extra in [
        vec!["--decision-threshold", "0.5"],
        vec!["--decision-threshold", "1.1"],
        vec!["--decision-threshold", "NaN"],
        vec!["--decision-timeout-seconds", "0"],
        vec!["--decision-base-url", "https://secret@example.com/v1"],
        vec!["--decision-model", ""],
    ] {
        Command::cargo_bin(BINARY)
            .unwrap()
            .args(["openai", "-", "--decision-provider", "laya"])
            .args(extra)
            .write_stdin(ONE_BINDING)
            .assert()
            .code(64);
    }
    Command::cargo_bin(BINARY)
        .unwrap()
        .args(["openai", "-", "--decision-threshold", "0.9"])
        .write_stdin(ONE_BINDING)
        .assert()
        .code(64);
    Command::cargo_bin(BINARY)
        .unwrap()
        .env_remove("TYPESAFE_API_KEY")
        .args(["openai", "-", "--decision-provider", "jev"])
        .write_stdin(ONE_BINDING)
        .assert()
        .code(64);
}

#[test]
fn timeout_disables_gate_instead_of_waiting_again_for_each_identifier() {
    let api = MockApi::with_delay(
        vec![(200, decision("A", 0.98, 0.02))],
        Duration::from_millis(1200),
    );
    let result = api
        .command("laya")
        .args(["--decision-timeout-seconds", "1"])
        .write_stdin(TWO_BINDINGS)
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("request failed or timed out"));
    assert_eq!(stderr.matches("decision API unavailable").count(), 1);
    assert_eq!(api.requests.lock().unwrap().len(), 3);
}

#[test]
fn unreachable_decision_server_still_uses_generator() {
    let api = MockApi::start(vec![]);
    Command::cargo_bin(BINARY)
        .unwrap()
        .args([
            "openai",
            "-",
            "--base-url",
            &format!("{}/v1", api.base),
            "--json-mode",
            "prompt",
            "--decision-provider",
            "laya",
            "--decision-base-url",
            "http://127.0.0.1:1/v1",
        ])
        .write_stdin(TWO_BINDINGS)
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    assert_eq!(api.requests.lock().unwrap().len(), 2);
}
