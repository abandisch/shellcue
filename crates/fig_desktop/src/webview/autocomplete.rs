use std::sync::LazyLock;

use url::Url;

pub static RESOURCE_URL: LazyLock<Url> = LazyLock::new(|| Url::parse("qcliresource://localhost").unwrap());

pub fn url() -> Url {
    RESOURCE_URL.clone()
}
