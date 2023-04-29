use std::{collections::HashMap, process::Command};

use anyhow::Result;
use turbopath::{AbsoluteSystemPathBuf, AnchoredSystemPathBuf, RelativeSystemPathBuf};

pub fn get_package_deps(
    turbo_root: &AbsoluteSystemPathBuf,
    package_path: &AnchoredSystemPathBuf,
    inputs: &[&str],
) -> Result<HashMap<RelativeSystemPathBuf, String>> {
    if inputs.len() == 0 {
    } else {
        unimplemented!()
    }
    Ok(HashMap::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    //use git2::{Oid, Repository};

    fn tmp_dir() -> Result<(tempfile::TempDir, AbsoluteSystemPathBuf)> {
        let tmp_dir = tempfile::tempdir()?;
        let dir = AbsoluteSystemPathBuf::new(tmp_dir.path().to_path_buf())?;
        Ok((tmp_dir, dir))
    }

    // fn setup_repository(repo_root: &AbsoluteSystemPathBuf) -> Result<Repository>
    // {     let repo = Repository::init(repo_root.as_path())?;
    //     let mut config = repo.config()?;
    //     config.set_str("user.name", "test")?;
    //     config.set_str("user.email", "test@example.com")?;

    //     Ok(repo)
    // }

    fn require_git_cmd(repo_root: &AbsoluteSystemPathBuf, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(repo_root);
        assert_eq!(cmd.output().unwrap().status.success(), true);
    }

    fn setup_repository(repo_root: &AbsoluteSystemPathBuf) {
        let cmds: &[&[&str]] = &[
            &["init", "."],
            &["config", "--local", "user.name", "test"],
            &["config", "--local", "user.email", "test@example.com"],
        ];
        for cmd in cmds {
            require_git_cmd(repo_root, cmd);
        }
    }

    fn commit_all(repo_root: &AbsoluteSystemPathBuf) {
        let cmds: &[&[&str]] = &[&["add", "."], &["commit", "-m", "foo"]];
        for cmd in cmds {
            require_git_cmd(repo_root, cmd);
        }
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
        let my_pkg_dir = repo_root.join_literal("my-pkg");
        my_pkg_dir.create_dir()?;

        // create file 1
        let committed_file_path = my_pkg_dir.join_literal("committed-file");
        committed_file_path.create_with_contents("committed bytes")?;

        // create file 2
        let deleted_file_path = my_pkg_dir.join_literal("deleted-file");
        deleted_file_path.create_with_contents("delete-me")?;

        // create file 3
        let nested_file_path = my_pkg_dir.join_literal("dir/nested-file");
        nested_file_path.ensure_dir()?;
        nested_file_path.create_with_contents("nested")?;

        // create a package.json
        let pkg_json_path = my_pkg_dir.join_literal("package.json");
        pkg_json_path.create_with_contents("{}")?;

        setup_repository(&repo_root);
        commit_all(&repo_root);

        // remove a file
        deleted_file_path.remove()?;

        // create another untracked file in git
        let uncommitted_file_path = my_pkg_dir.join_literal("uncommitted-file");
        uncommitted_file_path.create_with_contents("uncommitted bytes")?;

        // create an untracked file in git up a level
        let root_file_path = repo_root.join_literal("new-root-file");
        root_file_path.create_with_contents("new-root bytes")?;

        let package_path = AnchoredSystemPathBuf::from_raw("my-pkg")?;

        let expected = to_hash_map(&[
            ("committed-file", "3a29e62ea9ba15c4a4009d1f605d391cdd262033"),
            (
                "uncommitted-file",
                "4e56ad89387e6379e4e91ddfe9872cf6a72c9976",
            ),
            ("package.json", "9e26dfeeb6e641a33dae4961196235bdb965b21b"),
            (
                "dir/nested-file",
                "bfe53d766e64d78f80050b73cd1c88095bc70abb",
            ),
        ]);
        let hashes = get_package_deps(&repo_root, &package_path, &[])?;
        assert_eq!(hashes, expected);
        Ok(())
    }

    fn to_hash_map(pairs: &[(&str, &str)]) -> HashMap<RelativeSystemPathBuf, String> {
        let pairs: Vec<(RelativeSystemPathBuf, String)> = pairs
            .into_iter()
            .map(|(path, hash)| (RelativeSystemPathBuf::new(path).unwrap(), hash.to_string()))
            .collect::<Vec<_>>();
        HashMap::from_iter(pairs.into_iter())
    }
}
