use std::fs;
use std::process::{Command, Output};

fn export(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zeron-theme-export"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap()
}

fn successful(args: &[&str]) {
    let result = export(args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn cli_checks_real_output_and_detects_source_and_output_drift_without_writing() {
    let fixture = tempfile::tempdir().unwrap();
    let output = fixture.path().join("generated");
    let output = output.to_str().unwrap();
    let source = fixture.path().join("browser.json");
    let original_source = include_str!("../../../web/packages/theme/src/browser-tokens.json");
    fs::write(&source, original_source).unwrap();
    let source = source.to_str().unwrap();
    let args = ["--output", output, "--browser-tokens", source];
    successful(&args);
    successful(&["--output", output, "--browser-tokens", source, "--check"]);
    let artifact_path = std::path::Path::new(output).join("artifact.json");
    let module_path = std::path::Path::new(output).join("index.ts");
    let original = fs::read(&artifact_path).unwrap();
    let module = fs::read(&module_path).unwrap();
    successful(&args);
    assert_eq!(original, fs::read(&artifact_path).unwrap());
    assert_eq!(module, fs::read(&module_path).unwrap());

    let mut tokens: serde_json::Value = serde_json::from_str(original_source).unwrap();
    tokens["layout"]["space"]["xs"] = serde_json::json!(99);
    fs::write(source, serde_json::to_string(&tokens).unwrap()).unwrap();
    let stale = export(&["--output", output, "--browser-tokens", source, "--check"]);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("is stale"));
    assert_eq!(
        original,
        fs::read(&artifact_path).unwrap(),
        "--check must not write"
    );
    successful(&args);
    assert_ne!(original, fs::read(&artifact_path).unwrap());
    successful(&["--output", output, "--browser-tokens", source, "--check"]);

    fs::write(&module_path, "stale").unwrap();
    assert!(
        !export(&["--output", output, "--browser-tokens", source, "--check"])
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&module_path).unwrap(), "stale");
}

#[test]
fn cli_rejects_missing_output_and_unknown_arguments() {
    let fixture = tempfile::tempdir().unwrap();
    assert!(
        !export(&["--output", fixture.path().to_str().unwrap(), "--check"])
            .status
            .success()
    );
    assert!(!export(&["--unknown"]).status.success());
}
