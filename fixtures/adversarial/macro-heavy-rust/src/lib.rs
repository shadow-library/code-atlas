macro_rules! define_ops {
  ($($name:ident => $op:tt),* $(,)?) => {
    $(
      pub fn $name(a: i64, b: i64) -> i64 {
        a $op b
      }
    )*
  };
}

macro_rules! getter {
  ($field:ident, $ty:ty) => {
    pub fn $field(&self) -> $ty {
      self.$field
    }
  };
}

define_ops! {
  add => +,
  sub => -,
}

pub struct Point {
  x: i64,
  y: i64,
}

impl Point {
  getter!(x, i64);
  getter!(y, i64);

  pub fn manhattan(&self) -> i64 {
    add(self.x(), self.y())
  }
}

fn helper(value: i64) -> i64 {
  value * 2
}

pub fn plain(value: i64) -> i64 {
  helper(sub(value, 1))
}
