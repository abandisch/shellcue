use std::path::Path;

use fig_os_shim::{
    Env,
    Fs,
};
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    ReadFileRequest,
    ReadFileResponse,
};

use super::{
    RequestResult,
    RequestResultImpl,
};
use crate::util::resolve_filepath;

const SHELLS_PATH: &str = "/etc/shells";
const PATH_NOT_ALLOWED: &str = "Reading this path is not allowed";

/// Every path must already be canonical; only `/etc/shells` and `<themes>/*.json` pass.
fn is_readable(path: &Path, themes_dir: Option<&Path>, shells: Option<&Path>) -> bool {
    let in_themes_dir = themes_dir.is_some_and(|dir| path.parent() == Some(dir));
    shells == Some(path) || (in_themes_dir && path.extension().is_some_and(|ext| ext == "json"))
}

pub async fn read_file(request: ReadFileRequest, env: &Env, fs: &Fs, themes_dir: Option<&Path>) -> RequestResult {
    use fig_proto::fig::read_file_response::Type;
    let path = request.path.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env);
    // A path that cannot be resolved must be indistinguishable from a forbidden one.
    let Ok(canonical_path) = fs.canonicalize(&*resolved_path).await else {
        return RequestResult::error(PATH_NOT_ALLOWED);
    };
    let themes_dir = match themes_dir {
        Some(dir) => fs.canonicalize(dir).await.ok(),
        None => None,
    };
    let shells = fs.canonicalize(SHELLS_PATH).await.ok();
    if !is_readable(&canonical_path, themes_dir.as_deref(), shells.as_deref()) {
        return RequestResult::error(PATH_NOT_ALLOWED);
    }
    let kind = if request.is_binary_file() {
        Type::Data(
            fs.read(&canonical_path)
                .await
                .map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))?,
        )
    } else {
        Type::Text(
            fs.read_to_string(&canonical_path)
                .await
                .map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))?,
        )
    };
    let response = ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse { r#type: Some(kind) });

    Ok(response.into())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fig_proto::fig::{
        FilePath,
        ReadFileRequest,
    };

    use super::*;

    const THEMES: &str = "/res/themes";
    const SHELLS: &str = "/private/etc/shells";

    fn readable(path: &str, themes_dir: Option<&str>) -> bool {
        is_readable(Path::new(path), themes_dir.map(Path::new), Some(Path::new(SHELLS)))
    }

    fn read_request(path: &str) -> ReadFileRequest {
        ReadFileRequest {
            path: Some(FilePath {
                path: path.into(),
                relative_to: None,
                expand_tilde_in_path: None,
            }),
            ..Default::default()
        }
    }

    fn is_refused(result: &RequestResult) -> bool {
        matches!(result.as_deref(), Ok(ServerOriginatedSubMessage::Error(msg)) if msg == PATH_NOT_ALLOWED)
    }

    fn read_text(result: RequestResult) -> Option<String> {
        use fig_proto::fig::read_file_response::Type;

        match *result.ok()? {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Text(text)),
            }) => Some(text),
            _ => None,
        }
    }

    /// A real directory tree, removed when the returned chroot handle drops.
    async fn themes_tree(fs: &Fs) -> (Fs, PathBuf) {
        let tempdir = Fs::new_chroot();
        let root = tempdir.chroot_path("/");
        fs.create_dir_all(root.join("res/themes")).await.unwrap();
        fs.create_dir_all(root.join("etc")).await.unwrap();
        fs.write(root.join("res/themes/test.json"), "Hello, World!").await.unwrap();
        fs.write(root.join("res/themes/test.txt"), "text").await.unwrap();
        fs.write(root.join("etc/passwd"), "root").await.unwrap();
        (tempdir, root)
    }

    #[test]
    fn allows_etc_shells() {
        assert!(readable(SHELLS, Some(THEMES)));
        assert!(readable(SHELLS, None));
    }

    #[test]
    fn allows_json_directly_in_themes() {
        assert!(readable("/res/themes/x.json", Some(THEMES)));
        assert!(readable("/res/themes/dark.json", Some("/res/themes/")));
    }

    #[test]
    fn refuses_other_files_in_themes() {
        for path in [
            "/res/themes/x.txt",
            "/res/themes/x",
            "/res/themes/.json",
            "/res/themes/x.json.txt",
            "/res/themes/x.JSON",
        ] {
            assert!(!readable(path, Some(THEMES)), "{path:?}");
        }
    }

    #[test]
    fn refuses_nested_and_sibling_paths() {
        for path in [
            "/res/themes/sub/x.json",
            "/res/themes",
            "/res/x.json",
            "/res/themes-evil/x.json",
            "/res/themesx.json",
            "/",
        ] {
            assert!(!readable(path, Some(THEMES)), "{path:?}");
        }
    }

    #[test]
    fn refuses_system_and_home_files() {
        for path in [
            "/etc/passwd",
            "/private/etc/passwd",
            "/etc/shells.json",
            "/private/etc/shells/x.json",
            "/Users/user/.zshrc",
            "/home/user/.ssh/id_ed25519",
            "/home/user/x.json",
        ] {
            assert!(!readable(path, Some(THEMES)), "{path:?}");
        }
    }

    #[test]
    fn refuses_everything_but_shells_without_themes_dir() {
        for path in ["/res/themes/x.json", "/x.json", "/etc/passwd"] {
            assert!(!readable(path, None), "{path:?}");
        }
        assert!(!is_readable(Path::new(SHELLS), None, None));
    }

    #[tokio::test]
    async fn test_read_file() {
        use fig_proto::fig::read_file_response::Type;

        let fs = Fs::new();
        let env = Env::new();
        let (_tempdir, root) = themes_tree(&fs).await;
        let themes_dir = root.join("res/themes");
        let path = themes_dir.join("test.json");
        let content = "Hello, World!";

        let mut request = read_request(path.to_str().unwrap());

        let result = read_file(request.clone(), &env, &fs, Some(&themes_dir)).await.unwrap();
        let text = match *result {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Text(text)),
            }) => text,
            _ => panic!("Unexpected response type"),
        };
        assert_eq!(content, text);

        request.is_binary_file = Some(true);

        let result = read_file(request, &env, &fs, Some(&themes_dir)).await.unwrap();
        let data = match *result {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Data(data)),
            }) => data,
            _ => panic!("Unexpected response type"),
        };
        assert_eq!(content.as_bytes(), &data[..]);
    }

    #[tokio::test]
    async fn test_read_file_refuses_paths_outside_themes() {
        let fs = Fs::new();
        let env = Env::new();
        let (_tempdir, root) = themes_tree(&fs).await;
        let themes_dir = root.join("res/themes");

        for path in [
            themes_dir.join("../../etc/passwd"),
            themes_dir.join("test.txt"),
            root.join("etc/passwd"),
            PathBuf::from("/etc/passwd"),
        ] {
            let result = read_file(read_request(path.to_str().unwrap()), &env, &fs, Some(&themes_dir)).await;
            assert!(is_refused(&result), "{path:?}");
        }

        let path = themes_dir.join("test.json");
        let result = read_file(read_request(path.to_str().unwrap()), &env, &fs, None).await;
        assert!(is_refused(&result), "{path:?}");
    }

    #[tokio::test]
    async fn test_read_file_refuses_missing_paths_like_forbidden_ones() {
        let fs = Fs::new();
        let env = Env::new();
        let (_tempdir, root) = themes_tree(&fs).await;
        let themes_dir = root.join("res/themes");

        for path in [
            PathBuf::from("/missing.txt"),
            themes_dir.join("absent.json"),
            root.join("etc/passwd"),
        ] {
            let result = read_file(read_request(path.to_str().unwrap()), &env, &fs, Some(&themes_dir)).await;
            assert!(is_refused(&result), "{path:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_read_file_refuses_symlink_out_of_themes() {
        let fs = Fs::new();
        let env = Env::new();
        let (_tempdir, root) = themes_tree(&fs).await;
        let root = fs.canonicalize(&root).await.unwrap();
        let themes_dir = root.join("res/themes");
        let link = themes_dir.join("evil.json");
        std::os::unix::fs::symlink(root.join("etc/passwd"), &link).unwrap();

        let result = read_file(read_request(link.to_str().unwrap()), &env, &fs, Some(&themes_dir)).await;
        assert!(is_refused(&result));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_read_file_follows_symlinked_themes_dir() {
        let fs = Fs::new();
        let env = Env::new();
        let (_tempdir, root) = themes_tree(&fs).await;
        let root = fs.canonicalize(&root).await.unwrap();
        let themes_link = root.join("themes-link");
        std::os::unix::fs::symlink(root.join("res/themes"), &themes_link).unwrap();
        let path = themes_link.join("test.json");

        let result = read_file(read_request(path.to_str().unwrap()), &env, &fs, Some(&themes_link)).await;
        assert_eq!(read_text(result).as_deref(), Some("Hello, World!"));
    }

    #[tokio::test]
    async fn test_read_file_reads_real_etc_shells() {
        let result = read_file(read_request(SHELLS_PATH), &Env::new(), &Fs::new(), None).await;
        assert!(read_text(result).is_some_and(|text| !text.is_empty()));
    }

    #[tokio::test]
    async fn test_read_file_without_path_is_error() {
        let fs = Fs::new_fake();
        let env = Env::new();

        let err = read_file(ReadFileRequest::default(), &env, &fs, None).await.unwrap_err();
        assert_eq!(err.to_string(), "No path provided");
    }
}
