pub fn before() -> i32 {
  1
}

pub struct Early;

impl Early {
  pub fn run(&self) -> i32 {
    before()
  }
}

pub fn middle() -> i32 {
  let value = 1 + ;
  value
}

pub fn after() -> i32 {
  before() + 2
}

pub struct Late;

impl Late {
  pub fn go(&self) -> i32 {
    after()
  }
}
