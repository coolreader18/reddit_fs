extern crate ctrlc;
extern crate fuse;
extern crate indexmap;
extern crate libc;
extern crate rawr;
extern crate time;

use rawr::prelude::*;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::thread;
use std::time::Duration;

mod user;

const UA: &'static str = "linux:reddit_fs:v0.1.1 (by /u/coolreader18)";

fn main() {
  let mountpoint = std::env::args_os().nth(1).unwrap();
  let options = ["-o", "ro", "-o", "fsname=hello"]
    .iter()
    .map(|o| o.as_ref())
    .collect::<Vec<_>>();
  let client = RedditClient::new(UA, AnonymousAuthenticator::new());
  let fs = user::UserFS::new(client);

  let running = Arc::new(AtomicBool::new(true));
  let r = running.clone();

  ctrlc::set_handler(move || {
    r.store(false, Ordering::SeqCst);
  }).expect("Error setting Ctrl-C handler");

  if let Some(str_mountpoint) = mountpoint.to_str() {
    println!("Mounting to {}", str_mountpoint);
  }
  let _fuse_handle = match unsafe { fuse::spawn_mount(fs, &mountpoint, &options) } {
    Ok(handle) => handle,
    Err(err) => return eprintln!("Error mounting: {}", err),
  };

  while running.load(Ordering::SeqCst) {
    thread::sleep(Duration::from_millis(100));
  }
  println!("Unmounting and exiting");
}
