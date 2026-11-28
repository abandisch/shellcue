use std::sync::LazyLock;

use url::Url;

pub const SCHEME: &str = "qcliresource";
pub const HOST: &str = "localhost";

pub static RESOURCE_URL: LazyLock<Url> = LazyLock::new(|| Url::parse(&format!("{SCHEME}://{HOST}")).unwrap());

pub fn url() -> Url {
    RESOURCE_URL.clone()
}

/// Used for both the sender of every request and every navigation.
pub fn is_autocomplete_origin(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        url.scheme() == SCHEME
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str() == Some(HOST)
            && url.port().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocomplete_origin_accepts_the_page() {
        assert!(is_autocomplete_origin("qcliresource://localhost"));
        assert!(is_autocomplete_origin("qcliresource://localhost/"));
        assert!(is_autocomplete_origin("qcliresource://localhost:"));
        assert!(is_autocomplete_origin("qcliresource://localhost/index.html"));
        assert!(is_autocomplete_origin("qcliresource://localhost/index.html#top"));
        assert!(is_autocomplete_origin(url().as_str()));
    }

    #[test]
    fn autocomplete_origin_rejects_other_schemes() {
        for url in [
            "api://localhost/",
            "spec://localhost/",
            "fig://localhost/",
            "icon://localhost/",
            "shellcuesetup://localhost/",
            "http://localhost/",
            "https://localhost/",
            "file:///etc/passwd",
            "data:text/html,hello",
            "javascript:alert(1)",
            "about:blank",
            "blob:qcliresource://localhost/1234",
            "qcliresource:/localhost/",
        ] {
            assert!(!is_autocomplete_origin(url), "{url:?}");
        }
    }

    #[test]
    fn autocomplete_origin_rejects_other_hosts() {
        for url in [
            "qcliresource://evil/",
            "qcliresource://localhost.evil/",
            "qcliresource://a.localhost/",
            "qcliresource://evillocalhost/",
            "qcliresource://localhost./",
            "qcliresource://LOCALHOST/",
            "qcliresource://127.0.0.1/",
            "qcliresource:///index.html",
        ] {
            assert!(!is_autocomplete_origin(url), "{url:?}");
        }
    }

    #[test]
    fn autocomplete_origin_rejects_ports_and_userinfo() {
        for url in [
            "qcliresource://localhost:8080/",
            "qcliresource://localhost:0/",
            "qcliresource://x@localhost/",
            "qcliresource://x:y@localhost/",
            "qcliresource://:y@localhost/",
            "qcliresource://localhost@evil/",
        ] {
            assert!(!is_autocomplete_origin(url), "{url:?}");
        }
    }

    #[test]
    fn autocomplete_origin_rejects_garbage() {
        for url in [
            "",
            " ",
            "localhost",
            "qcliresource",
            "qcliresource://",
            "://localhost/",
            "\0",
        ] {
            assert!(!is_autocomplete_origin(url), "{url:?}");
        }
    }
}
