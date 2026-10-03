use rmcp::model::{Implementation, ServerCapabilities};

#[test]
fn should_build_server_capabilities_with_tools_enabled() {
  let capabilities = ServerCapabilities::builder().enable_tools().build();
  assert!(capabilities.tools.is_some());
  let implementation = Implementation::from_build_env();
  assert!(!implementation.name.is_empty());
  let _stdio: fn() -> (tokio::io::Stdin, tokio::io::Stdout) = rmcp::transport::stdio;
}
