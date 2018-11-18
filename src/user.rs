use fuse;
use indexmap;
use libc;
use rawr;
use time;

use fuse::ReplyEntry;
use fuse::Request;
use libc::ENOENT;
use rawr::prelude::*;
use rawr::responses::user::*;
use std::ffi::OsStr;
use std::iter::Extend;

fn keep_bits(a: u64, n: u64) -> u64 {
  a << 64 - n >> 64 - n
}

#[derive(Debug)]
enum Resource {
  Top,
  User(usize),
  LinkKarma(usize),
  CommentKarma(usize),
  Username(usize),
  Created(usize),
  Summary(usize),
}

impl Resource {
  const TOP_MASK: u64 = 0b00001;
  const USER_MASK: u64 = 0b00010;
  const LINK_KARMA_MASK: u64 = 0b00011;
  const COMMENT_KARMA_MASK: u64 = 0b00100;
  const USERNAME_MASK: u64 = 0b00101;
  const CREATED_MASK: u64 = 0b00110;
  const SUMMARY_MASK: u64 = 0b00111;

  pub fn from_ino(ino: u64) -> Resource {
    let val = ino as usize >> 5;
    match keep_bits(ino, 5) {
      Resource::TOP_MASK => Resource::Top,
      Resource::USER_MASK => Resource::User(val),
      Resource::LINK_KARMA_MASK => Resource::LinkKarma(val),
      Resource::COMMENT_KARMA_MASK => Resource::CommentKarma(val),
      Resource::USERNAME_MASK => Resource::Username(val),
      Resource::CREATED_MASK => Resource::Created(val),
      Resource::SUMMARY_MASK => Resource::Summary(val),
      _ => panic!("Invalid ino type"),
    }
  }

  pub fn to_ino(&self) -> u64 {
    #[inline]
    fn shl(val: &usize, mask: u64) -> u64 {
      (*val as u64) << 5 | mask
    }
    match self {
      Resource::Top => Resource::TOP_MASK,
      Resource::User(val) => shl(val, Resource::USER_MASK),
      Resource::LinkKarma(val) => shl(val, Resource::LINK_KARMA_MASK),
      Resource::CommentKarma(val) => shl(val, Resource::COMMENT_KARMA_MASK),
      Resource::Username(val) => shl(val, Resource::USERNAME_MASK),
      Resource::Created(val) => shl(val, Resource::CREATED_MASK),
      Resource::Summary(val) => shl(val, Resource::SUMMARY_MASK),
    }
  }
}

/// because rawr's user.about() is really restrictive
#[derive(Debug)]
struct User {
  about: UserAboutData,
}

impl User {
  pub fn new(about: UserAboutData) -> User {
    User { about }
  }
  pub fn fetch(client: &RedditClient, name: String) -> Result<User, rawr::errors::APIError> {
    let url = format!("/user/{}/about?raw_json=1", name);
    client
      .get_json::<UserAbout>(&url, false)
      .and_then(|res| Ok(User::new(res.data)))
  }

  pub fn summary(&self) -> String {
    let age = time::get_time() - time::Timespec::new(self.about.created, 0);
    format!(
      r#"{name}
Link Karma: {link_karma}
Comment Karma: {comment_karma}
A redditor for {age} years
"#,
      name = self.about.name,
      link_karma = self.about.link_karma,
      comment_karma = self.about.comment_karma,
      age = age.num_days() / 365
    )
  }

  pub fn attrs(&self, ino: u64, is_dir: bool, size: u64) -> fuse::FileAttr {
    let ts = self.timespec();
    fuse::FileAttr {
      ino,
      size,
      blocks: size / 512,
      atime: ts,
      mtime: ts,
      ctime: ts,
      crtime: ts,
      kind: if is_dir {
        fuse::FileType::Directory
      } else {
        fuse::FileType::RegularFile
      },
      perm: 0o644,
      nlink: 0,
      uid: unsafe { libc::getuid() },
      gid: unsafe { libc::getgid() },
      rdev: 0,
      flags: 0,
    }
  }

  pub fn timespec(&self) -> time::Timespec {
    time::Timespec::new(self.about.created, 0)
  }
}

pub struct UserFS {
  client: RedditClient,
  users: indexmap::IndexMap<String, User>,
}

impl UserFS {
  pub fn new(client: RedditClient) -> UserFS {
    UserFS {
      client,
      users: indexmap::IndexMap::default(),
    }
  }

  fn get_user_by_name(&mut self, name: String) -> Result<(usize, &User), rawr::errors::APIError> {
    use indexmap::map::Entry;
    let entry = self.users.entry(name.clone());
    let i = entry.index();
    let user = match entry {
      Entry::Occupied(o) => o.into_mut(),
      Entry::Vacant(v) => v.insert(User::fetch(&self.client, name)?),
    };
    Ok((i, user))
  }

