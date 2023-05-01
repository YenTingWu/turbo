use std::path::PathBuf;

use path_slash::PathBufExt;

use crate::RelativeSystemPathBuf;

pub struct RelativeUnixPath<'a> {
    inner: &'a str,
}

impl<'a> From<&'a str> for RelativeUnixPath<'a> {
    fn from(value: &'a str) -> Self {
        Self { inner: value }
    }
}

impl<'a> RelativeUnixPath<'a> {
    pub fn to_system_path(&self) -> RelativeSystemPathBuf {
        RelativeSystemPathBuf::new_unchecked(PathBuf::from_slash(self.inner))
    }
}
