// Cargo exposes CARGO_BIN_EXE_<name> only to the tests of the package that defines the binary, so atlas-cli builds its own copy of the fake server.
include!("../../../atlas-lsp/src/bin/atlas-fake-lsp.rs");
