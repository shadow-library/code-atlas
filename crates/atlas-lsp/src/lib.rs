mod client;
mod error;
pub mod framing;
mod position;
mod process;
pub mod protocol;
mod transport;
mod uri;

pub use client::{Client, ClientHandler, ClientOptions, Initialized, NoopHandler, ServerEvent, ShutdownOutcome};
pub use error::{Disconnect, LspError, LspResult};
pub use position::{LineBreaks, LineIndex, PositionEncoding, PositionError, negotiate_encoding};
pub use process::{ProcessExit, STDERR_TAIL_BYTES, ServerCommand, ServerProcess, StderrTail};
pub use transport::{BoxedReader, BoxedWriter, Transport};
pub use uri::{UriError, canonical_path, canonical_uri, path_from_uri, uri_from_path};
