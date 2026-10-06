use crate::{Result, ensure, now, s, store::Db};
use serde_json::{Value, json};

pub fn create(db: &Db, actor: &str, body: &Value) -> Result<Value> {
    let kind = s(body, "type");
    ensure(
        ["bug", "feature"].contains(&kind),
        400,
        "Choose bug or feature.",
    )?;
    let title = s(body, "title").trim();
    let details = s(body, "details").trim();
    let name = if actor.is_empty() {
        s(body, "name").trim()
    } else {
        actor
    };
    ensure(
        !name.is_empty() && name.chars().count() <= 80,
        400,
        "Your display name is required (up to 80 characters).",
    )?;
    ensure(
        !title.is_empty() && title.chars().count() <= 200,
        400,
        "A title is required (up to 200 characters).",
    )?;
    ensure(
        details.chars().count() >= 10 && details.chars().count() <= 8000,
        400,
        "Describe the report in 10 to 8000 characters.",
    )?;
    let number = db.value("reportSequence")?.as_u64().unwrap_or(0) + 1;
    db.set("reportSequence", &json!(number))?;
    let at = now();
    let item = json!({"id":format!("report:{number}"),"number":number,"kind":"issue","local":true,"reportType":kind,"title":title,"body":details,"author":if actor.is_empty() {format!("guest: {name}")} else {actor.to_string()},"verifiedAuthor":!actor.is_empty(),"createdAt":at,"updatedAt":at,"revision":at,"url":null,"plan":"needs-triage","direction":"pending","risk":"medium","area":"untriaged","priority":"normal","platforms":[],"reviews":[],"validations":[],"votes":{},"checks":[]});
    db.put(&item)?;
    db.event(
        s(&item, "id"),
        name,
        "report-created",
        &json!({"verified":!actor.is_empty(),"type":kind}),
    )?;
    Ok(item)
}
