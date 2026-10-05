use std::io::{self, Read, Write};

pub type BoxedReader = Box<dyn Read + Send>;
pub type BoxedWriter = Box<dyn Write + Send>;

pub struct Transport {
  reader: BoxedReader,
  writer: BoxedWriter,
}

impl Transport {
  pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
    Self {
      reader: Box::new(reader),
      writer: Box::new(writer),
    }
  }

  pub fn pair() -> io::Result<(Self, Self)> {
    let (client_reader, server_writer) = io::pipe()?;
    let (server_reader, client_writer) = io::pipe()?;
    Ok((Self::new(client_reader, client_writer), Self::new(server_reader, server_writer)))
  }

  pub fn into_parts(self) -> (BoxedReader, BoxedWriter) {
    (self.reader, self.writer)
  }
}
