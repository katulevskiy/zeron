use crate::{Error, Result, array, config::Config, domain, flag, now, s, store::Store};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

pub fn verify_webhook(raw: &[u8], signature: &str, secret: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some(signature) = signature
        .strip_prefix("sha256=")
        .and_then(|s| hex::decode(s).ok())
    else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(raw);
    mac.verify_slice(&signature).is_ok()
}
fn encode(value: &str) -> String {
    reqwest::Url::parse("https://example.com")
        .unwrap()
        .query_pairs_mut()
        .append_pair("x", value)
        .finish()
        .query()
        .unwrap()[2..]
        .replace('+', "%20")
}
pub fn summary(item: &Value, claims: &[Value], config: &Config) -> String {
    let assessment = domain::assess(item, &config.policy);
    let mut body = format!(
        "<!-- zeron-review -->\n### Zeron review queue\n\n**{}** · {} risk · {}\n\n[Open review workspace]({}/?item={}) · Revision `{}`\n\n",
        assessment.state.replace('-', " "),
        s(item, "risk"),
        s(item, "area"),
        config.public_url,
        encode(s(item, "id")),
        s(item, "revision").chars().take(8).collect::<String>()
    );
    if !assessment.blockers.is_empty() {
        body.push_str("Next actions:\n");
        for blocker in assessment.blockers {
            body.push_str(&format!("- {}\n", blocker.replace('\n', " ")));
        }
        body.push('\n');
    } else if s(item, "kind") == "pr" {
        body.push_str("Required review evidence is complete. Final merge and integration rules still apply.\n\n");
    } else {
        body.push_str("Roadmap placement is recorded. Track reproduction and linked implementations on GitHub.\n\n");
    }
    if !claims.is_empty() {
        body.push_str("Active work:\n");
        for claim in claims {
            body.push_str(&format!(
                "- {}: @{}, until {}\n",
                s(claim, "task"),
                s(claim, "actor"),
                s(claim, "expires")
            ));
        }
        body.push('\n');
    }
    body.push_str("This summary is maintained by the review app. Decisions and test evidence are recorded in the workspace; the app does not merge PRs.");
    body
}
type ItemLocks = Mutex<HashMap<String, Arc<Mutex<()>>>>;
#[derive(Clone)]
pub struct GitHub {
    pub config: Arc<Config>,
    pub client: Client,
    token: Arc<Mutex<Option<(String, i64)>>>,
    sync_locks: Arc<ItemLocks>,
}
impl GitHub {
    pub fn new(config: Arc<Config>) -> Result<Self> {
        Ok(Self {
            config,
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("zeron-review")
                .build()?,
            token: Arc::new(Mutex::new(None)),
            sync_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    pub async fn request_token(
        &self,
        path: &str,
        method: Method,
        body: Option<&Value>,
        token: &str,
    ) -> Result<Value> {
        let mut request = self
            .client
            .request(method.clone(), format!("{}{path}", self.config.api_url))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(Error::new(
                502,
                format!("GitHub {method} {path}: {}", response.status().as_u16()),
            ));
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(Value::Null)
        } else {
            Ok(response.json().await?)
        }
    }
    async fn token(&self) -> Result<String> {
        let mut cached = self.token.lock().await;
        if let Some((token, until)) = &*cached
            && *until > Utc::now().timestamp_millis()
        {
            return Ok(token.clone());
        }
        let config = &self.config;
        if [
            &config.app_id,
            &config.installation_id,
            &config.private_key_path,
        ]
        .iter()
        .any(|s| s.is_empty())
        {
            return Ok(config.read_token.clone());
        }
        let key =
            jsonwebtoken::EncodingKey::from_rsa_pem(&std::fs::read(&config.private_key_path)?)
                .map_err(Error::internal)?;
        let now = Utc::now().timestamp();
        let jwt = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &json!({"iat":now-60,"exp":now+540,"iss":config.app_id}),
            &key,
        )
        .map_err(Error::internal)?;
        let value = self
            .request_token(
                &format!(
                    "/app/installations/{}/access_tokens",
                    config.installation_id
                ),
                Method::POST,
                None,
                &jwt,
            )
            .await?;
        let token = s(&value, "token").to_owned();
        let expires = DateTime::parse_from_rfc3339(s(&value, "expires_at"))
            .map_err(Error::internal)?
            .timestamp_millis()
            - 60_000;
        *cached = Some((token.clone(), expires));
        Ok(token)
    }
    pub async fn request(&self, path: &str, method: Method, body: Option<&Value>) -> Result<Value> {
        self.request_token(path, method, body, &self.token().await?)
            .await
    }
    pub async fn pages(&self, path: &str) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        for page in 1..=100 {
            let value = self
                .request(
                    &format!(
                        "{path}{}per_page=100&page={page}",
                        if path.contains('?') { '&' } else { '?' }
                    ),
                    Method::GET,
                    None,
                )
                .await?;
            let rows = value
                .as_array()
                .ok_or_else(|| Error::internal("GitHub returned a non-array page"))?;
            result.extend(rows.iter().cloned());
            if rows.len() < 100 {
                return Ok(result);
            }
        }
        Err(Error::internal("Repository exceeds the pagination limit"))
    }
    pub async fn sync_item(&self, store: &Store, number: u64, kind: &str) -> Result<Value> {
        // Fetch and commit in order for this item. Local domain actions remain independent.
        let gate = self
            .sync_locks
            .lock()
            .await
            .entry(format!("{kind}:{number}"))
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = gate.lock().await;
        let prefix = format!("/repos/{}", self.config.policy.repository);
        let raw = self
            .request(
                &format!(
                    "{prefix}/{}/{number}",
                    if kind == "pr" { "pulls" } else { "issues" }
                ),
                Method::GET,
                None,
            )
            .await?;
        let id = format!("{kind}:{number}");
        let read_id = id.clone();
        let previous = store.read(move |db| db.get(&read_id)).await?;
        let mut source = json!({"id":id,"number":number,"kind":kind,"title":raw["title"],"author":raw["user"]["login"],"url":raw["html_url"],"createdAt":raw["created_at"],
            "updatedAt":raw["updated_at"],"closed":s(&raw,"state") == "closed","revision":if kind == "pr" {raw["head"]["sha"].clone()} else {raw["updated_at"].clone()},
            "labels":array(&raw,"labels").iter().map(|l| l["name"].clone()).collect::<Vec<_>>(),"syncedAt":now()});
        if kind == "pr" {
            source["baseRevision"] = raw["base"]["sha"].clone();
            source["draft"] = raw["draft"].clone();
            source["merged"] = (!raw["merged_at"].is_null()).into();
            let reviews = self
                .pages(&format!("{prefix}/pulls/{number}/reviews"))
                .await?;
            source["nativeReviews"]=json!(reviews.iter().filter(|r| ["APPROVED","CHANGES_REQUESTED","DISMISSED"].contains(&s(r,"state"))).map(|r| json!({"actor":r["user"]["login"],"revision":r["commit_id"],"source":"github-review","sourceId":r["id"],
                "verdict":match s(r,"state") {"APPROVED"=>"pass","CHANGES_REQUESTED"=>"changes",_=>"dismissed"},"summary":if s(r,"body").is_empty() {"Native GitHub review"} else {s(r,"body")},"at":r["submitted_at"]})).collect::<Vec<_>>());
            let mut checks = vec![];
            for name in &self.config.policy.required_checks {
                let runs=self.request(&format!("{prefix}/commits/{}/check-runs?check_name={}&filter=latest&per_page=100",s(&source,"revision"),encode(name)),Method::GET,None).await?;
                if let Some(run) = array(&runs, "check_runs")
                    .into_iter()
                    .max_by_key(|r| r["id"].as_u64().unwrap_or(0))
                {
                    let old = previous
                        .as_ref()
                        .map(|p| array(p, "checks"))
                        .unwrap_or_default()
                        .into_iter()
                        .find(|c| {
                            s(c, "name") == name
                                && c["runId"] == run["id"]
                                && c["completedAt"] == run["completed_at"]
                        });
                    let base = old
                        .filter(|c| !s(c, "baseRevision").is_empty())
                        .map(|c| c["baseRevision"].clone())
                        .unwrap_or_else(|| source["baseRevision"].clone());
                    checks.push(json!({"name":name,"revision":source["revision"],"baseRevision":base,"conclusion":if s(&run,"conclusion").is_empty() {"pending"} else {s(&run,"conclusion")},"runId":run["id"],"completedAt":run["completed_at"],"url":run["html_url"]}));
                }
            }
            source["checks"] = json!(checks);
        }
        store.write(move |db| domain::ingest(db, &source)).await
    }
    pub async fn reconcile(&self, store: &Store) -> Result<()> {
        let rows = self
            .pages(&format!(
                "/repos/{}/issues?state=open",
                self.config.policy.repository
            ))
            .await?;
        let mut open = HashSet::new();
        for row in rows {
            let kind = if row["pull_request"].is_object() {
                "pr"
            } else {
                "issue"
            };
            let number = row["number"]
                .as_u64()
                .ok_or_else(|| Error::internal("Invalid issue number"))?;
            open.insert(format!("{kind}:{number}"));
            self.sync_item(store, number, kind).await?;
        }
        for item in store.read(|db| db.list()).await? {
            if !flag(&item, "closed") && !flag(&item, "sample") && !open.contains(s(&item, "id")) {
                self.sync_item(
                    store,
                    item["number"].as_u64().unwrap_or(0),
                    s(&item, "kind"),
                )
                .await?;
            }
        }
        store
            .write(|db| db.set("lastReconciled", &json!(now())))
            .await
    }
    pub async fn writeback(&self, store: &Store, item: &Value) -> Result<()> {
        if flag(item, "sample") {
            return Ok(());
        }
        let prefix = format!("/repos/{}", self.config.policy.repository);
        let number = item["number"].as_u64().unwrap_or(0);
        let label = format!("review:{}", domain::assess(item, &self.config.policy).state);
        if let Err(error) = self
            .request(
                &format!("{prefix}/labels/{}", encode(&label)),
                Method::GET,
                None,
            )
            .await
        {
            if !error.message.ends_with(": 404") {
                return Err(error);
            }
            self.request(
                &format!("{prefix}/labels"),
                Method::POST,
                Some(
                    &json!({"name":label,"color":"8b5cf6","description":"Managed by Zeron review"}),
                ),
            )
            .await?;
        }
        let existing = self
            .request(
                &format!("{prefix}/issues/{number}/labels"),
                Method::GET,
                None,
            )
            .await?;
        for old in existing.as_array().unwrap_or(&vec![]) {
            if s(old, "name").starts_with("review:") && s(old, "name") != label {
                self.request(
                    &format!("{prefix}/issues/{number}/labels/{}", encode(s(old, "name"))),
                    Method::DELETE,
                    None,
                )
                .await?;
            }
        }
        if !existing
            .as_array()
            .is_some_and(|rows| rows.iter().any(|l| s(l, "name") == label))
        {
            self.request(
                &format!("{prefix}/issues/{number}/labels"),
                Method::POST,
                Some(&json!({"labels":[label]})),
            )
            .await?;
        }
        let context_id = s(item, "id").to_owned();
        let claims = store.read(move |db| db.claims(&context_id)).await?;
        let body = summary(item, &claims, &self.config);
        let key = format!("comment:{}", s(item, "id"));
        let body_key = format!("comment-body:{}", s(item, "id"));
        let read_key = key.clone();
        let read_body_key = body_key.clone();
        let (comment_id, previous) = store
            .read(move |db| Ok((db.value(&read_key)?, db.value(&read_body_key)?)))
            .await?;
        if previous.as_str() != Some(&body) {
            let mut result = None;
            if let Some(id) = comment_id.as_u64() {
                match self
                    .request(
                        &format!("{prefix}/issues/comments/{id}"),
                        Method::PATCH,
                        Some(&json!({"body":body})),
                    )
                    .await
                {
                    Ok(comment) => result = Some(comment),
                    Err(e) if e.message.ends_with(": 404") => {}
                    Err(e) => return Err(e),
                }
            }
            let comment = match result {
                Some(c) => c,
                None => {
                    self.request(
                        &format!("{prefix}/issues/{number}/comments"),
                        Method::POST,
                        Some(&json!({"body":body})),
                    )
                    .await?
                }
            };
            let saved_body = body.clone();
            store
                .write(move |db| {
                    db.set(&key, &comment["id"])?;
                    db.set(&body_key, &json!(saved_body))
                })
                .await?;
        }
        if s(item, "kind") == "pr" {
            let ready = domain::assess(item, &self.config.policy).state == "ready-to-merge";
            let check = json!({"name":"zeron-review/readiness","head_sha":item["revision"],"status":"completed","conclusion":if flag(item,"closed") {"cancelled"} else if ready {"success"} else {"action_required"},
                "details_url":format!("{}/?item={}",self.config.public_url,encode(s(item,"id"))),"output":{"title":if ready {"Review evidence complete"} else {"Review evidence incomplete"},"summary":body}});
            let key = format!("check-body:{}", s(item, "id"));
            let read_key = key.clone();
            if store.read(move |db| db.value(&read_key)).await? == json!(check.to_string()) {
                return Ok(());
            }
            let response = self
                .request(&format!("{prefix}/check-runs"), Method::POST, Some(&check))
                .await?;
            let id = s(item, "id").to_owned();
            store
                .write(move |db| {
                    db.set(&format!("check:{id}"), &response["id"])?;
                    db.set(&key, &json!(check.to_string()))
                })
                .await?;
        }
        Ok(())
    }
}
