// straitjacket-allow-file:duplication (generated)
// GENERATED — Magnus binding skeleton. Mirrors the hand-written patterns of entl-ruby.
// Ruby's GVL serialises access; unary calls run inline (nogvl is a per-op opt-in later).
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;
use magnus::{function, method, prelude::*, Error, RHash, Ruby};
use crate::core::{self, Poll, PollStream};
use crate::core::EntlCore;

fn rberr(e: impl std::fmt::Display) -> Error {
    let ruby = Ruby::get().expect("called outside the Ruby GVL");
    Error::new(ruby.exception_runtime_error(), e.to_string())
}

fn git_stats_hash(ruby: &Ruby, v: core::GitStats) -> Result<RHash, Error> {
    let h = ruby.hash_new();
    h.aset("new_commits", v.new_commits)?;
    h.aset("file_changes", v.file_changes)?;
    Ok(h)
}

fn change_batch_hash(ruby: &Ruby, v: core::ChangeBatch) -> Result<RHash, Error> {
    let h = ruby.hash_new();
    h.aset("table", v.table)?;
    h.aset("op", v.op)?;
    h.aset("rows_json", v.rows_json)?;
    Ok(h)
}

/// Poll-based stream dressed as `.next` (nil at end) — wrap with an Enumerator in Ruby.
#[magnus::wrap(class = "Entl::Changes", free_immediately, size)]
pub struct Changes { stream: RefCell<Box<dyn PollStream<core::ChangeBatch>>> }
impl Changes {
    fn next(ruby: &Ruby, rb_self: &Self) -> Result<Option<RHash>, Error> {
        loop {
            match rb_self.stream.borrow().poll(Duration::from_millis(500)) {
                Poll::Item(b) => return Ok(Some(change_batch_hash(ruby, b)?)),
                Poll::Idle => continue,
                Poll::Closed => return Ok(None),
            }
        }
    }
}
/// An open entl database.
#[magnus::wrap(class = "Entl", free_immediately, size)]
pub struct Entl { core: Arc<core::Impl> }

impl Entl {
    fn new(db_path: String) -> Result<Self, Error> {
        Ok(Self { core: Arc::new(core::Impl::open(&db_path).map_err(rberr)?) })
    }
    fn load_git(ruby: &Ruby, rb_self: &Self, repo_path: String) -> Result<RHash, Error> {
        let v = rb_self.core.load_git(&repo_path).map_err(rberr)?;
        git_stats_hash(ruby, v)
    }
    fn query(&self, sql: String) -> Result<String, Error> {
        self.core.query(&sql).map_err(rberr)
    }
    fn changes(&self, repo_path: String, github: bool) -> Result<Changes, Error> {
        let stream = self.core.changes(&repo_path, github).map_err(rberr)?;
        Ok(Changes { stream: RefCell::new(stream) })
    }
}

pub fn register(ruby: &Ruby) -> Result<(), Error> {
    let class = ruby.define_class("Entl", ruby.class_object())?;
    class.define_singleton_method("new", function!(Entl::new, 1))?;
    class.define_method("load_git", method!(Entl::load_git, 1))?;
    class.define_method("query", method!(Entl::query, 1))?;
    class.define_method("changes", method!(Entl::changes, 2))?;
    let s = class.define_class("Changes", ruby.class_object())?;
    s.define_method("next", method!(Changes::next, 0))?;
    Ok(())
}
