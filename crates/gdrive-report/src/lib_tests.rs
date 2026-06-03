use super::*;

#[test]
fn markdown_writer_handles_paths_without_parent_directories() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let previous_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(temp_dir.path()).expect("set cwd");

    MarkdownReportWriter.write_markdown("report.md", "# report\n").expect("write markdown");

    let contents = std::fs::read_to_string(temp_dir.path().join("report.md")).expect("report");
    assert!(contents.contains("# report"));

    std::env::set_current_dir(previous_dir).expect("restore cwd");
}
