use std::collections::HashMap;

use anyhow::Result;
use turbopath::{AbsoluteSystemPathBuf, AnchoredSystemPathBuf};

pub fn get_package_deps(
    turbo_root: &AbsoluteSystemPathBuf,
    package_path: &AnchoredSystemPathBuf,
    inputs: &[&str],
) -> Result<()> {
    if inputs.len() == 0 {
    } else {
        unimplemented!()
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> Result<(tempfile::TempDir, AbsoluteSystemPathBuf)> {
        let tmp_dir = tempfile::tempdir()?;
        let dir = AbsoluteSystemPathBuf::new(tmp_dir.path().to_path_buf())?;
        Ok((tmp_dir, dir))
    }

    #[test]
    fn test_get_package_deps() -> Result<()> {
        // Directory structure:
        // <root>/
        //   new-root-file <- new file not added to git
        //   my-pkg/
        //     committed-file
        //     deleted-file
        //     uncommitted-file <- new file not added to git
        //     dir/
        //       nested-file
        let (_repo_root_tmp, repo_root) = tmp_dir()?;
        my_pkg_dir = repo_root.join_literal("my-pkg");

        let package_path = AnchoredSystemPathBuf::from_raw("my-pkg")?;
        get_package_deps(&repo_root, &package_path, &[])?;
        Ok(())
    }
}
