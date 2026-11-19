//! The native source client: a registry is third-party data, so the
//! guarantees pinned here are the ones that keep bad data out — origin
//! policy, document validation and page completion.

use std::thread;

use shiv_api::{NativeSourceClient, resolve_url};
use tiny_http::{Response, Server};

/// Serves every request with `handler`, returning the base URL.
fn stub<F>(handler: F) -> String
where
    F: Fn(&tiny_http::Request) -> Response<std::io::Cursor<Vec<u8>>> + Send + 'static,
{
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let resp = handler(&req);
            let _ = req.respond(resp);
        }
    });
    url
}

/// A stub whose descriptor declares the stub's own origin, so the
/// descriptor passes its own origin check.
fn origin_stub<F>(handler: F) -> String
where
    F: Fn(&str, &tiny_http::Request) -> Response<std::io::Cursor<Vec<u8>>> + Send + 'static,
{
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let origin = url.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let resp = handler(&origin, &req);
            let _ = req.respond(resp);
        }
    });
    url
}

fn json(body: impl Into<String>) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
}

fn descriptor(origin: &str, kind: &str, index_field: &str) -> String {
    format!(
        r#"{{
            "format": "dmm-source",
            "protocolVersion": "1.0",
            "sourceId": "my-community",
            "name": "Example Community",
            "gameId": "deadlock",
            "catalog": {{"kind": "{kind}", "{index_field}": "{origin}/catalog.json"}},
            "networkOrigins": {{
                "api": ["{origin}"],
                "downloads": ["{origin}"],
                "media": ["{origin}"]
            }}
        }}"#
    )
}

#[test]
fn a_descriptor_from_another_game_or_protocol_is_refused() {
    let client = NativeSourceClient::new();

    let wrong_game = r#"{"format":"dmm-source","protocolVersion":"1.0","sourceId":"x","name":"X","gameId":"counter-strike","catalog":{"kind":"static","indexUrl":"http://x/c.json"},"networkOrigins":{"api":["http://x"],"downloads":["http://x"]}}"#.to_owned();
    let served = stub(move |_| json(wrong_game.clone()));
    assert!(client.fetch_descriptor(&format!("{served}/d.json")).is_err());

    let wrong_protocol = r#"{"format":"dmm-source","protocolVersion":"2.0","sourceId":"x","name":"X","gameId":"deadlock","catalog":{"kind":"static","indexUrl":"http://x/c.json"},"networkOrigins":{"api":["http://x"],"downloads":["http://x"]}}"#.to_owned();
    let served = stub(move |_| json(wrong_protocol.clone()));
    assert!(client.fetch_descriptor(&format!("{served}/d.json")).is_err());

    let wrong_format = r#"{"format":"other","protocolVersion":"1.0","sourceId":"x","name":"X","gameId":"deadlock","catalog":{"kind":"static","indexUrl":"http://x/c.json"},"networkOrigins":{"api":["http://x"],"downloads":["http://x"]}}"#.to_owned();
    let served = stub(move |_| json(wrong_format.clone()));
    assert!(client.fetch_descriptor(&format!("{served}/d.json")).is_err());

    let no_origins = r#"{"format":"dmm-source","protocolVersion":"1.0","sourceId":"x","name":"X","gameId":"deadlock","catalog":{"kind":"static","indexUrl":"http://x/c.json"},"networkOrigins":{"api":[],"downloads":[]}}"#.to_owned();
    let served = stub(move |_| json(no_origins.clone()));
    assert!(client.fetch_descriptor(&format!("{served}/d.json")).is_err());

    // A valid descriptor passes: one that declares the origin it is
    // actually served from.
    let served = origin_stub(|origin, _| json(descriptor(origin, "static", "indexUrl")));
    let fetched = client.fetch_descriptor(&format!("{served}/d.json"));
    assert!(fetched.is_ok(), "{fetched:?}");
}

#[test]
fn an_artifact_outside_the_declared_download_origins_is_refused() {
    let origin = origin_stub(move |origin, req| match req.url() {
        url if url.ends_with("/d.json") => json(descriptor(origin, "static", "indexUrl")),
        url if url.ends_with("/catalog.json") => json(
            r#"{"snapshotId":"s1","items":[{"id":"evil","availability":"available",
            "detailUrl":"./evil.json"}]}"#,
        ),
        _ => json(
            r#"{"id":"evil","title":"Evil","availability":"available",
            "release":{"id":"r1","contentRevision":"rev-1",
            "artifacts":[{"id":"a1","filename":"x.vpk","format":"vpk",
            "download":{"url":"https://evil.test/x.vpk"}}]}}"#,
        ),
    });
    let client = NativeSourceClient::new();
    let descriptor = client
        .fetch_descriptor(&format!("{origin}/d.json"))
        .unwrap();
    let refused = client
        .fetch_catalog(&descriptor)
        .map_err(|e| e.to_string())
        .expect_err("an artifact outside the download origins");
    assert!(refused.contains("download origins"), "{refused}");
}

#[test]
fn a_static_catalog_must_complete_one_snapshot() {
    let origin = origin_stub(move |origin, req| match req.url().rsplit('/').next().unwrap_or_default() {
        "d.json" => json(descriptor(origin, "static", "indexUrl")),
        "catalog.json" => json(format!(
            r#"{{"snapshotId":"s1","items":[{{"id":"a","availability":"removed"}}],
            "nextUrl":"{origin}/other.json"}}"#
        )),
        _ => json(r#"{"snapshotId":"s2","items":[{"id":"b","availability":"removed"}]}"#),
    });
    let client = NativeSourceClient::new();
    let descriptor = client
        .fetch_descriptor(&format!("{origin}/d.json"))
        .unwrap();
    let refused = client
        .fetch_catalog(&descriptor)
        .map_err(|e| e.to_string())
        .expect_err("a catalog whose pages describe two snapshots");
    assert!(refused.contains("different snapshots"), "{refused}");
}

