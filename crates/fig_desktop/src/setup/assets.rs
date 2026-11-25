//! Serves the setup page from a fixed table compiled into the binary.

use std::borrow::Cow;

use wry::http::header::{
    CACHE_CONTROL,
    CONTENT_SECURITY_POLICY,
    CONTENT_TYPE,
    X_CONTENT_TYPE_OPTIONS,
};
use wry::http::{
    Method,
    Request,
    Response,
    StatusCode,
};

use super::bridge::HOST;

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const INDEX_HTML: &str = include_str!("page/index.html");
const SETUP_CSS: &str = include_str!("page/setup.css");
const SETUP_JS: &str = include_str!("page/setup.js");
const ICON_PNG: &[u8] = include_bytes!("../../icons/128x128@2x.png");

fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "/" | "/index.html" => Some(("text/html; charset=utf-8", INDEX_HTML.as_bytes())),
        "/setup.css" => Some(("text/css; charset=utf-8", SETUP_CSS.as_bytes())),
        "/setup.js" => Some(("text/javascript; charset=utf-8", SETUP_JS.as_bytes())),
        "/icon.png" => Some(("image/png", ICON_PNG)),
        _ => None,
    }
}

pub fn respond(request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let (status, found) = if *request.method() != Method::GET {
        (StatusCode::METHOD_NOT_ALLOWED, None)
    } else if request.uri().host() != Some(HOST) {
        (StatusCode::BAD_REQUEST, None)
    } else {
        match asset(request.uri().path()) {
            Some(found) => (StatusCode::OK, Some(found)),
            None => (StatusCode::NOT_FOUND, None),
        }
    };

    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_SECURITY_POLICY, CSP)
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(CACHE_CONTROL, "no-store");
    let body: &'static [u8] = match found {
        Some((mime, body)) => {
            response = response.header(CONTENT_TYPE, mime);
            body
        },
        None => &[],
    };
    response.body(Cow::Borrowed(body)).unwrap_or_else(|_| {
        let mut response = Response::new(Cow::Borrowed(&[][..]));
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        response
    })
}

#[cfg(test)]
mod tests {
    use fig_util::consts::{
        CLI_BINARY_NAME,
        PRODUCT_NAME,
    };

    use super::super::bridge::parse_command;
    use super::super::view::{
        AccessibilityStatus,
        InputMethodStatus,
        ShellStatus,
    };
    use super::*;

    const TOKENS: [&str; 6] = [
        "ready",
        "set-up-shell",
        "request-accessibility",
        "enable-input-method",
        "not-now",
        "close",
    ];

