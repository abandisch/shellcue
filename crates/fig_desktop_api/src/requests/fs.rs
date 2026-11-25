use fig_os_shim::{
    Env,
    Fs,
};
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    ReadFileRequest,
    ReadFileResponse,
};

use super::RequestResult;
use crate::util::resolve_filepath;

pub async fn read_file(request: ReadFileRequest, env: &Env, fs: &Fs) -> RequestResult {
    use fig_proto::fig::read_file_response::Type;
    let path = request.path.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env);
    let kind = if request.is_binary_file() {
        Type::Data(
            fs.read(&*resolved_path)
                .await
                .map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))?,
        )
    } else {
        Type::Text(
            fs.read_to_string(&*resolved_path)
                .await
                .map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))?,
        )
    };
    let response = ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse { r#type: Some(kind) });

    Ok(response.into())
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use fig_proto::fig::{
        FilePath,
        ReadFileRequest,
    };

    use super::*;

    #[tokio::test]
    async fn test_read_file() {
        use fig_proto::fig::read_file_response::Type;

        let fs = Fs::new_fake();
        let env = Env::new();

        let path = Utf8PathBuf::from("/test.txt");
        let content = "Hello, World!";
        fs.write(&path, content.as_bytes()).await.unwrap();

        let mut request = ReadFileRequest {
            path: Some(FilePath {
                path: path.to_string(),
                relative_to: None,
                expand_tilde_in_path: None,
            }),
            ..Default::default()
        };

        let result = read_file(request.clone(), &env, &fs).await.unwrap();
        let text = match *result {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Text(text)),
            }) => text,
            _ => panic!("Unexpected response type"),
        };
        assert_eq!(content, text);

        request.is_binary_file = Some(true);

        let result = read_file(request, &env, &fs).await.unwrap();
        let data = match *result {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Data(data)),
            }) => data,
            _ => panic!("Unexpected response type"),
        };
        assert_eq!(content.as_bytes(), &data[..]);
    }

    #[tokio::test]
    async fn test_read_file_missing_is_error() {
        let fs = Fs::new_fake();
        let env = Env::new();

        let request = ReadFileRequest {
            path: Some(FilePath {
                path: "/missing.txt".into(),
                relative_to: None,
                expand_tilde_in_path: None,
            }),
            ..Default::default()
        };

        let err = read_file(request, &env, &fs).await.unwrap_err();
        assert!(err.to_string().starts_with("Failed reading file at /missing.txt"));
    }

    #[tokio::test]
    async fn test_read_file_without_path_is_error() {
        let fs = Fs::new_fake();
        let env = Env::new();

        let err = read_file(ReadFileRequest::default(), &env, &fs).await.unwrap_err();
        assert_eq!(err.to_string(), "No path provided");
    }
}
