use std::thread;

use shiv_api::{ApiError, DeadlockerClient};
use tiny_http::{Header, Response, Server};

/// Serves `n` requests with `handler`, returning the base URL.
fn stub<F>(n: usize, handler: F) -> String
where
    F: Fn(&tiny_http::Request) -> Response<std::io::Cursor<Vec<u8>>> + Send + 'static,
{
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    thread::spawn(move || {
        for _ in 0..n {
            let Ok(req) = server.recv() else { return };
            let resp = handler(&req);
            let _ = req.respond(resp);
        }
    });
    url
}

fn json(body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

#[test]
fn mod_link_reads_the_named_submission() {
    let url = stub(1, |req| {
        assert_eq!(req.url(), "/mods/mod_1770149787580");
        json(
            r#"{"success":true,"mod":{"id":"mod_1770149787580","title":"QOL Lock",
            "mod_link":"https://gamebanana.com/mods/650634 ","futureField":1}}"#,
        )
    });
    let link = DeadlockerClient::new(&url)
        .mod_link("mod_1770149787580")
        .unwrap();
    assert_eq!(link, "https://gamebanana.com/mods/650634");
}

#[test]
fn an_answer_without_a_mod_link_is_a_parse_error() {
    for body in [
        r#"{"success":true,"mod":{"id":"x"}}"#,
        r#"{"success":true}"#,
        r#"{"success":false,"error":"no such mod"}"#,
        r#"{"success":true,"mod":{"mod_link":"  "}}"#,
    ] {
        let url = stub(1, move |_| json(body));
        let answered = DeadlockerClient::new(&url).mod_link("mod_1");
        assert!(
            matches!(answered, Err(ApiError::Parse(_))),
            "{body} parsed into an answer"
        );
    }
}

#[test]
fn a_missing_page_is_an_http_error() {
    let url = stub(1, |_| {
        Response::from_string("no such page").with_status_code(404)
    });
    let answered = DeadlockerClient::new(&url).mod_link("mod_1");
    assert!(matches!(answered, Err(ApiError::Http { status: 404, .. })));
}

#[test]
fn a_bad_slug_is_refused_before_any_request() {
    // The base points nowhere: a slug that reached the network would fail
    // as a connect error instead of the parse refusal.
    let client = DeadlockerClient::new("http://127.0.0.1:9");
    for slug in ["", "..", "../x", "a b", &"a".repeat(65)] {
        let answered = client.mod_link(slug);
        assert!(
            matches!(answered, Err(ApiError::Parse(_))),
            "{slug:?} was not refused locally"
        );
    }
}
