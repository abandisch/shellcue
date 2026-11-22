use std::borrow::Cow;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use fig_os_shim::Context;
use percent_encoding::percent_decode_str;
use wry::http::header::CONTENT_TYPE;
use wry::http::{
    Request,
    Response,
    StatusCode,
};

use super::util::{
    res_400,
    res_404,
    res_500,
};
use crate::webview::WindowId;

// handle `spec://localhost/spec.js`
pub async fn handle(
    ctx: Arc<Context>,
    request: Request<Vec<u8>>,
    _: WindowId,
) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    let specs_path = fig_util::directories::resources_path_ctx(&ctx)?.join("specs");
    serve(&specs_path, request.uri().host(), request.uri().path())
}

fn serve(root: &Path, host: Option<&str>, uri_path: &str) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    if host != Some("localhost") {
        return res_400();
    }

    // the root itself may be a symlink, so compare canonical forms
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(err) if err.kind() == ErrorKind::NotFound => return res_404(),
        Err(err) => return res_500(err),
    };

    let Ok(decoded) = percent_decode_str(uri_path).decode_utf8() else {
        return res_400();
    };
    let relative = decoded.trim_start_matches('/');

    let path = match root.join(relative).canonicalize() {
        Ok(path) => path,
        Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => return res_404(),
        Err(err) => return res_500(err),
    };

    // dont allow escaping the specs dir
    if !path.starts_with(&root) {
        return res_400();
    }

    let content_type: &str = match path.extension().and_then(|ext| ext.to_str()) {
        Some("js") => mime::APPLICATION_JAVASCRIPT_UTF_8.as_ref(),
        Some("json") => mime::APPLICATION_JSON.as_ref(),
        _ => return res_404(),
    };

    if !path.is_file() {
        return res_404();
    }

    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return res_404(),
        Err(err) => return res_500(err),
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .body(content.into())?)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn specs_dir(dir: &Path) -> std::path::PathBuf {
        let root = dir.join("specs");
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn content_type<'a>(res: &'a Response<Cow<'static, [u8]>>) -> &'a str {
        res.headers()[CONTENT_TYPE].to_str().unwrap()
    }

    #[test]
    fn serves_index_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        let index = br#"{"completions":["git"],"diffVersionedCompletions":[]}"#;
        fs::write(root.join("index.json"), index).unwrap();

        let res = serve(&root, Some("localhost"), "/index.json").unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(content_type(&res), "application/json");
        assert_eq!(&res.body()[..], index);
    }

    #[test]
    fn serves_nested_js() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::create_dir_all(root.join("aws")).unwrap();
        fs::write(root.join("aws/s3.js"), b"export default {};").unwrap();

        let res = serve(&root, Some("localhost"), "/aws/s3.js").unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(content_type(&res), "application/javascript; charset=utf-8");
        assert_eq!(&res.body()[..], b"export default {};");
    }

    #[test]
    fn refuses_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::write(dir.path().join("secret.js"), b"secret").unwrap();

        let res = serve(&root, Some("localhost"), "/../secret.js").unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_ne!(&res.body()[..], b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_out_of_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::write(dir.path().join("secret.js"), b"secret").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret.js"), root.join("link.js")).unwrap();

        let res = serve(&root, Some("localhost"), "/link.js").unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_ne!(&res.body()[..], b"secret");
    }

    #[test]
    fn refuses_percent_encoded_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::write(dir.path().join("secret.js"), b"secret").unwrap();

        let res = serve(&root, Some("localhost"), "/%2e%2e/secret.js").unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_ne!(&res.body()[..], b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn serves_from_symlinked_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        let index = br#"{"completions":["git"],"diffVersionedCompletions":[]}"#;
        fs::write(root.join("index.json"), index).unwrap();
        let link = dir.path().join("specs-link");
        std::os::unix::fs::symlink(&root, &link).unwrap();

        let res = serve(&link, Some("localhost"), "/index.json").unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(content_type(&res), "application/json");
        assert_eq!(&res.body()[..], index);
    }

    #[test]
    fn wrong_host_is_bad_request() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::write(root.join("git.js"), b"export default {};").unwrap();

        let res = serve(&root, Some("example.com"), "/git.js").unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn unknown_extension_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::write(root.join("x.png"), b"png").unwrap();

        let res = serve(&root, Some("localhost"), "/x.png").unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn html_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::write(root.join("x.html"), b"<html></html>").unwrap();

        let res = serve(&root, Some("localhost"), "/x.html").unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());

        let res = serve(&root, Some("localhost"), "/missing.js").unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn missing_root_is_not_found() {
        let dir = tempfile::tempdir().unwrap();

        let res = serve(&dir.path().join("specs"), Some("localhost"), "/index.json").unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn serves_percent_encoded_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = specs_dir(dir.path());
        fs::create_dir_all(root.join("@fig")).unwrap();
        fs::write(root.join("@fig/x.js"), b"export default {};").unwrap();

        let res = serve(&root, Some("localhost"), "/%40fig/x.js").unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(content_type(&res), "application/javascript; charset=utf-8");
        assert_eq!(&res.body()[..], b"export default {};");
    }
}
