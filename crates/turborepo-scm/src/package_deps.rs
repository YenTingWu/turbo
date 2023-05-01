use std::{
    collections::HashMap,
    ffi::OsString,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
};

use anyhow::{anyhow, Result};
use turbopath::{AbsoluteSystemPathBuf, AnchoredSystemPathBuf, RelativeUnixPathBuf};

type GitHashes = HashMap<RelativeUnixPathBuf, String>;

pub fn get_package_deps(
    turbo_root: &AbsoluteSystemPathBuf,
    package_path: &AnchoredSystemPathBuf,
    inputs: &[&str],
) -> Result<GitHashes> {
    let result = if inputs.len() == 0 {
        let full_pkg_path = turbo_root.resolve(package_path);
        let mut hashes = git_ls_tree(&full_pkg_path)?;
        let to_hash = append_git_status(turbo_root, inputs, &mut hashes)?;
        hashes
    } else {
        unimplemented!()
    };
    Ok(result)
}

fn append_git_status(
    root_path: &AbsoluteSystemPathBuf,
    patterns: &[&str],
    hashes: &mut GitHashes,
) -> Result<Vec<RelativeUnixPathBuf>> {
    let mut to_hash = Vec::new();
    let mut args = vec!["status", "--untracked-files", "--no-renames", "-z", "--"];
    if patterns.len() == 0 {
        args.push(".");
    } else {
        let mut patterns = Vec::from(patterns);
        args.append(&mut patterns);
    }
    let mut git = Command::new("git")
        .args(args.as_slice())
        .current_dir(root_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let stdout = git
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("failed to get stdout for git status"))?;
        let mut reader = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            {
                let bytes_read = reader.read_until(b'\0', &mut buffer)?;
                if bytes_read == 0 {
                    break;
                }
                {
                    let (filename, x, y) = parse_status(&buffer)?;
                    let filename = String::from_utf8(filename.to_vec())?;
                    let filename = std::path::PathBuf::from(OsString::from(filename));
                    let path = RelativeUnixPathBuf::new(filename)?;
                    let is_delete = x == b'D' || y == b'D';
                    if is_delete {
                        hashes.remove(&path);
                    } else {
                        to_hash.push(path);
                    }
                }
            }
        }
    }
    git.wait()?;
    Ok(to_hash)
}

fn git_ls_tree(root_path: &AbsoluteSystemPathBuf) -> Result<GitHashes> {
    let mut hashes = GitHashes::new();
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
        let mut reader = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            {
                let bytes_read = reader.read_until(b'\0', &mut buffer)?;
                if bytes_read == 0 {
                    break;
                }
                {
                    let (filename, hash) = parse_ls_tree(&buffer)?;
                    let filename = String::from_utf8(filename.to_vec())?;
                    let filename = std::path::PathBuf::from(OsString::from(filename));
                    let hash = String::from_utf8(hash.to_vec())?;
                    let path = RelativeUnixPathBuf::new(filename)?;
                    hashes.insert(path, hash);
                }
            }
        }
    }
    git.wait()?;
    Ok(hashes)
}

fn git_hash_object(files_to_hash: Vec<RelativeUnixPathBuf>, &mut hashes: GitHashes) -> Result<()> {
    let mut git = Command::new("git")
        .args(&["hash-object", "--stdin-paths"])
        .current_dir(root_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let stdin = git
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("failed to get stdin for git hash-object"));
        let stdout = git
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("failed to get stdout for git ls-tree"))?;
    }
}

fn parse_status(i: &[u8]) -> Result<(&[u8], u8, u8)> {
    use nom::Finish;
    match nom_parse_status(i).finish() {
        Ok((_, tup)) => Ok(tup),
        Err(e) => Err(anyhow!("nom: {:?} {}", e, std::str::from_utf8(e.input)?)),
    }
}

fn nom_parse_status(i: &[u8]) -> nom::IResult<&[u8], (&[u8], u8, u8)> {
    let (i, x) = nom::bytes::complete::take(1usize)(i)?;
    let (i, y) = nom::bytes::complete::take(1usize)(i)?;
    let (i, _) = nom::character::complete::space1(i)?;
    let (i, filename) = non_space(i)?;
    Ok((i, (filename, x[0], y[0])))
}

fn parse_ls_tree(i: &[u8]) -> Result<(&[u8], &[u8])> {
    use nom::Finish;
    match nom_parse_ls_tree(i).finish() {
        Ok((_, tup)) => Ok(tup),
        Err(e) => Err(anyhow!("nom: {:?}", e)),
    }
}

fn nom_parse_ls_tree(i: &[u8]) -> nom::IResult<&[u8], (&[u8], &[u8])> {
    let (i, _) = non_space(i)?;
    let (i, _) = nom::character::complete::space1(i)?;
    let (i, _) = non_space(i)?;
    let (i, _) = nom::character::complete::space1(i)?;
    let (i, hash) = hash(i)?;
    let (i, _) = nom::character::complete::space1(i)?;
    let (i, filename) = non_space(i)?;
    Ok((i, (filename, hash)))
}

fn non_space(i: &[u8]) -> nom::IResult<&[u8], &[u8]> {
    nom::bytes::complete::is_not(" \0")(i)
}

fn hash(i: &[u8]) -> nom::IResult<&[u8], &[u8]> {
    nom::bytes::complete::take(40usize)(i)
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
