use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};

use thiserror::Error;

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FramingError {
  #[error("the stream ended in the middle of a message")]
  Truncated,
  #[error("reading the stream failed: {0}")]
  Io(#[from] io::Error),
  #[error("message header has no Content-Length")]
  MissingContentLength,
  #[error("invalid Content-Length {0:?}")]
  InvalidContentLength(String),
  #[error("conflicting Content-Length headers {first} and {second}")]
  ConflictingContentLength { first: usize, second: usize },
  #[error("message of {length} bytes exceeds the {limit} byte limit")]
  TooLarge { length: usize, limit: usize },
  #[error("message header exceeds {MAX_HEADER_BYTES} bytes")]
  HeaderTooLong,
  #[error("malformed header line {0:?}")]
  MalformedHeader(String),
}

pub struct FrameReader<R> {
  inner: BufReader<R>,
  limit: usize,
}

impl<R: Read> FrameReader<R> {
  pub fn new(reader: R) -> Self {
    Self::with_limit(reader, MAX_MESSAGE_BYTES)
  }

  pub fn with_limit(reader: R, limit: usize) -> Self {
    Self {
      inner: BufReader::new(reader),
      limit,
    }
  }

  pub fn read_frame(&mut self) -> Result<Option<Vec<u8>>, FramingError> {
    let Some(length) = self.read_header()? else {
      return Ok(None);
    };
    let mut body = vec![0u8; length];
    match self.inner.read_exact(&mut body) {
      Ok(()) => Ok(Some(body)),
      Err(error) if error.kind() == ErrorKind::UnexpectedEof => Err(FramingError::Truncated),
      Err(error) => Err(FramingError::Io(error)),
    }
  }

  fn read_header(&mut self) -> Result<Option<usize>, FramingError> {
    let mut length: Option<usize> = None;
    let mut header_bytes = 0usize;
    let mut first = true;
    loop {
      let Some(line) = self.read_line(&mut header_bytes)? else {
        return match first {
          true => Ok(None),
          false => Err(FramingError::Truncated),
        };
      };
      first = false;
      if line.is_empty() {
        return length.map(Some).ok_or(FramingError::MissingContentLength);
      }
      let Some((name, value)) = line.split_once(':') else {
        return Err(FramingError::MalformedHeader(line));
      };
      if !name.trim().eq_ignore_ascii_case("content-length") {
        continue;
      }
      let parsed = parse_length(value.trim())?;
      if parsed > self.limit {
        return Err(FramingError::TooLarge {
          length: parsed,
          limit: self.limit,
        });
      }
      match length {
        Some(first) if first != parsed => return Err(FramingError::ConflictingContentLength { first, second: parsed }),
        _ => length = Some(parsed),
      }
    }
  }

  fn read_line(&mut self, header_bytes: &mut usize) -> Result<Option<String>, FramingError> {
    let mut line = Vec::new();
    loop {
      let available = match self.inner.fill_buf() {
        Ok(available) => available,
        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
        Err(error) => return Err(FramingError::Io(error)),
      };
      if available.is_empty() {
        return match line.is_empty() {
          true => Ok(None),
          false => Err(FramingError::Truncated),
        };
      }
      let (taken, done) = match available.iter().position(|byte| *byte == b'\n') {
        Some(index) => (index + 1, true),
        None => (available.len(), false),
      };
      line.extend_from_slice(&available[..taken]);
      self.inner.consume(taken);
      *header_bytes += taken;
      if *header_bytes > MAX_HEADER_BYTES {
        return Err(FramingError::HeaderTooLong);
      }
      if done {
        break;
      }
    }
    line.pop();
    if line.last() == Some(&b'\r') {
      line.pop();
    }
    String::from_utf8(line)
      .map(Some)
      .map_err(|error| FramingError::MalformedHeader(String::from_utf8_lossy(error.as_bytes()).into_owned()))
  }
}

fn parse_length(value: &str) -> Result<usize, FramingError> {
  if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
    return Err(FramingError::InvalidContentLength(value.to_owned()));
  }
  value.parse().map_err(|_| FramingError::InvalidContentLength(value.to_owned()))
}

pub fn encode_frame(body: &[u8]) -> Vec<u8> {
  let mut frame = Vec::with_capacity(body.len() + 32);
  frame.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
  frame.extend_from_slice(body);
  frame
}

pub fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
  writer.write_all(&encode_frame(body))?;
  writer.flush()
}

#[cfg(test)]
mod tests {
  use super::*;

