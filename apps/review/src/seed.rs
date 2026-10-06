use crate::{Result, config::Policy, domain, store::Db};
use chrono::{Duration, SecondsFormat, Utc};
use serde_json::json;
pub const ACTORS: [&str; 5] = [
    "demo-maintainer",
    "demo-reviewer",
    "demo-validator",
    "demo-triager",
    "demo-author",
];
pub fn policy() -> Policy {
    Policy {
        repository: "zeronsh/zeron".into(),
        maintainers: vec![ACTORS[0].into()],
        reviewers: vec![ACTORS[1].into()],
        validators: vec![ACTORS[2].into()],
        triagers: vec![ACTORS[3].into()],
        required_checks: vec!["core-tests".into()],
        claim_hours: 24.0,
        decision_days: 30,
    }
}
pub fn seed(db: &Db) -> Result<()> {
    if !db.list()?.is_empty() {
        return Ok(());
    }
    let scenarios = [
        (
            "pr",
            "Keep browser annotations attached after navigation",
            "browser",
            "medium",
            vec!["macos", "windows"],
            "pending",
        ),
        (
            "pr",
            "Handle dropped files with spaces in their paths",
            "desktop",
            "low",
            vec!["macos"],
            "accepted",
        ),
        (
            "pr",
            "Restore session titles after reconnecting",
            "sync",
            "high",
            vec!["linux", "macos"],
            "accepted",
        ),
        (
            "pr",
            "Show provider limits in the account panel",
            "providers",
            "medium",
            vec!["macos"],
            "accepted",
        ),
        (
            "pr",
            "Fix missing controls on older Windows versions",
            "desktop",
            "low",
            vec!["windows"],
            "accepted",
        ),
        (
            "issue",
            "UI stops responding after switching Spaces",
            "desktop",
            "high",
            vec!["macos"],
            "pending",
        ),
        (
            "issue",
            "Add searchable history for child sessions",
            "sessions",
            "medium",
            vec![],
            "pending",
        ),
        (
            "pr",
            "Preserve scroll position while switching sessions",
            "desktop",
            "low",
            vec!["linux"],
            "accepted",
        ),
    ];
    for (index, (kind, title, area, risk, platforms, direction)) in
        scenarios.into_iter().enumerate()
    {
        let number = 90001 + index;
        let id = format!("{kind}:{number}");
        let revision = format!("{}{number:08}", "a".repeat(32));
        let at = (Utc::now() - Duration::days(index as i64 + 1))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let checks = if kind == "pr" && index != 0 {
            vec![
                json!({"name":"core-tests","conclusion":"success","revision":revision,"baseRevision":"b".repeat(40)}),
            ]
        } else {
            vec![]
        };
        let mut item = domain::ingest(
            db,
            &json!({"id":id,"number":number,"kind":kind,"title":title,"area":area,"risk":risk,"platforms":platforms,"direction":direction,
            "triaged":true,"priority":if index == 5 {"urgent"} else {"normal"},"author":"demo-author","sample":true,"revision":revision,"baseRevision":"b".repeat(40),
            "createdAt":at,"updatedAt":at,"syncedAt":at,"plan":if index == 5 {"now"} else if index == 6 {"next"} else {"needs-triage"},"checks":checks}),
        )?;
        if [3, 4, 7].contains(&index) {
            item["reviews"] = json!([{"actor":"demo-reviewer","revision":revision,"verdict":if index == 7 {"changes"} else {"pass"},
            "summary":if index == 7 {"Switching to an empty session still jumps to the top. Please cover that transition."} else {"Reviewed the state transitions and regression coverage. No blocking findings."},"at":at}]);
        }
        if index == 4 {
            item["validations"] = json!([{"actor":"demo-validator","revision":revision,"platform":"windows","verdict":"pass","environment":"Windows 10, x64",
            "summary":"Compared controls before and after the change. Checked maximized and restored windows at two display scales.","artifact":"","at":at}]);
        }
        db.put(&item)?;
        db.event(&id, "demo", "scenario-created", &json!({"sample":true}))?;
        if index == 3 {
            db.claim(
                &id,
                "validate:macos",
                "demo-validator",
                &revision,
                &(Utc::now() + Duration::hours(12)).to_rfc3339_opts(SecondsFormat::Millis, true),
            )?;
        }
    }
    Ok(())
}