    fn get(uri: &str) -> Response<Cow<'static, [u8]>> {
        respond(&Request::builder().uri(uri).body(vec![]).unwrap())
    }

    const PINNED_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

    #[track_caller]
    fn assert_security_headers(response: &Response<Cow<'static, [u8]>>) {
        let headers = response.headers();
        assert_eq!(headers[CONTENT_SECURITY_POLICY], PINNED_CSP);
        let policy = headers[CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(!policy.contains("unsafe"), "{policy}");
        assert!(!policy.contains('*'), "{policy}");
        assert_eq!(headers[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert_eq!(headers.get_all(CONTENT_SECURITY_POLICY).iter().count(), 1);
        assert!(headers.get("Access-Control-Allow-Origin").is_none());
    }

    #[test]
    fn serves_each_asset() {
        for (path, mime) in [
            ("/", "text/html; charset=utf-8"),
            ("/index.html", "text/html; charset=utf-8"),
            ("/setup.css", "text/css; charset=utf-8"),
            ("/setup.js", "text/javascript; charset=utf-8"),
            ("/icon.png", "image/png"),
        ] {
            let response = get(&format!("shellcuesetup://localhost{path}"));
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[CONTENT_TYPE], mime, "{path}");
            assert!(!response.body().is_empty(), "{path}");
            assert_security_headers(&response);
        }
        assert!(get("shellcuesetup://localhost/icon.png").body().starts_with(b"\x89PNG"));
    }

    #[test]
    fn refuses_unknown_paths() {
        for path in [
            "/../Cargo.toml",
            "/%2e%2e/Cargo.toml",
            "/index.htm",
            "/Setup.js",
            "/page/setup.js",
            "/setup.js/",
            "//setup.js",
        ] {
            let response = get(&format!("shellcuesetup://localhost{path}"));
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(response.body().is_empty(), "{path}");
            assert!(response.headers().get(CONTENT_TYPE).is_none(), "{path}");
            assert_security_headers(&response);
        }
    }

    #[test]
    fn refuses_other_methods() {
        for method in [Method::POST, Method::PUT, Method::DELETE, Method::HEAD, Method::OPTIONS] {
            let request = Request::builder()
                .method(method.clone())
                .uri("shellcuesetup://localhost/")
                .body(vec![])
                .unwrap();
            let response = respond(&request);
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{method}");
            assert!(response.body().is_empty());
            assert_security_headers(&response);
        }
    }

    #[test]
    fn refuses_other_hosts() {
        for uri in [
            "shellcuesetup://evil/",
            "shellcuesetup://localhost.evil/index.html",
            "shellcuesetup://127.0.0.1/setup.js",
            "/index.html",
        ] {
            let response = get(uri);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
            assert!(response.body().is_empty());
            assert_security_headers(&response);
        }
    }

    #[test]
    fn page_has_no_inline_code() {
        let html = INDEX_HTML.to_ascii_lowercase();
        let mut rest = html.as_str();
        let mut scripts = 0;
        while let Some(start) = rest.find("<script") {
            let tag_end = start + rest[start..].find('>').unwrap() + 1;
            let tag = &rest[start..tag_end];
            assert!(tag.contains("src=\"setup.js\""), "{tag}");
            assert!(rest[tag_end..].starts_with("</script>"), "{tag}");
            scripts += 1;
            rest = &rest[tag_end..];
        }
        assert_eq!(scripts, 1);
        assert!(!html.contains("style="));
        assert!(!html.contains("<style"));
        assert!(!html.contains("javascript:"));
        for word in html.split(|c: char| c.is_whitespace() || c == '<') {
            assert!(!(word.starts_with("on") && word.contains('=')), "{word}");
        }
    }

    #[test]
    fn page_has_no_external_urls() {
        for text in [INDEX_HTML, SETUP_CSS, SETUP_JS] {
            assert!(!text.contains("http:"));
            assert!(!text.contains("https:"));
            assert!(!text.contains("//"));
            assert!(!text.contains("@import"));
            assert!(!text.contains("url("));
        }
    }

    #[test]
    fn script_avoids_unsafe_sinks() {
        for needle in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
            "Function(",
            "fetch(",
            "XMLHttpRequest",
            "localStorage",
            "sessionStorage",
            "indexedDB",
            "setTimeout",
            "setInterval",
            "import(",
        ] {
            assert!(!SETUP_JS.contains(needle), "{needle}");
        }
    }

    #[test]
    fn script_posts_only_known_tokens() {
        assert_eq!(SETUP_JS.matches("postMessage(").count(), 1);
        assert!(SETUP_JS.contains("window.ipc.postMessage(command)"));

        let mut sent = vec![];
        for (index, _) in SETUP_JS.match_indices("send(") {
            if SETUP_JS[..index].ends_with("function ") {
                continue;
            }
            let rest = SETUP_JS[index + "send(".len()..]
                .strip_prefix('"')
                .unwrap_or_else(|| panic!("send() without a string literal at byte {index}"));
            let token = &rest[..rest.find('"').unwrap()];
            assert!(parse_command(token).is_some(), "{token}");
            assert!(rest[token.len()..].starts_with("\")"), "{token}");
            sent.push(token);
        }
        for token in TOKENS {
            assert!(parse_command(token).is_some(), "{token}");
            assert!(sent.contains(&token), "{token}");
        }
    }

    #[test]
    fn script_knows_every_status() {
        const DEFAULT_BRANCH: [&str; 2] = [r#""needed""#, r#""not_enabled""#];

        let shell = [
            ShellStatus::Needed,
            ShellStatus::Working,
            ShellStatus::Done,
            ShellStatus::Failed,
        ];
        let accessibility = [
            AccessibilityStatus::Needed,
            AccessibilityStatus::Waiting,
            AccessibilityStatus::Granted,
            AccessibilityStatus::TimedOut,
        ];
        let input_method = [
            InputMethodStatus::NotEnabled,
            InputMethodStatus::Working,
            InputMethodStatus::Enabled,
            InputMethodStatus::Failed,
        ];
        let mut names = vec![];
        for status in shell {
            match status {
                ShellStatus::Needed | ShellStatus::Working | ShellStatus::Done | ShellStatus::Failed => {},
            }
            names.push(serde_json::to_string(&status).unwrap());
        }
        for status in accessibility {
            match status {
                AccessibilityStatus::Needed
                | AccessibilityStatus::Waiting
                | AccessibilityStatus::Granted
                | AccessibilityStatus::TimedOut => {},
            }
            names.push(serde_json::to_string(&status).unwrap());
        }
        for status in input_method {
            match status {
                InputMethodStatus::NotEnabled
                | InputMethodStatus::Working
                | InputMethodStatus::Enabled
                | InputMethodStatus::Failed => {},
            }
            names.push(serde_json::to_string(&status).unwrap());
        }
        assert_eq!(names.len(), 12);
        for name in &names {
            assert!(
                SETUP_JS.contains(name.as_str()) || DEFAULT_BRANCH.contains(&name.as_str()),
                "{name}"
            );
        }
        for name in DEFAULT_BRANCH {
            assert!(names.iter().any(|known| known == name), "{name}");
        }
    }

    #[test]
    fn script_ids_exist_in_the_page() {
        let mut checked = 0;
        for helper in ["byId(\"", "setText(\"", "show(\"", "setMark(\""] {
            for (index, _) in SETUP_JS.match_indices(helper) {
                let rest = &SETUP_JS[index + helper.len()..];
                let id = &rest[..rest.find('"').unwrap()];
                assert!(INDEX_HTML.contains(&format!("id=\"{id}\"")), "{helper}{id}");
                checked += 1;
            }
        }
        assert!(checked > 20, "{checked}");
    }

    #[test]
    fn page_names_the_product_and_the_cli() {
        let title = format!("Set up {PRODUCT_NAME}");
        assert!(INDEX_HTML.contains(&format!("<title>{title}</title>")));
        assert!(INDEX_HTML.contains(&format!("<h1>{title}</h1>")));
        assert_eq!(INDEX_HTML.matches("<title>").count(), 1);
        assert_eq!(INDEX_HTML.matches("<h1>").count(), 1);

        let marker = "<span class=\"path\">";
        let mut commands = 0;
        for (index, _) in INDEX_HTML.match_indices(marker) {
            let rest = &INDEX_HTML[index + marker.len()..];
            let command = &rest[..rest.find("</span>").unwrap()];
            assert!(command.starts_with(&format!("{CLI_BINARY_NAME} ")), "{command}");
            commands += 1;
        }
        assert_eq!(commands, 1);
    }
}
