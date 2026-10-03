use std::thread;
use std::time::{Duration, Instant};

use atlas_core::LlmError;
use serde_json::Value;

const ERROR_BODY_CHARS: usize = 300;
const BASE_BACKOFF: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
  pub max_retries: u32,
  pub max_delay: Duration,
  pub retry_statuses: &'static [u16],
}

impl RetryPolicy {
  pub(crate) const fn new(retry_statuses: &'static [u16]) -> Self {
    Self {
      max_retries: 2,
      max_delay: Duration::from_secs(30),
      retry_statuses,
    }
  }
}

fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
  let seconds = response.headers().get("retry-after")?.to_str().ok()?.trim().parse::<f64>().ok()?;
  (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

pub(crate) fn post_json(url: &str, headers: &[(&str, String)], body: &Value, timeout: Duration, retry: RetryPolicy) -> Result<Value, LlmError> {
  let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into();
  let started = Instant::now();
  let mut attempt = 0;
  loop {
    let mut request = agent.post(url);
    for (name, value) in headers {
      request = request.header(*name, value.as_str());
    }
    let mut response = request.send_json(body).map_err(|error| map_transport(error, timeout))?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
      return response
        .body_mut()
        .read_json::<Value>()
        .map_err(|error| LlmError::InvalidResponse(format!("response body is not JSON: {error}")));
    }
    if !retry.retry_statuses.contains(&status) {
      let text = response.body_mut().read_to_string().unwrap_or_default();
      return Err(LlmError::Provider(format!("HTTP {status}: {}", text.chars().take(ERROR_BODY_CHARS).collect::<String>())));
    }
    let advised = retry_after(&response);
    let delay = advised.unwrap_or_else(|| BASE_BACKOFF * 2u32.pow(attempt)).min(retry.max_delay);
    if attempt >= retry.max_retries || started.elapsed() + delay >= timeout {
      return Err(LlmError::RateLimited { retry_after: advised });
    }
    tracing::warn!(status, attempt = attempt + 1, delay_ms = delay.as_millis() as u64, "retrying throttled LLM request");
    thread::sleep(delay);
    attempt += 1;
  }
}

fn map_transport(error: ureq::Error, timeout: Duration) -> LlmError {
  match error {
    ureq::Error::Timeout(_) => LlmError::Timeout(timeout),
    other => LlmError::Provider(format!("transport error: {other}")),
  }
}

#[cfg(test)]
pub(crate) mod test_server {
  use std::io::{Read, Write};
  use std::net::TcpListener;
  use std::sync::{Arc, Mutex};
  use std::thread;

  pub(crate) struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: &'static str,
  }

  pub(crate) struct TestServer {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<String>>>,
  }

  pub(crate) fn serve(replies: Vec<Reply>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    thread::spawn(move || {
      for reply in replies {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let raw = read_request(&mut stream);
        seen.lock().unwrap().push(raw);
        let mut head = format!(
          "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
          reply.status,
          reply.body.len()
        );
        for (name, value) in reply.headers {
          head.push_str(&format!("{name}: {value}\r\n"));
        }
        let _ = stream.write_all(format!("{head}\r\n{}", reply.body).as_bytes());
      }
    });
    TestServer { base_url, requests }
  }

  fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
      let read = stream.read(&mut chunk).unwrap_or(0);
      data.extend_from_slice(&chunk[..read]);
      let text = String::from_utf8_lossy(&data);
      if read == 0 {
        return text.into_owned();
      }
      if let Some((head, body)) = text.split_once("\r\n\r\n") {
        let length = head
          .lines()
          .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
          .unwrap_or(0);
        if body.len() >= length {
          return text.into_owned();
        }
      }
    }
  }
}
