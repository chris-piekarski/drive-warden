mod support;

use support::*;

// Exercises read-only command handlers in account mode, including the
// `--format json` print branches that table-mode tests don't reach.
#[test]
fn read_commands_and_json_branches_smoke() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"]
    )
    .status
    .success());
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["sync"]).status.success());

    let cases: &[&[&str]] = &[
        &["report", "storage"],
        &["report", "summary"],
        &["report", "duplicates"],
        &["report", "sharing"],
        &["find", "duplicates"],
        &["--format", "json", "find", "duplicates"],
        &["find", "large", "--min", "1"],
        &["--format", "json", "find", "large", "--min", "1"],
        &["find", "shared"],
        &["--format", "json", "find", "shared"],
        &["db", "stats"],
        &["--format", "json", "db", "stats"],
        &["inspect", "file", "photo-file"],
        &["--format", "json", "inspect", "file", "photo-file"],
        &["trash-status", "--within-days", "30"],
        &["--format", "json", "trash-status", "--within-days", "30"],
        &["trash-history"],
        &["--format", "json", "trash-history"],
        &["move-history"],
        &["--format", "json", "move-history"],
        &["auth", "status"],
        &["db", "remote", "status"],
        &["--format", "json", "db", "remote", "status"],
        &["unshare", "--shared-with", "anyone"],
        &["--format", "json", "unshare", "--shared-with", "anyone"],
        &["move", "--file-id", "public-file", "--to-root"],
        &["--format", "json", "move", "--file-id", "public-file", "--to-root"],
        &["unshare", "--shared-with", "anyone", "--dry-run"],
    ];
    let mut failures = Vec::new();
    for args in cases {
        let out = run_account_in(&tmp, "personal", args);
        if !out.status.success() {
            failures.push(format!("{args:?} -> stdout={} stderr={}", stdout(&out), stderr(&out)));
        }
    }
    assert!(failures.is_empty(), "failed commands:\n{}", failures.join("\n"));
}
