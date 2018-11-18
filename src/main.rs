extern crate fuse;
extern crate indexmap;
extern crate libc;
extern crate rawr;
extern crate time;

use rawr::prelude::*;

mod user;

const UA: &'static str = "redditfs";

fn main() {
  let mountpoint = std::env::args_os().nth(1).unwrap();
  let options = ["-o", "ro", "-o", "fsname=hello"]
    .iter()
    .map(|o| o.as_ref())
    .collect::<Vec<_>>();
  let client = RedditClient::new(UA, AnonymousAuthenticator::new());
  let fs = user::UserFS::new(client);
  if let Err(err) = fuse::mount(fs, &mountpoint, &options) {
    println!("Error: {}", err);
  }
}
