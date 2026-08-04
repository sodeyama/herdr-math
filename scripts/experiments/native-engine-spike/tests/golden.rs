use std::fs;

#[test]
fn golden_corpus_renders() {
    let run = native_engine_spike::golden::run_golden()
        .expect("The golden corpus pipeline should complete even when individual cases fail");

    assert!(
        run.cases.len() >= 28,
        "Only {} golden cases were attempted",
        run.cases.len()
    );

    let index_path = run.output_dir.join("index.json");
    assert!(index_path.exists(), "index.json was not written");
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(&index_path).expect("index.json should be readable"))
            .expect("index.json should parse");
    assert_eq!(
        index
            .get("cases")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(run.cases.len()),
        "index.json should contain every attempted case"
    );

    let failed_corpus = run
        .cases
        .iter()
        .filter(|case| case.kind == "corpus" && !case.native.is_ok())
        .map(|case| format!("{}: {}", case.id, case.native.status))
        .collect::<Vec<_>>();
    assert!(
        failed_corpus.is_empty(),
        "Native corpus failures:\n{}",
        failed_corpus.join("\n")
    );
}
