use std::{env, fs, path::Path};

fn main() {
  let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
  let target = Path::new(&out_dir).join("constants.rs");
  fs::write(target, "pub const MAX_ITEMS: usize = 128;\npub fn default_label() -> &'static str { \"generated\" }\n").expect("write constants");
  println!("cargo:rerun-if-changed=build.rs");
}
