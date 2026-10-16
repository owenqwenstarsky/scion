use crate::workspace::IndexEntry;
use scion_core::BlobId;
use serde_json::{json, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{status}: {body}")]
    Status { status: u16, body: String },
    #[error("{0}")]
    Msg(String),
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    session: Option<String>,
    csrf: Option<String>,
    token: Option<String>,
}

impl Client {
    pub fn new(base: impl Into<String>, insecure: bool) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .map_err(|err| Error::Msg(err.to_string()))?;
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            session: None,
            csrf: None,
            token: None,
        })
    }

    pub fn with_session(mut self, session: impl Into<String>, csrf: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self.csrf = Some(csrf.into());
        self
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<Value, Error> {
        self.send(
            self.http
                .post(self.url("/v1/auth/login"))
                .json(&json!({"username": username, "password": password})),
        )
        .await
    }

    pub async fn session(&self) -> Result<Value, Error> {
        self.send(self.authed(self.http.get(self.url("/v1/auth/session"))))
            .await
    }

    pub async fn change_password(&self, current: &str, new_password: &str) -> Result<(), Error> {
        let response = self
            .send_raw(
                self.authed(self.http.post(self.url("/v1/auth/change-password")))
                    .json(&json!({"current": current, "new_password": new_password})),
            )
            .await?;
        status_ok(&response)
    }

    pub async fn list_repos(&self) -> Result<Value, Error> {
        self.send(self.authed(self.http.get(self.url("/v1/repos"))))
            .await
    }

    pub async fn create_repo(&self, name: &str, description: &str) -> Result<Value, Error> {
        self.send(
            self.authed(self.http.post(self.url("/v1/repos")))
                .json(&json!({"name": name, "description": description})),
        )
        .await
    }

    pub async fn get_repo(&self, owner: &str, name: &str) -> Result<Value, Error> {
        self.send(
            self.authed(
                self.http
                    .get(self.url(&format!("/v1/repos/{owner}/{name}"))),
            ),
        )
        .await
    }

    pub async fn tree(
        &self,
        owner: &str,
        name: &str,
        rev: &str,
        path: &str,
    ) -> Result<Value, Error> {
        let mut url = format!("/v1/repos/{owner}/{name}/tree?ref={}", urlenc(rev));
        if !path.is_empty() {
            url.push_str("&path=");
            url.push_str(&urlenc(path));
        }
        self.send(self.authed(self.http.get(self.url(&url)))).await
    }

    pub async fn file(
        &self,
        owner: &str,
        name: &str,
        rev: &str,
        path: &str,
    ) -> Result<Value, Error> {
        let url = format!(
            "/v1/repos/{owner}/{name}/file?ref={}&path={}",
            urlenc(rev),
            urlenc(path)
        );
        self.send(self.authed(self.http.get(self.url(&url)))).await
    }

    pub async fn log(
        &self,
        owner: &str,
        name: &str,
        rev: &str,
        limit: u32,
    ) -> Result<Value, Error> {
        let url = format!(
            "/v1/repos/{owner}/{name}/log?ref={}&limit={limit}",
            urlenc(rev)
        );
        self.send(self.authed(self.http.get(self.url(&url)))).await
    }

    pub async fn blame(
        &self,
        owner: &str,
        name: &str,
        rev: &str,
        path: &str,
    ) -> Result<Value, Error> {
        let url = format!(
            "/v1/repos/{owner}/{name}/blame?ref={}&path={}",
            urlenc(rev),
            urlenc(path)
        );
        self.send(self.authed(self.http.get(self.url(&url)))).await
    }

    pub async fn diff(&self, owner: &str, name: &str, rev: &str) -> Result<Value, Error> {
        let url = format!("/v1/repos/{owner}/{name}/diff?ref={}", urlenc(rev));
        self.send(self.authed(self.http.get(self.url(&url)))).await
    }

    pub async fn put_blob(&self, owner: &str, name: &str, bytes: &[u8]) -> Result<String, Error> {
        let value = self
            .send(
                self.authed(
                    self.http
                        .put(self.url(&format!("/v1/repos/{owner}/{name}/blobs"))),
                )
                .body(bytes.to_vec()),
            )
            .await?;
        value["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Msg("blob response missing id".into()))
    }

    pub async fn create_changeset(
        &self,
        owner: &str,
        name: &str,
        parent: Option<&str>,
        message: &str,
        changes: &[Value],
    ) -> Result<Value, Error> {
        let parents = parent.map(|id| vec![id.to_string()]).unwrap_or_default();
        self.send(
            self.authed(
                self.http
                    .post(self.url(&format!("/v1/repos/{owner}/{name}/changesets"))),
            )
            .json(&json!({"parents": parents, "message": message, "changes": changes})),
        )
        .await
    }

    pub async fn land(
        &self,
        owner: &str,
        name: &str,
        changeset: &str,
        ref_name: &str,
        strategy: &str,
    ) -> Result<Value, Error> {
        self.send(
            self.authed(
                self.http
                    .post(self.url(&format!("/v1/repos/{owner}/{name}/land"))),
            )
            .json(&json!({"changeset": changeset, "ref": ref_name, "strategy": strategy})),
        )
        .await
    }

    pub async fn list_files(
        &self,
        owner: &str,
        name: &str,
        rev: &str,
    ) -> Result<Vec<IndexEntry>, Error> {
        let mut out = Vec::new();
        self.walk(owner, name, rev, "", &mut out).await?;
        Ok(out)
    }

    fn walk<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
        rev: &'a str,
        path: &'a str,
        out: &'a mut Vec<IndexEntry>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let tree = self.tree(owner, name, rev, path).await?;
            let entries = tree["entries"].as_array().cloned().unwrap_or_default();
            for entry in entries {
                let entry_name = entry["name"].as_str().unwrap_or("");
                let full = if path.is_empty() {
                    entry_name.to_string()
                } else {
                    format!("{path}/{entry_name}")
                };
                if entry["type"] == "dir" {
                    self.walk(owner, name, rev, &full, out).await?;
                } else if let Some(blob) = entry["blob"].as_str() {
                    out.push(IndexEntry {
                        path: full,
                        blob: blob.to_string(),
                    });
                }
            }
            Ok(())
        })
    }

    pub fn hash_file(bytes: &[u8]) -> String {
        BlobId::of(bytes).to_hex()
    }

    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut builder = builder;
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        if let Some(session) = &self.session {
            builder = builder.header("x-scion-session", session);
        }
        if let Some(csrf) = &self.csrf {
            builder = builder.header("x-scion-csrf", csrf);
        }
        builder
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<Value, Error> {
        let response = self.send_raw(builder).await?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|err| Error::Msg(err.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(Error::Status { status, body: text });
        }
        if text.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(&text).map_err(|err| Error::Msg(format!("{err}: {text}")))
        }
    }

    async fn send_raw(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        builder
            .send()
            .await
            .map_err(|err| Error::Msg(err.to_string()))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

fn status_ok(response: &reqwest::Response) -> Result<(), Error> {
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(Error::Status {
            status,
            body: response.status().to_string(),
        })
    }
}

fn urlenc(value: &str) -> String {
    urlencoding_min(value)
}

fn urlencoding_min(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
