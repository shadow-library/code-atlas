pub struct Outer { pub value: u32 }
pub struct Other { pub count: u32 }
impl Outer {
  pub fn helper(&self) {}
  pub fn run(&self) -> u32 {
    self.helper();
    struct Inner { count: u32 }
    impl Inner {
      fn helper(&self) {}
      fn go(&self) -> u32 { self.helper(); self.count }
    }
    self.value
  }
  pub fn only_inner(&self) {
    trait Local { fn helper(&self) {} fn go(&self) { self.helper(); } }
  }
}
