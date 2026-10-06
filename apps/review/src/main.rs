use serde_json::{Value, json};
use std::{env, path::Path};
use zeron_review::{
    Error, Result,
    config::Config,
    domain,
    server::{App, router},
    store::{Db, Store},
};

fn import(config: &Config, args: &[String]) -> Result<()> {
    let items = if let Some(index) = args.iter().position(|a| a == "--file") {
        let path = args
            .get(index + 1)
            .ok_or_else(|| Error::new(400, "--file requires a path"))?;
        serde_json::from_slice::<Value>(&std::fs::read(path)?)?["items"]
            .as_array()
            .cloned()
            .ok_or_else(|| Error::new(400, "Snapshot requires an items array"))?
    } else if args.iter().any(|a| a == "--github") {
        let mut items = Vec::new();
        for (kind, command, fields) in [
            (
                "pr",
                "pr",
                "number,title,author,url,createdAt,updatedAt,labels,headRefOid,baseRefOid,isDraft,statusCheckRollup",
            ),
            (
                "issue",
                "issue",
                "number,title,author,url,createdAt,updatedAt,labels",
            ),
        ] {
            let output = std::process::Command::new("gh")
                .args([
                    command,
                    "list",
                    "--repo",
                    &config.policy.repository,
                    "--state",
                    "open",
                    "--limit",
                    "1000",
                    "--json",
                    fields,
                ])
                .output()?;
            if !output.status.success() {
                return Err(Error::new(
                    500,
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
            let rows: Vec<Value> = serde_json::from_slice(&output.stdout)?;
            for raw in rows {
                let revision = if kind == "pr" {
                    raw["headRefOid"].clone()
                } else {
                    raw["updatedAt"].clone()
                };
                let checks=raw["statusCheckRollup"].as_array().cloned().unwrap_or_default().into_iter().filter(|c| config.policy.required_checks.contains(&zeron_review::s(c,"name").to_owned())).map(|c| json!({"name":c["name"],"conclusion":zeron_review::s(&c,"conclusion").to_lowercase(),"revision":revision,"baseRevision":null})).collect::<Vec<_>>();
                items.push(json!({"id":format!("{kind}:{}",raw["number"]),"kind":kind,"number":raw["number"],"title":raw["title"],"author":raw["author"]["login"],"url":raw["url"],"createdAt":raw["createdAt"],"updatedAt":raw["updatedAt"],"syncedAt":zeron_review::now(),"revision":revision,"baseRevision":raw["baseRefOid"],"draft":raw["isDraft"].as_bool().unwrap_or(false),"labels":zeron_review::array(&raw,"labels").iter().map(|l| l["name"].clone()).collect::<Vec<_>>(),"sample":false,"checks":checks}));
            }
        }
        items
    } else {
        return Err(Error::new(
            400,
            "Use import --github or import --file snapshot.json; add --demo for the sandbox.",
        ));
    };
    let mut db = Db::open(&config.data_path)?;
    db.transaction(|db| {
        for item in &items {
            let id = zeron_review::s(item, "id");
            if !(id.starts_with("pr:") || id.starts_with("issue:"))
                || zeron_review::s(item, "revision").is_empty()
            {
                return Err(Error::new(400, "Invalid snapshot item"));
            }
            domain::ingest(db, item)?;
        }
        db.set("lastReconciled", &json!(zeron_review::now()))
    })?;
    println!(
        "Imported {} items into the {} database. No GitHub writes were made.",
        items.len(),
        if config.demo { "sandbox" } else { "live" }
    );
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = Config::load(args.iter().any(|a| a == "--demo"))?;
    if args.first().is_some_and(|a| a == "import") {
        return import(&config, &args);
    }
    let address = format!("{}:{}", config.host, config.port);
    let public_url = config.public_url.clone();
    let store = Store::open(Path::new(&config.data_path))?;
    let app = App::new(config, store).await?;
    let listener = tokio::net::TcpListener::bind(&address).await?;
    app.workers();
    println!(
        "Zeron review (Rust, {}) listening on {public_url}",
        if app.config.demo { "demo" } else { "live" }
    );
    axum::serve(listener, router(app))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