  struct Trickle<'a> {
    data: &'a [u8],
    chunk: usize,
  }

  impl Read for Trickle<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
      let count = self.chunk.min(buffer.len()).min(self.data.len());
      buffer[..count].copy_from_slice(&self.data[..count]);
      self.data = &self.data[count..];
      Ok(count)
    }
  }

  struct Interrupting<'a> {
    data: &'a [u8],
    interrupt_next: bool,
  }

  impl Read for Interrupting<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
      self.interrupt_next = !self.interrupt_next;
      if self.interrupt_next {
        return Err(io::Error::from(ErrorKind::Interrupted));
      }
      let count = buffer.len().min(self.data.len()).min(3);
      buffer[..count].copy_from_slice(&self.data[..count]);
      self.data = &self.data[count..];
      Ok(count)
    }
  }

  fn frames(input: &[u8]) -> Vec<Result<Vec<u8>, String>> {
    let mut reader = FrameReader::new(input);
    let mut found = Vec::new();
    loop {
      match reader.read_frame() {
        Ok(Some(body)) => found.push(Ok(body)),
        Ok(None) => return found,
        Err(error) => {
          found.push(Err(error.to_string()));
          return found;
        }
      }
    }
  }

  #[test]
  fn should_frame_a_message_with_content_length_in_bytes() {
    let mut out = Vec::new();
    write_frame(&mut out, "{\"a\":\"é\"}".as_bytes()).unwrap();

    assert_eq!(out, "Content-Length: 10\r\n\r\n{\"a\":\"é\"}".as_bytes());
  }

  #[test]
  fn should_read_two_messages_from_one_chunk() {
    let input = b"Content-Length: 2\r\n\r\n{}Content-Length: 4\r\n\r\nnull";

    assert_eq!(frames(input), vec![Ok(b"{}".to_vec()), Ok(b"null".to_vec())]);
  }

  #[test]
  fn should_read_messages_delivered_one_byte_at_a_time() {
    let input = b"Content-Length: 7\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n[1,2,3]Content-Length: 2\r\n\r\n{}";
    let mut reader = FrameReader::new(Trickle { data: input, chunk: 1 });

    assert_eq!(reader.read_frame().unwrap(), Some(b"[1,2,3]".to_vec()));
    assert_eq!(reader.read_frame().unwrap(), Some(b"{}".to_vec()));
    assert_eq!(reader.read_frame().unwrap(), None);
  }

  #[test]
  fn should_retry_interrupted_reads() {
    let input = b"Content-Length: 4\r\n\r\ntrue";
    let mut reader = FrameReader::new(Interrupting {
      data: input,
      interrupt_next: false,
    });

    assert_eq!(reader.read_frame().unwrap(), Some(b"true".to_vec()));
  }

  #[test]
  fn should_accept_lowercase_names_bare_newlines_and_spaces_around_the_value() {
    let input = b"content-length:   2  \n\n{}";

    assert_eq!(frames(input), vec![Ok(b"{}".to_vec())]);
  }

  #[test]
  fn should_accept_a_repeated_identical_content_length() {
    let input = b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}";

    assert_eq!(frames(input), vec![Ok(b"{}".to_vec())]);
  }

  #[test]
  fn should_reject_a_header_without_content_length() {
    let input = b"Content-Type: application/json\r\n\r\n{}";

    assert_eq!(frames(input), vec![Err("message header has no Content-Length".to_owned())]);
  }

  #[test]
  fn should_reject_conflicting_content_lengths() {
    let input = b"Content-Length: 2\r\nContent-Length: 3\r\n\r\n{}";

    assert_eq!(frames(input), vec![Err("conflicting Content-Length headers 2 and 3".to_owned())]);
  }

  #[test]
  fn should_reject_non_numeric_negative_and_overflowing_lengths() {
    for value in ["abc", "-1", "+2", "", "1e3", "99999999999999999999999999"] {
      let input = format!("Content-Length: {value}\r\n\r\n{{}}");
      assert_eq!(frames(input.as_bytes()), vec![Err(format!("invalid Content-Length {value:?}"))], "value {value:?}");
    }
  }

  #[test]
  fn should_reject_an_oversized_message_without_reading_its_body() {
    let input = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);

    assert_eq!(
      frames(input.as_bytes()),
      vec![Err(format!("message of {} bytes exceeds the {MAX_MESSAGE_BYTES} byte limit", MAX_MESSAGE_BYTES + 1))]
    );
  }

  #[test]
  fn should_apply_a_custom_size_limit() {
    let mut reader = FrameReader::with_limit(&b"Content-Length: 5\r\n\r\nhello"[..], 4);

    assert!(matches!(reader.read_frame(), Err(FramingError::TooLarge { length: 5, limit: 4 })));
  }

  #[test]
  fn should_reject_a_header_line_without_a_colon() {
    let input = b"Server starting...\r\nContent-Length: 2\r\n\r\n{}";

    assert_eq!(frames(input), vec![Err("malformed header line \"Server starting...\"".to_owned())]);
  }

  #[test]
  fn should_reject_a_header_that_never_ends() {
    let input = vec![b'x'; MAX_HEADER_BYTES + 10];

    assert_eq!(frames(&input), vec![Err(format!("message header exceeds {MAX_HEADER_BYTES} bytes"))]);
  }

  #[test]
  fn should_reject_a_header_with_invalid_utf8() {
    let input = b"X-Name: \xff\xfe\r\nContent-Length: 2\r\n\r\n{}";

    assert_eq!(frames(input), vec![Err("malformed header line \"X-Name: \u{fffd}\u{fffd}\"".to_owned())]);
  }

  #[test]
  fn should_report_truncation_inside_the_header_and_the_body() {
    assert_eq!(frames(b"Content-Length: 2\r\n"), vec![Err("the stream ended in the middle of a message".to_owned())]);
    assert_eq!(frames(b"Content-Len"), vec![Err("the stream ended in the middle of a message".to_owned())]);
    assert_eq!(frames(b"Content-Length: 5\r\n\r\n{}"), vec![Err("the stream ended in the middle of a message".to_owned())]);
  }

  #[test]
  fn should_pass_a_body_with_invalid_utf8_through_unchanged() {
    let input = b"Content-Length: 2\r\n\r\n\xff\xfe";

    assert_eq!(frames(input), vec![Ok(vec![0xff, 0xfe])]);
  }

  #[test]
  fn should_return_none_at_a_clean_end_of_stream() {
    assert_eq!(frames(b""), vec![]);
  }
}
