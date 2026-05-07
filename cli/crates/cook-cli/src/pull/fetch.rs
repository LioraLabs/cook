//! HTTP fetch for the registry tarball. Single ureq agent per call with a
//! 60-second timeout and a `cook/<version>` User-Agent.

use std::io::Read;
use std::time::Duration;

use super::errors::PullError;

/// Build the archive URL for a given registry base. Forge tarballs live at
/// `<base>/archive/main.tar.gz`.
pub fn archive_url(registry_base: &str) -> String {
    format!("{}/archive/main.tar.gz", registry_base)
}

/// Fetch the archive at `archive_url` and return the response body as a
/// streaming reader.
pub fn fetch_archive(archive_url: &str) -> Result<Box<dyn Read + Send + Sync>, PullError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("cook/", env!("CARGO_PKG_VERSION")))
        .build();

    let resp = agent
        .get(archive_url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(status, _) => PullError::Network {
                url: archive_url.to_string(),
                source: format!("HTTP {status}").into(),
            },
            other => PullError::Network {
                url: archive_url.to_string(),
                source: Box::new(other),
            },
        })?;

    Ok(resp.into_reader())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_url_appends_path() {
        assert_eq!(
            archive_url("https://example.test/r"),
            "https://example.test/r/archive/main.tar.gz"
        );
    }

    #[test]
    fn fetch_reads_body_from_mock_server() {
        let mut server = mockito::Server::new();
        let body = b"PRETEND TARBALL BYTES";
        let m = server
            .mock("GET", "/archive/main.tar.gz")
            .with_status(200)
            .with_header("content-type", "application/gzip")
            .with_body(body)
            .create();

        let url = format!("{}/archive/main.tar.gz", server.url());
        let mut reader = fetch_archive(&url).unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();

        assert_eq!(buf, body);
        m.assert();
    }

    #[test]
    fn http_error_status_maps_to_network_error() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/archive/main.tar.gz")
            .with_status(404)
            .create();

        let url = format!("{}/archive/main.tar.gz", server.url());
        let err = fetch_archive(&url).err().expect("expected Err but got Ok");
        match err {
            PullError::Network { url: u, .. } => assert!(u.contains("/archive/main.tar.gz")),
            other => panic!("wrong variant: {other:?}"),
        }
        m.assert();
    }

    #[test]
    fn unreachable_url_maps_to_network_error() {
        // Use a port that refuses connections (closed loopback port).
        let url = "http://127.0.0.1:1/archive/main.tar.gz";
        let err = fetch_archive(url).err().expect("expected Err but got Ok");
        assert!(matches!(err, PullError::Network { .. }));
    }
}