  fn get_user(&self, idx: usize) -> &User {
    self.users.get_index(idx).unwrap().1
  }

  fn resource_content(&self, resource: Resource) -> String {
    match resource {
      Resource::LinkKarma(idx) => format!("{}\n", self.get_user(idx).about.link_karma),
      Resource::CommentKarma(idx) => format!("{}\n", self.get_user(idx).about.comment_karma),
      Resource::Created(idx) => format!("{}\n", self.get_user(idx).about.created),
      Resource::Username(idx) => format!("{}\n", self.get_user(idx).about.name),
      Resource::Summary(idx) => self.get_user(idx).summary(),
      _ => panic!("invalid resource ino"),
    }
  }
  fn resource_len(&self, resource: Resource) -> u64 {
    self.resource_content(resource).len() as u64
  }
}

fn lookup_user_resource(name: &str, i: usize) -> Option<Resource> {
  match name {
    "linkkarma" => Some(Resource::LinkKarma(i)),
    "commentkarma" => Some(Resource::CommentKarma(i)),
    "username" => Some(Resource::Username(i)),
    "created" => Some(Resource::Created(i)),
    "summary" => Some(Resource::Summary(i)),
    _ => None,
  }
}

impl fuse::Filesystem for UserFS {
  fn lookup(&mut self, _req: &Request, parent: u64, os_name: &OsStr, reply: ReplyEntry) {
    let name = os_name.to_str().unwrap().to_owned();
    match Resource::from_ino(parent) {
      Resource::Top => {
        if let Ok((i, user)) = self.get_user_by_name(name) {
          reply.entry(
            &user.timespec(),
            &user.attrs(Resource::User(i).to_ino(), true, 0),
            0,
          );
        } else {
          reply.error(ENOENT);
        }
      }
      Resource::User(i) => {
        let resource = match lookup_user_resource(name.as_str(), i) {
          Some(resource) => resource,
          None => {
            reply.error(ENOENT);
            return;
          }
        };
        let user = self.get_user(i);
        reply.entry(
          &user.timespec(),
          &user.attrs(resource.to_ino(), false, self.resource_len(resource)),
          0,
        );
      }
      _ => {}
    }
  }

  fn getattr(&mut self, _req: &Request, ino: u64, reply: fuse::ReplyAttr) {
    let resource = Resource::from_ino(ino);
    match resource {
      Resource::Top => {
        let ts = time::Timespec::new(0, 0);
        reply.attr(
          &ts,
          &fuse::FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: ts,
            mtime: ts,
            ctime: ts,
            crtime: ts,
            kind: fuse::FileType::Directory,
            perm: 0o755,
            nlink: 0,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            flags: 0,
          },
        );
      }
      Resource::User(val) => {
        let user = self.get_user(val);
        reply.attr(&user.timespec(), &user.attrs(ino, true, 0));
      }
      Resource::LinkKarma(val)
      | Resource::CommentKarma(val)
      | Resource::Username(val)
      | Resource::Created(val)
      | Resource::Summary(val) => {
        let user = self.get_user(val);
        reply.attr(
          &user.timespec(),
          &user.attrs(ino, false, self.resource_len(resource)),
        );
      }
    }
  }

  fn read(
    &mut self,
    _req: &Request,
    ino: u64,
    _fh: u64,
    _offset: i64,
    _size: u32,
    reply: fuse::ReplyData,
  ) {
    let data = self.resource_content(Resource::from_ino(ino));
    reply.data(data.as_bytes());
  }

  fn readdir(
    &mut self,
    _req: &Request,
    ino: u64,
    _fh: u64,
    offset: i64,
    mut reply: fuse::ReplyDirectory,
  ) {
    let mut out: Vec<(u64, fuse::FileType, &str)> = vec![
      (1, fuse::FileType::Directory, "."),
      (1, fuse::FileType::Directory, ".."),
    ];
    match Resource::from_ino(ino) {
      Resource::User(idx) => out.extend(
        [
          "linkkarma",
          "commentkarma",
          "username",
          "created",
          "summary",
        ]
          .iter()
          .map(move |filename| {
            let ino = lookup_user_resource(filename, idx).unwrap().to_ino() as u64;
            (ino, fuse::FileType::RegularFile, *filename)
          }),
      ),
      Resource::Top => for (i, user) in self.users.keys().enumerate() {
        out.push((Resource::User(i).to_ino(), fuse::FileType::Directory, user));
      },
      _ => return reply.error(libc::ENOTDIR),
    };
    for (i, (ino, file_type, filename)) in out.iter().enumerate().skip(offset as usize) {
      reply.add(*ino, i as i64 + 1, *file_type, filename);
    }
    reply.ok();
  }
}
