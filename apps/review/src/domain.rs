use crate::{Error, Result, array, config::Policy, ensure, flag, now, s, store::Db};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Assessment {
    pub state: String,
    pub blockers: Vec<String>,
}
pub fn assess(item: &Value, policy: &Policy) -> Assessment {
    if s(item, "kind") == "issue" {
        return Assessment {
            state: if flag(item, "closed") {
                "closed"
            } else if s(item, "plan").is_empty() {
                "needs-triage"
            } else {
                s(item, "plan")
            }
            .into(),
            blockers: if !flag(item, "closed") && s(item, "plan") == "needs-triage" {
                vec!["Issue triage needed".into()]
            } else {
                vec![]
            },
        };
    }
    let mut blockers = Vec::new();
    if s(item, "direction") != "accepted" {
        blockers.push("Direction decision needed".into());
    }
    if !flag(item, "triaged") {
        blockers.push("Scope, risk, and platform triage needed".into());
    }
    if flag(item, "draft") {
        blockers.push("Author has marked this PR as draft".into());
    }
    let mut current: Vec<_> = array(item, "reviews")
        .into_iter()
        .filter(|r| s(r, "revision") == s(item, "revision"))
        .collect();
    current.sort_by_key(|record| {
        DateTime::parse_from_rfc3339(s(record, "at"))
            .map(|at| at.timestamp_millis())
            .unwrap_or(0)
    });
    let mut latest = BTreeMap::new();
    for review in &current {
        latest.insert(s(review, "actor"), review);
    }
    let trusted: Vec<_> = latest
        .values()
        .filter(|r| {
            s(r, "actor") != s(item, "author") && policy.roles(s(r, "actor")).contains(&"reviewer")
        })
        .collect();
    let changes = trusted.iter().any(|r| s(r, "verdict") == "changes");
    let approved = trusted.iter().any(|r| s(r, "verdict") == "pass");
    if changes {
        blockers.push("Reviewer requested changes".into());
    }
    if !approved {
        blockers.push("Independent code review needed".into());
    }
    if s(item, "risk") == "high" && s(item, "riskApproval") != s(item, "revision") {
        blockers.push("Maintainer risk approval needed".into());
    }
    for platform in array(item, "platforms") {
        let records = array(item, "validations");
        let mut latest = BTreeMap::new();
        for record in &records {
            if record["platform"] == platform
                && s(record, "revision") == s(item, "revision")
                && s(record, "actor") != s(item, "author")
                && policy.roles(s(record, "actor")).contains(&"validator")
            {
                latest.insert(s(record, "actor"), record);
            }
        }
        if latest.values().any(|v| s(v, "verdict") == "fail") {
            blockers.push(format!(
                "{} validation failed",
                platform.as_str().unwrap_or("")
            ));
        } else if !latest.values().any(|v| s(v, "verdict") == "pass") {
            blockers.push(format!(
                "{} validation needed",
                platform.as_str().unwrap_or("")
            ));
        }
    }
    let checks = array(item, "checks");
    for name in &policy.required_checks {
        let check = checks.iter().find(|c| s(c, "name") == name);
        if !check.is_some_and(|c| {
            s(c, "revision") == s(item, "revision")
                && !s(c, "baseRevision").is_empty()
                && s(c, "baseRevision") == s(item, "baseRevision")
                && s(c, "conclusion") == "success"
        }) {
            blockers.push(format!(
                "Required check: {name} ({})",
                check
                    .map(|c| s(c, "conclusion"))
                    .filter(|c| !c.is_empty())
                    .unwrap_or("missing")
            ));
        }
    }
    if !s(item, "blocker").is_empty() {
        blockers.push(s(item, "blocker").to_owned());
    }
    let state = if flag(item, "merged") {
        "merged"
    } else if flag(item, "closed") {
        "closed"
    } else if s(item, "direction") == "pending" {
        "needs-triage"
    } else if s(item, "direction") != "accepted" {
        "needs-direction"
    } else if changes {
        "changes-requested"
    } else if blockers.is_empty() {
        "ready-to-merge"
    } else if approved {
        "needs-validation"
    } else {
        "ready-for-review"
    };
    Assessment {
        state: state.into(),
        blockers,
    }
}
pub fn view(item: &Value, claims: Vec<Value>, policy: &Policy) -> Result<Value> {
    let mut result = item.clone();
    let assessment = assess(item, policy);
    result["state"] = assessment.state.into();
    result["blockers"] = json!(assessment.blockers);
    result["claims"] = json!(claims);
    let at = if s(item, "decisionAt").is_empty() {
        s(item, "createdAt")
    } else {
        s(item, "decisionAt")
    };
    let date =
        DateTime::parse_from_rfc3339(at).map_err(|_| Error::new(400, "Invalid item date."))?;
    result["decisionDue"] = (date + Duration::days(policy.decision_days))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
        .into();
    Ok(result)
}
pub fn context(db: &Db, item: &Value, policy: &Policy) -> Result<Value> {
    let mut result = view(item, db.claims(s(item, "id"))?, policy)?;
    result["events"] = json!(db.events(s(item, "id"))?);
    Ok(result)
}
fn text(body: &Value, key: &str, label: &str, max: usize) -> Result<String> {
    let value = s(body, key).trim();
    ensure(
        !value.is_empty() && value.chars().count() <= max,
        400,
        format!("{label} is required (maximum {max} characters)."),
    )?;
    Ok(value.to_owned())
}
fn choice(body: &Value, key: &str, options: &[&str]) -> Result<String> {
    ensure(
        options.contains(&s(body, key)),
        400,
        format!("Invalid {key}."),
    )?;
    Ok(s(body, key).into())
}
fn push(item: &mut Value, key: &str, record: Value) {
    if !item[key].is_array() {
        item[key] = json!([]);
    }
    item[key].as_array_mut().unwrap().push(record);
}
pub fn mutate(
    db: &Db,
    id: &str,
    actor: &str,
    action: &str,
    body: &Value,
    policy: &Policy,
) -> Result<Value> {
    let mut item = db
        .get(id)?
        .ok_or_else(|| Error::new(404, "Item not found."))?;
    ensure(!actor.is_empty(), 401, "Sign in to contribute.")?;
    ensure(
        s(body, "revision") == s(&item, "revision"),
        409,
        "This revision changed. Refresh before submitting.",
    )?;
    ensure(
        !flag(&item, "closed") && !flag(&item, "merged"),
        409,
        "This item is already resolved.",
    )?;
    let roles = policy.roles(actor);
    let allowed = |role| {
        ensure(
            roles.contains(&role),
            403,
            format!("{role} capability required."),
        )
    };
    let independent = || {
        ensure(
            actor != s(&item, "author"),
            403,
            "Authors cannot verify their own changes.",
        )
    };
    let at = now();
    let revision = s(&item, "revision").to_owned();
    match action {
        "direction" => {
            allowed("triager")?;
            let verdict = choice(
                body,
                "verdict",
                &["accepted", "deferred", "rejected", "needs-decision"],
            )?;
            if s(&item, "risk") != "low" || verdict == "needs-decision" {
                allowed("maintainer")?;
            }
            let reason = text(body, "reason", "Decision reason", 4000)?;
            item["direction"] = verdict.clone().into();
            item["directionReason"] = reason.clone().into();
            item["decisionAt"] = at.clone().into();
            item["blocker"] = if ["deferred", "rejected"].contains(&verdict.as_str()) {
                reason.into()
            } else {
                Value::Null
            };
        }
        "triage" => {
            allowed("triager")?;
            let risk = choice(body, "risk", &["low", "medium", "high"])?;
            let platforms = body["platforms"]
                .as_array()
                .ok_or_else(|| Error::new(400, "Invalid platforms."))?;
            ensure(
                platforms.iter().all(|p| {
                    ["linux", "macos", "windows", "ios"].contains(&p.as_str().unwrap_or(""))
                }),
                400,
                "Invalid platforms.",
            )?;
            let rank = |risk: &str| {
                ["low", "medium", "high"]
                    .iter()
                    .position(|v| *v == risk)
                    .unwrap_or(1)
            };
            if flag(&item, "triaged")
                && (rank(&risk) < rank(s(&item, "risk"))
                    || array(&item, "platforms")
                        .iter()
                        .any(|p| !platforms.contains(p)))
            {
                allowed("maintainer")?;
            }
            item["risk"] = risk.into();
            item["platforms"] = json!(platforms);
            item["priority"] = choice(body, "priority", &["urgent", "normal", "low"])?.into();
            item["area"] = text(body, "area", "Area", 80)?.into();
            item["triaged"] = true.into();
            if let Some(blocker) = body["blocker"].as_str() {
                let blocker = blocker.trim().chars().take(1000).collect::<String>();
                item["blocker"] = if blocker.is_empty() {
                    Value::Null
                } else {
                    blocker.into()
                };
            }
            if s(&item, "kind") == "issue" {
                item["plan"] =
                    choice(body, "plan", &["now", "next", "later", "needs-triage"])?.into();
            }
        }
        "claim" | "renew" | "release" => {
            let task = text(body, "task", "Task", 80)?;
            let mut tasks = vec!["code-review".to_owned()];
            tasks.extend(
                array(&item, "platforms")
                    .iter()
                    .map(|p| format!("validate:{}", p.as_str().unwrap_or(""))),
            );
            if s(&item, "kind") == "issue" {
                tasks = vec!["reproduce".into()];
            } else {
                independent()?;
            }
            ensure(
                tasks.contains(&task),
                400,
                "Task is not required for this item.",
            )?;
            let previous = db.claims(id)?.into_iter().find(|c| s(c, "task") == task);
            if action == "release" {
                ensure(
                    previous
                        .as_ref()
                        .is_some_and(|c| s(c, "actor") == actor || roles.contains(&"maintainer")),
                    403,
                    "Only the claimant or a maintainer can release this task.",
                )?;
                db.release(id, &task)?;
            } else {
                ensure(
                    action != "renew" || previous.as_ref().is_some_and(|c| s(c, "actor") == actor),
                    409,
                    "Claim expired or belongs to someone else.",
                )?;
                ensure(
                    previous.as_ref().is_none_or(|c| s(c, "actor") == actor),
                    409,
                    format!(
                        "Already claimed by {}.",
                        previous.as_ref().map(|c| s(c, "actor")).unwrap_or("")
                    ),
                )?;
                let hours = body["hours"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .unwrap_or(policy.claim_hours)
                    .clamp(0.25, policy.claim_hours.max(0.25))
                    .min(policy.claim_hours);
                let expires = (Utc::now() + Duration::milliseconds((hours * 3_600_000.0) as i64))
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
                db.claim(id, &task, actor, &revision, &expires)?;
            }
        }
        "review" => {
            ensure(s(&item, "kind") == "pr", 400, "Code reviews apply to PRs.")?;
            allowed("reviewer")?;
            independent()?;
            let record = json!({"actor":actor,"revision":revision,"verdict":choice(body,"verdict",&["pass","changes"])?,"summary":text(body,"summary","Review findings",4000)?,"at":at});
            push(&mut item, "reviews", record);
            if db
                .claims(id)?
                .iter()
                .any(|c| s(c, "task") == "code-review" && s(c, "actor") == actor)
            {
                db.release(id, "code-review")?;
            }
        }
        "validate" => {
            allowed("validator")?;
            independent()?;
            ensure(
                array(&item, "platforms").contains(&body["platform"]),
                400,
                "Platform is not in required coverage.",
            )?;
            let artifact = s(body, "artifact").trim();
            ensure(
                artifact.is_empty()
                    || reqwest::Url::parse(artifact).is_ok_and(|u| u.scheme() == "https"),
                400,
                "Artifact must be an HTTPS URL.",
            )?;
            let record = json!({"actor":actor,"revision":revision,"platform":s(body,"platform"),"verdict":choice(body,"verdict",&["pass","fail"])?,"environment":text(body,"environment","Test environment",300)?,"summary":text(body,"summary","Test evidence",4000)?,"artifact":artifact,"at":at});
            push(&mut item, "validations", record);
            let task = format!("validate:{}", s(body, "platform"));
            if db
                .claims(id)?
                .iter()
                .any(|c| s(c, "task") == task && s(c, "actor") == actor)
            {
                db.release(id, &task)?;
            }
        }
        "risk-approval" => {
            allowed("maintainer")?;
            independent()?;
            text(body, "reason", "Approval reason", 4000)?;
            item["riskApproval"] = revision.into();
        }
        "vote" => {
            let dimension = choice(body, "dimension", &["demand", "urgency"])?;
            item["votes"][format!("{actor}:{dimension}")] = json!({"actor":actor,"dimension":dimension,"reason":text(body,"reason","Use case",1000)?,"at":at});
        }
        _ => return Err(Error::new(404, "Unknown action.")),
    }
    item["updatedAt"] = at.into();
    db.put(&item)?;
    db.event(id, actor, action, body)?;
    db.enqueue(id)?;
    context(db, &item, policy)
}
pub fn ingest(db: &Db, source: &Value) -> Result<Value> {
    let id = s(source, "id");
    ensure(
        !id.is_empty() && source.is_object(),
        400,
        "Invalid source item.",
    )?;
    let previous = db.get(id)?;
    let mut item = json!({"direction":"pending","risk":"medium","area":"untriaged","priority":"normal","platforms":[],"reviews":[],"validations":[],"votes":{},"checks":[],"plan":"needs-triage"});
    if let Some(old) = &previous {
        for (key, value) in old
            .as_object()
            .ok_or_else(|| Error::internal("Invalid stored item"))?
        {
            item[key] = value.clone();
        }
    }
    for (key, value) in source.as_object().unwrap() {
        if key != "nativeReviews" {
            item[key] = value.clone();
        }
    }
    if let Some(native) = source["nativeReviews"].as_array() {
        let mut reviews = array(&previous.clone().unwrap_or(Value::Null), "reviews");
        reviews.retain(|r| s(r, "source") != "github-review");
        reviews.extend(native.iter().cloned());
        item["reviews"] = json!(reviews);
    }
    if let Some(old) = &previous
        && s(old, "revision") != s(&item, "revision")
    {
        db.clear_claims(id)?;
        item["riskApproval"] = Value::Null;
        db.event(
            id,
            "github",
            "revision-changed",
            &json!({"before":old["revision"],"after":item["revision"]}),
        )?;
    }
    let comparable = |value: &Value| {
        let mut value = value.clone();
        if let Some(map) = value.as_object_mut() {
            for key in ["syncedAt", "updatedAt", "githubUpdatedAt"] {
                map.remove(key);
            }
        }
        value
    };
    db.put(&item)?;
    if previous.as_ref().map(comparable) != Some(comparable(&item)) {
        db.enqueue(id)?;
    }
    Ok(item)
}
