use std::{
    collections::HashMap,
    io::BufReader,
    process::{Command, Stdio},
};

use anyhow::{anyhow, Result};
use turbopath::{
    AbsoluteSystemPathBuf, AnchoredSystemPathBuf, RelativeSystemPathBuf, RelativeUnixPathBuf,
};

type GitHashes = HashMap<RelativeUnixPathBuf, String>;

pub fn get_package_deps(
    turbo_root: &AbsoluteSystemPathBuf,
    package_path: &AnchoredSystemPathBuf,
    inputs: &[&str],
) -> Result<GitHashes> {
    let result = if inputs.len() == 0 {
        let full_pkg_path = turbo_root.resolve(package_path);
        git_ls_tree(&full_pkg_path)?
    } else {
        unimplemented!()
    };
    Ok(result)
}

fn git_ls_tree(root_path: &AbsoluteSystemPathBuf) -> Result<GitHashes> {
    let mut git = Command::new("git")
        .args(&["ls-tree", "-r", "-z", "HEAD"])
        .current_dir(root_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let stdout = git
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("failed to get stdout for git ls-tree"))?;
        let reader = BufReader::new(stdout);
    }
    git.wait()?;
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> Result<(tempfile::TempDir, AbsoluteSystemPathBuf)> {
        let tmp_dir = tempfile::tempdir()?;
        let dir = AbsoluteSystemPathBuf::new(tmp_dir.path().to_path_buf())?;
        Ok((tmp_dir, dir))
    }

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

    fn to_hash_map(pairs: &[(&str, &str)]) -> GitHashes {
        HashMap::from_iter(
            pairs
                .into_iter()
                .map(|(path, hash)| (RelativeUnixPathBuf::new_unchecked(path), hash.to_string())),
        )
    }
}
