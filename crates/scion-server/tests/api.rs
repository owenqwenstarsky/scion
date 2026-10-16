use reqwest::Method;
use scion_core::BlobId;
use scion_iam::PasswordParams;
use scion_server::{router, App};
use serde_json::Value;
use std::time::Duration;

struct TestServer {
    base: String,
    client: reqwest::Client,
}

struct Session<'a> {
    server: &'a TestServer,
    token: String,
    csrf: String,
    must_change: bool,
}

impl TestServer {
    async fn spawn() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep();
        let app = App::open(
            &path,
            Some("bootstrap-token-is-32-bytes-long!!"),
            Some("web-service-token-is-32-bytes!!"),
            PasswordParams::fast_for_tests(),
            true,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router(app)).await.unwrap();
        });
        Self {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

impl Session<'_> {
    fn req(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.server
            .client
            .request(method, self.server.url(path))
            .header("x-scion-session", &self.token)
            .header("x-scion-csrf", &self.csrf)
    }
}

async fn body_of(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    let value = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    (status, value)
}

#[tokio::test]
async fn vertical_slice_permissions_and_cas() {
    let server = TestServer::spawn().await;
    let created = server
        .client
        .post(server.url("/v1/setup/first-admin"))
        .bearer_auth("bootstrap-token-is-32-bytes-long!!")
        .json(&serde_json::json!({"username": "ada", "password": "correct-horse-1"}))
        .send()
        .await
        .unwrap();
    let (status, _) = body_of(created).await;
    assert_eq!(status, 201);
    let again = server
        .client
        .post(server.url("/v1/setup/first-admin"))
        .bearer_auth("bootstrap-token-is-32-bytes-long!!")
        .json(&serde_json::json!({"username": "bea", "password": "correct-horse-2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409);

    let ada = login(&server, "ada", "correct-horse-1").await;
    assert!(ada.must_change);
    let (status, _) = body_of(
        ada.req(Method::POST, "/v1/repos")
            .json(&serde_json::json!({"name": "widgets"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 403);

    let (status, _) = body_of(
        ada.req(Method::POST, "/v1/auth/change-password")
            .json(&serde_json::json!({"current": "correct-horse-1", "new_password": "correct-horse-2"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 204);
    let ada = login(&server, "ada", "correct-horse-2").await;
    assert!(!ada.must_change);

    let (status, _) = body_of(
        ada.req(Method::POST, "/v1/repos")
            .json(&serde_json::json!({"name": "widgets", "description": "demo"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201);

    let text = "hello scion\n";
    let blob = BlobId::of(text.as_bytes());
    let (status, put) = body_of(
        ada.req(Method::PUT, "/v1/repos/ada/widgets/blobs")
            .body(text.as_bytes().to_vec())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(put["id"], blob.to_hex());

    let (status, cs) = body_of(
        ada.req(Method::POST, "/v1/repos/ada/widgets/changesets")
            .json(&serde_json::json!({
                "parents": [],
                "message": "initial",
                "changes": [{"path": "README", "op": "put", "blob": blob.to_hex()}]
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{cs}");
    let id = cs["id"].as_str().unwrap().to_string();

    let (status, _) = body_of(
        ada.req(Method::POST, "/v1/repos/ada/widgets/land")
            .header("Idempotency-Key", "land-1")
            .json(&serde_json::json!({"changeset": id, "ref": "main", "strategy": "rebase"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let (status, file) = body_of(
        ada.req(Method::GET, "/v1/repos/ada/widgets/file?path=README")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(file["text"], "hello scion\n");

    let (status, _) = body_of(
        ada.req(Method::POST, "/v1/repos/ada/widgets/edits")
            .json(&serde_json::json!({
                "ref": "main",
                "message": "second",
                "changes": [{"path": "README", "op": "put", "text": "hello scion\nagain\n"}]
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let blame = ada
        .req(Method::GET, "/v1/repos/ada/widgets/blame?path=README")
        .send()
        .await
        .unwrap();
    assert_eq!(blame.status(), 200);

    let (status, _) = body_of(
        ada.req(Method::POST, "/v1/users")
            .json(&serde_json::json!({"username": "bea", "password": "bea-password-1"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201);
    let bea = login(&server, "bea", "bea-password-1").await;
    let hidden = bea
        .req(Method::GET, "/v1/repos/ada/widgets")
        .send()
        .await
        .unwrap();
    assert_eq!(hidden.status(), 404);
    let (_, listed) = body_of(bea.req(Method::GET, "/v1/repos").send().await.unwrap()).await;
    assert!(listed["repos"].as_array().unwrap().is_empty());

    let grant = bea_grant(&ada, &server).await;
    assert_eq!(grant, 204);
    assert_eq!(
        bea.req(Method::GET, "/v1/repos/ada/widgets/file?path=README")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        bea.req(Method::POST, "/v1/repos/ada/widgets/edits")
            .json(&serde_json::json!({"message": "nope", "changes": [{"path": "x", "op": "put", "text": "no"}]}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );

    assert_eq!(
        ada.req(Method::POST, "/v1/repos/ada/widgets/path-rules")
            .json(&serde_json::json!({"glob": "secret/**", "can_read": false, "can_write": false, "username": "bea"}))
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    assert_eq!(
        ada.req(Method::POST, "/v1/repos/ada/widgets/edits")
            .json(&serde_json::json!({"message": "secret", "changes": [{"path": "secret/key", "op": "put", "text": "hidden"}]}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        bea.req(Method::GET, "/v1/repos/ada/widgets/file?path=secret/key")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );

    let (_, token) = body_of(
        ada.req(Method::POST, "/v1/tokens")
            .json(&serde_json::json!({"name": "ci", "max_role": "read"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let narrowed = server
        .client
        .post(server.url("/v1/repos/ada/widgets/edits"))
        .bearer_auth(token["token"].as_str().unwrap())
        .json(&serde_json::json!({"message": "widen", "changes": [{"path": "x", "op": "put", "text": "no"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(narrowed.status(), 403);
}

async fn bea_grant(ada: &Session<'_>, _server: &TestServer) -> u16 {
    ada.req(Method::PUT, "/v1/repos/ada/widgets/grants")
        .json(&serde_json::json!({"username": "bea", "role": "read"}))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn login<'a>(server: &'a TestServer, username: &str, password: &str) -> Session<'a> {
    let response = server
        .client
        .post(server.url("/v1/auth/login"))
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    let (status, value) = body_of(response).await;
    assert_eq!(status, 200, "{value}");
    Session {
        server,
        token: value["session_token"].as_str().unwrap().to_string(),
        csrf: value["csrf_token"].as_str().unwrap().to_string(),
        must_change: value["must_change_password"].as_bool().unwrap(),
    }
}
