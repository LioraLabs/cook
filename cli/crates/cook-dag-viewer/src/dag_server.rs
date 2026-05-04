//! HTTP server for the DAG viewer.

use crate::ViewerError;

const HTML_TEMPLATE: &str = include_str!("dag_viewer.html");

pub fn serve_dag(dag_json: &str) -> Result<(), ViewerError> {
    let html = HTML_TEMPLATE.replace("/*DAG_DATA_PLACEHOLDER*/{}", dag_json);

    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| ViewerError::ServerStart(e.to_string()))?;

    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(0);
    let url = format!("http://127.0.0.1:{port}");

    eprintln!("cook: DAG viewer at {url}");
    eprintln!("cook: press Ctrl+C to stop");

    let _ = open::that(&url);

    loop {
        let request = match server.recv() {
            Ok(r) => r,
            Err(_) => break,
        };

        let response = tiny_http::Response::from_string(&html).with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                .unwrap(),
        );

        let _ = request.respond(response);
    }

    Ok(())
}