#[test]
fn a_catalog_never_lists_a_mod_twice() {
    let origin = origin_stub(move |origin, req| {
        if req.url().ends_with("/d.json") {
            json(descriptor(origin, "api", "listUrl"))
        } else {
            json(
                r#"{"items":[{"id":"a","availability":"removed"},
                {"id":"a","availability":"removed"}],"nextCursor":null}"#,
            )
        }
    });
    let client = NativeSourceClient::new();
    let descriptor = client
        .fetch_descriptor(&format!("{origin}/d.json"))
        .unwrap();
    let refused = client
        .fetch_catalog(&descriptor)
        .map_err(|e| e.to_string())
        .expect_err("a catalog that lists one mod twice");
    assert!(refused.contains("twice"), "{refused}");
}

#[test]
fn an_api_catalog_follows_cursors_until_null() {
    let origin = origin_stub(move |origin, req| match req.url() {
        url if url.ends_with("/d.json") => json(descriptor(origin, "api", "listUrl")),
        url if url.contains("cursor=page-2") => {
            json(r#"{"items":[{"id":"b","availability":"removed"}],"nextCursor":null}"#)
        }
        url if url.ends_with("/catalog.json") => json(
            r#"{"items":[{"id":"a","availability":"removed"}],"nextCursor":"page-2"}"#,
        ),
        other => panic!("unexpected request {other}"),
    });
    let client = NativeSourceClient::new();
    let descriptor = client
        .fetch_descriptor(&format!("{origin}/d.json"))
        .unwrap();
    let catalog = client.fetch_catalog(&descriptor).unwrap();
    let ids: Vec<&str> = catalog.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn a_release_with_an_unsafe_filename_is_refused() {
    let origin = origin_stub(move |origin, req| {
        if req.url().ends_with("/d.json") {
            json(descriptor(origin, "static", "indexUrl"))
        } else if req.url().ends_with("/catalog.json") {
            json(
                r#"{"snapshotId":"s1","items":[{"id":"x","availability":"available",
                "detailUrl":"./x.json"}]}"#,
            )
        } else {
            json(
                r#"{"id":"x","title":"Traverse","availability":"available",
                "release":{"id":"r1","contentRevision":"rev-1",
                "artifacts":[{"id":"a1","filename":"../../escape.vpk","format":"vpk",
                "download":{"url":"./x.vpk"}}]}}"#,
            )
        }
    });
    let client = NativeSourceClient::new();
    let descriptor = client
        .fetch_descriptor(&format!("{origin}/d.json"))
        .unwrap();
    let refused = client
        .fetch_catalog(&descriptor)
        .map_err(|e| e.to_string())
        .expect_err("a release with an unsafe file name");
    assert!(refused.contains("safe file name"), "{refused}");
}

#[test]
fn relative_urls_resolve_against_their_document() {
    let base = "https://mods.example.org/dmm/mods/example.json";
    assert_eq!(
        resolve_url(base, "../files/example.vpk"),
        "https://mods.example.org/dmm/files/example.vpk"
    );
    assert_eq!(
        resolve_url(base, "/.well-known/other.json"),
        "https://mods.example.org/.well-known/other.json"
    );
    assert_eq!(
        resolve_url(base, "https://cdn.example.org/x.vpk"),
        "https://cdn.example.org/x.vpk"
    );
    assert_eq!(
        resolve_url("https://host/a/b/c.json", "d/e.json"),
        "https://host/a/b/d/e.json"
    );
}

#[test]
fn a_full_static_source_parses_end_to_end() {
    let (descriptor, catalog, detail) = (
        r#"{"format":"dmm-source","protocolVersion":"1.0",
           "sourceId":"my-community","name":"Example Community","gameId":"deadlock",
           "catalog":{"kind":"static","indexUrl":"./catalog.json"},
           "networkOrigins":{"api":["ORIGIN"],"downloads":["ORIGIN"],"media":[]}}"#,
        r#"{"snapshotId":"s1","items":[{"id":"example","availability":"available",
           "detailUrl":"./mods/example.json"}]}"#,
        r#"{"id":"example","title":"Harmless Example","availability":"available",
           "isNsfw":false,"author":"Example Community","description":"One VPK.",
           "release":{"id":"release-1","contentRevision":"revision-1","version":"1.0.0",
           "artifacts":[{"id":"example-vpk","filename":"example.vpk","format":"vpk",
           "download":{"url":"../files/example.vpk"},"size":141}],
           "installation":{"mode":"manual"}}}"#,
    );
    let origin = origin_stub(move |origin, req| {
        let _ = origin;
        match req.url() {
            url if url.ends_with("/d.json") => {
                json(descriptor.replace("ORIGIN", &stub_origin_of(req)))
            }
            url if url.ends_with("/catalog.json") => json(catalog),
            _ => json(detail),
        }
    });
    let client = NativeSourceClient::new();
    let source = client
        .fetch_descriptor(&format!("{origin}/d.json"))
        .expect("descriptor");
    let listings = client.fetch_catalog(&source).expect("catalog");
    assert_eq!(listings.len(), 1);
    assert!(listings[0].installable(), "{:?}", listings[0]);
}

/// The stub's own origin, recovered from the request URL the handler sees.
fn stub_origin_of(req: &tiny_http::Request) -> String {
    let host = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Host"))
        .map(|h| h.value.to_string())
        .unwrap_or_default();
    format!("http://{host}")
}
