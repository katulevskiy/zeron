//! Exercise the actual CLI, including deterministic generation and a stale fixture.
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn run(dir: &Path, check: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wiregen"));
    command.arg("--out-dir").arg(dir);
    if check {
        command.arg("--check");
    }
    command.output().unwrap()
}

fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn real_cli_is_deterministic_and_rejects_a_modified_supported_contract() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wiregen-tests");
    fs::create_dir_all(&root).unwrap();
    let temp = tempfile::tempdir_in(root).unwrap();
    let output = temp.path().join("generated");
    let generated = run(&output, false);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let first = snapshot(&output);
    assert!(run(&output, true).status.success());
    assert!(run(&output, false).status.success());
    assert_eq!(snapshot(&output), first);
    assert!(run(&output, true).status.success());

    let identity = output.join("EngineInfo.ts");
    let original = fs::read_to_string(&identity).unwrap();
    fs::write(
        &identity,
        original.replace("deviceId: string", "deviceId: number"),
    )
    .unwrap();
    let stale = run(&output, true);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("EngineInfo.ts: differs"));
    assert!(run(&output, false).status.success());
    assert_eq!(snapshot(&output), first);
    assert!(run(&output, true).status.success());

    fs::remove_file(identity).unwrap();
    let missing = run(&output, true);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("EngineInfo.ts: missing"));

    assert!(run(&output, false).status.success());
    let obsolete = output.join("RemovedWireType.ts");
    fs::write(&obsolete, original).unwrap();
    assert_eq!(run(&output, true).status.code(), Some(1));
    assert!(run(&output, false).status.success());
    assert!(!obsolete.exists());
    assert_eq!(snapshot(&output), first);

    let handwritten = output.join("Handwritten.ts");
    fs::write(&handwritten, "export type Handwritten = string;\n").unwrap();
    assert_eq!(run(&output, false).status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(handwritten).unwrap(),
        "export type Handwritten = string;\n"
    );
}
