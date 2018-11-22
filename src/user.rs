use e_num::ENum;
use fuse::{FileAttr, FileType, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request};
use libc::ENOENT;
use rawr::errors::APIError;
use rawr::prelude::*;
use rawr::responses::listing::{Listing, Submission};
use rawr::responses::user::{UserAbout, UserAboutData};
use std::ffi::OsStr;
use std::iter::Extend;

#[derive(Debug, ENum)]
#[e_num(start_at = 2)]
enum Resource {
  #[e_num(constant = 1)]
  Top,
  User(usize),
  LinkKarma(usize),
  CommentKarma(usize),
  Username(usize),
  Created(usize),
  Summary(usize),
  UserPosts(usize),
}

impl Resource {
  pub fn from_ino(ino: u64) -> Resource {
    Resource::from_num(ino as usize)
  }
  pub fn to_ino(&self) -> u64 {
    self.to_num() as u64
  }
  pub fn filetype(&self) -> FileType {
    use self::FileType::*;
    use self::Resource::*;
    match self {
      Top | User(_) | UserPosts(_) => Directory,
      LinkKarma(_) | CommentKarma(_) | Username(_) | Created(_) | Summary(_) => RegularFile,
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
  pub fn fetch(client: &RedditClient, name: String) -> Result<User, APIError> {
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

  pub fn attrs(&self, ino: u64, filetype: FileType, size: u64) -> FileAttr {
    let ts = self.timespec();
    FileAttr {
      ino,
      size,
      blocks: size / 512,
      atime: ts,
      mtime: ts,
      ctime: ts,
      crtime: ts,
      kind: filetype,
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
  user_posts: std::collections::HashMap<String, Vec<Submission>>,
}

fn fetch_user_posts(client: &RedditClient, username: String) -> Result<Vec<Submission>, APIError> {
  let url = format!("/user/{}/submitted?raw_json=1&limit=10", username);
  let result = client.get_json::<Listing>(&url, false)?;
  Ok(
    result
      .data
      .children
      .into_iter()
      .map(|thing| thing.data)
      .collect::<Vec<_>>(),
  )
}

impl UserFS {
  pub fn new(client: RedditClient) -> UserFS {
    UserFS {
      client,
      users: indexmap::IndexMap::default(),
      user_posts: std::collections::HashMap::default(),
    }
  }

  fn get_user_by_name(&mut self, name: String) -> Result<(usize, &User), APIError> {
    let name = name.to_lowercase();
    let entry = self.users.entry(name.clone());
    let i = entry.index();
    use indexmap::map::Entry;
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
      _ => panic!("can't get content of resource"),
    }
  }
  fn resource_len(&self, resource: Resource) -> u64 {
    if resource.filetype() == FileType::RegularFile {
      self.resource_content(resource).len() as u64
    } else {
      0
    }
  }

  fn user_posts(&mut self, username: String) -> Result<&Vec<Submission>, APIError> {
    use std::collections::hash_map::Entry;

    match self.user_posts.entry(username.clone()) {
      Entry::Occupied(o) => Ok(o.into_mut()),
      Entry::Vacant(v) => Ok(v.insert(fetch_user_posts(&self.client, username)?)),
    }
  }
}

fn lookup_user_resource(name: &str, i: usize) -> Option<Resource> {
  Some(match name {
    "linkkarma" => Resource::LinkKarma(i),
    "commentkarma" => Resource::CommentKarma(i),
    "username" => Resource::Username(i),
    "created" => Resource::Created(i),
    "summary" => Resource::Summary(i),
    "_posts" => Resource::UserPosts(i),
    _ => return None,
  })
}

impl fuse::Filesystem for UserFS {
  fn lookup(&mut self, _req: &Request, parent: u64, os_name: &OsStr, reply: ReplyEntry) {
    let name = os_name.to_str().unwrap().to_owned();
    match Resource::from_ino(parent) {
      Resource::Top => {
        if let Ok((i, user)) = self.get_user_by_name(name) {
          reply.entry(
            &user.timespec(),
            &user.attrs(Resource::User(i).to_ino(), FileType::Directory, 0),
            0,
          );
        } else {
          reply.error(ENOENT);
        }
      }
      Resource::User(i) => {
        let resource = match lookup_user_resource(name.as_str(), i) {
          Some(resource) => resource,
          None => return reply.error(ENOENT),
        };
        let user = self.get_user(i);
        reply.entry(
          &user.timespec(),
          &user.attrs(
            resource.to_ino(),
            resource.filetype(),
            self.resource_len(resource),
          ),
          0,
        );
      }
      _ => {}
    }
  }

  fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
    let resource = Resource::from_ino(ino);
    match resource {
      Resource::Top => {
        let ts = time::Timespec::new(0, 0);
        reply.attr(
          &ts,
          &FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: ts,
            mtime: ts,
            ctime: ts,
            crtime: ts,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 0,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            flags: 0,
          },
        );
      }
      Resource::User(val)
      | Resource::UserPosts(val)
      | Resource::LinkKarma(val)
      | Resource::CommentKarma(val)
      | Resource::Username(val)
      | Resource::Created(val)
      | Resource::Summary(val) => {
        let user = self.get_user(val);
        reply.attr(
          &user.timespec(),
          &user.attrs(ino, resource.filetype(), self.resource_len(resource)),
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
    reply: ReplyData,
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
    mut reply: ReplyDirectory,
  ) {
    let mut out: Vec<(u64, FileType, &str)> = vec![
      (1, FileType::Directory, "."),
      (1, FileType::Directory, ".."),
    ];
    match Resource::from_ino(ino) {
      Resource::Top => for (i, user) in self.users.keys().enumerate() {
        out.push((Resource::User(i).to_ino(), FileType::Directory, user));
      },
      Resource::User(idx) => out.extend(
        [
          "linkkarma",
          "commentkarma",
          "username",
          "created",
          "summary",
          "_posts",
        ]
          .iter()
          .map(move |filename| {
            let resource = lookup_user_resource(filename, idx).unwrap();
            (resource.to_ino() as u64, resource.filetype(), *filename)
          }),
      ),
      Resource::UserPosts(idx) => {
        let username = self.get_user(idx).about.name.clone();
        let posts = self.user_posts(username).expect("Couldn't get posts");
        for Submission { .. } in posts.iter() {}
      }
      _ => return reply.error(libc::ENOTDIR),
    };
    for (i, (ino, file_type, filename)) in out.iter().enumerate().skip(offset as usize) {
      reply.add(*ino, i as i64 + 1, *file_type, filename);
    }
    reply.ok();
  }
}
