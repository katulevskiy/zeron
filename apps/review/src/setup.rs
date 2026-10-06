//! One-time owner-operated GitHub App registration; credentials never reach the frontend.
use crate::{
    Error, Result,
    config::{Config, hash, secret},
    ensure, s,
    server::App,
};
use axum::{
    body::Body,
    http::{HeaderMap, Uri},
    response::Response,
};
use chrono::Utc;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn page(body: String) -> Result<Response> {
    Response::builder().header("content-type","text/html; charset=utf-8")
        .header("content-security-policy","default-src 'self'; style-src 'self'; form-action https://github.com; frame-ancestors 'none'; base-uri 'none'")
        .header("referrer-policy","no-referrer").body(Body::from(format!("<!doctype html><html><head><meta name='viewport' content='width=device-width,initial-scale=1'><title>Contribution Manager setup</title><link rel='stylesheet' href='/style.css'></head><body><main class='setup'>{body}</main></body></html>"))).map_err(Error::internal)
}
fn save(config: &Config, value: &Value) -> Result<()> {
    if let Some(parent) = config.credentials_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = config.credentials_path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, &config.credentials_path)?;
    Ok(())
}
pub async fn handle(app: Arc<App>, uri: &Uri, headers: &HeaderMap) -> Result<Response> {
    ensure(
        !app.config.demo && !app.config.preview,
        404,
        "Setup is unavailable in the sandbox.",
    )?;
    let url = reqwest::Url::parse(&format!("{}{}", app.config.public_url, uri))
        .map_err(Error::internal)?;
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    if uri.path() == "/setup/github" {
        let provided = query.get("token").map(String::as_str).unwrap_or_default();
        use subtle::ConstantTimeEq;
        ensure(
            !app.config.setup_token.is_empty()
                && bool::from(provided.as_bytes().ct_eq(app.config.setup_token.as_bytes())),
            403,
            "Owner setup link required.",
        )?;
        ensure(
            app.config.app_id.is_empty(),
            409,
            "A GitHub App is already configured.",
        )?;
        let state = secret();
        let key = format!("setup:{}", hash(&state));
        app.store
            .write(move |db| {
                db.set(
                    &key,
                    &json!({"expires":Utc::now().timestamp_millis()+3600000}),
                )
            })
            .await?;
        let base = &app.config.public_url;
        let manifest = json!({"name":"Contribution Manager","url":base,"public":true,"hook_attributes":{"url":format!("{base}/webhooks/github"),"active":true},"redirect_url":format!("{base}/setup/callback"),"callback_urls":[format!("{base}/auth/callback")],"setup_url":format!("{base}/setup/installed"),"default_permissions":{"metadata":"read","contents":"read","pull_requests":"read","issues":"write","checks":"write"},"default_events":["pull_request","pull_request_review","issues","issue_comment","check_run","check_suite"],"request_oauth_on_install":false});
        let action = format!("https://github.com/settings/apps/new?state={state}");
        let mut response = page(format!(
            "<h1>Connect Contribution Manager</h1><p>Create the GitHub App, then install it on <strong>{}</strong> only. GitHub will ask you to confirm its permissions.</p><p>The App reads PRs and code, updates issue labels and workflow comments, and publishes readiness checks. Final merges stay on GitHub.</p><form method='post' action='{}'><input type='hidden' name='manifest' value='{}'><button class='primary' type='submit'>Create GitHub App</button></form>",
            escape(&app.config.policy.repository),
            escape(&action),
            escape(&manifest.to_string())
        ))?;
        response.headers_mut().insert(
            "set-cookie",
            format!(
                "contribution_setup={state}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600{}",
                if base.starts_with("https:") {
                    "; Secure"
                } else {
                    ""
                }
            )
            .parse()
            .unwrap(),
        );
        return Ok(response);
    }
    let cookie = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .filter_map(|v| v.trim().split_once('='))
        .find(|(k, _)| *k == "contribution_setup")
        .map(|(_, v)| v)
        .unwrap_or("");
    let key = format!("setup:{}", hash(cookie));
    let read_key = key.clone();
    let record = app.store.read(move |db| db.value(&read_key)).await?;
    ensure(
        !cookie.is_empty()
            && record["expires"].as_i64().unwrap_or(0) > Utc::now().timestamp_millis(),
        403,
        "Setup expired; reopen your owner setup link.",
    )?;
    if uri.path() == "/setup/callback" {
        ensure(
            query.get("state").is_some_and(|state| state == cookie),
            403,
            "Invalid setup state.",
        )?;
        let code = query
            .get("code")
            .ok_or_else(|| Error::new(400, "GitHub did not return a registration code."))?;
        ensure(
            !flag_completed(&record),
            409,
            "This registration was already completed.",
        )?;
        let converted = app
            .github
            .request_token(
                &format!("/app-manifests/{code}/conversions"),
                reqwest::Method::POST,
                None,
                "",
            )
            .await?;
        let owner = app.config.policy.repository.split('/').next().unwrap_or("");
        ensure(
            s(&converted["owner"], "login").eq_ignore_ascii_case(owner),
            403,
            "Register the App under the repository owner's account.",
        )?;
        let key_path = app.config.credentials_path.with_extension("pem");
        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&key_path, s(&converted, "pem"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        let settings = json!({"GITHUB_APP_ID":converted["id"].to_string(),"GITHUB_PRIVATE_KEY_PATH":key_path.to_string_lossy(),"GITHUB_CLIENT_ID":converted["client_id"],"GITHUB_CLIENT_SECRET":converted["client_secret"],"GITHUB_WEBHOOK_SECRET":converted["webhook_secret"],"slug":converted["slug"]});
        save(&app.config, &settings)?;
        app.store
            .write(move |db| {
                db.set(
                    &key,
                    &json!({"expires":Utc::now().timestamp_millis()+3600000,"completed":true}),
                )
            })
            .await?;
        return Response::builder()
            .status(302)
            .header(
                "location",
                format!(
                    "https://github.com/apps/{}/installations/new",
                    s(&converted, "slug")
                ),
            )
            .body(Body::empty())
            .map_err(Error::internal);
    }
    if uri.path() == "/setup/installed" {
        let id = query
            .get("installation_id")
            .filter(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| Error::new(400, "Missing installation identifier."))?;
        let mut settings: Value =
            serde_json::from_slice(&std::fs::read(&app.config.credentials_path)?)?;
        settings["GITHUB_INSTALLATION_ID"] = id.clone().into();
        let mut config = Config::load(false)?;
        config.installation_id = id.clone();
        let github = crate::github::GitHub::new(Arc::new(config))?;
        github
            .request(
                &format!("/repos/{}", app.config.policy.repository),
                reqwest::Method::GET,
                None,
            )
            .await?;
        save(&app.config, &settings)?;
        app.store.write(move |db| db.remove(&key)).await?;
        return page("<h1>GitHub connected</h1><p>Contribution Manager is restarting with the installation credentials. Return to the dashboard in a few seconds and sign in with GitHub.</p><a class='button primary' href='/'>Open Contribution Manager</a>".into());
    }
    Err(Error::new(404, "Setup route not found."))
}
fn flag_completed(record: &Value) -> bool {
    record["completed"].as_bool().unwrap_or(false)
}
