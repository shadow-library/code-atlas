use std::ops::Deref;

use atlas_core::RunId;
use rusqlite::Connection;

pub struct ReadTx<'c> {
  conn: &'c Connection,
}

impl<'c> ReadTx<'c> {
  pub(crate) fn new(conn: &'c Connection) -> Self {
    Self { conn }
  }

  pub(crate) fn conn(&self) -> &'c Connection {
    self.conn
  }
}

pub struct WriteTx<'c> {
  read: ReadTx<'c>,
  active_run: Option<RunId>,
}

impl<'c> WriteTx<'c> {
  pub(crate) fn new(conn: &'c Connection) -> Self {
    Self {
      read: ReadTx::new(conn),
      active_run: None,
    }
  }

  pub fn active_run(&self) -> Option<RunId> {
    self.active_run
  }

  pub(crate) fn set_active_run(&mut self, run: Option<RunId>) {
    self.active_run = run;
  }
}

impl<'c> Deref for WriteTx<'c> {
  type Target = ReadTx<'c>;

  fn deref(&self) -> &Self::Target {
    &self.read
  }
}
