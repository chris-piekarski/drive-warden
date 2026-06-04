use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

pub fn temp_db_path(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().join("inventory.db")
}

#[allow(dead_code)]
pub fn mock_auth_path(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().join("mock-auth.json")
}

pub fn run_mock_command(temp_dir: &TempDir, args: &[&str]) -> Output {
    run_mock_command_with_config(temp_dir, Path::new("tests/config/mock.toml"), args)
}

#[allow(dead_code)]
pub fn run_mock_command_with_fixture(
    temp_dir: &TempDir,
    fixture_dir: &str,
    args: &[&str],
) -> Output {
    let config_path = write_mock_config(temp_dir, fixture_dir);
    run_mock_command_with_config(temp_dir, &config_path, args)
}

#[allow(dead_code)]
pub fn write_mock_config(temp_dir: &TempDir, fixture_dir: &str) -> PathBuf {
    let config_path =
        temp_dir.path().join(format!("{}.toml", fixture_dir.replace(['/', '\\'], "_")));
    let contents = format!("[backend]\nkind = \"mock\"\nfixture_dir = \"{fixture_dir}\"\n");
    fs::write(&config_path, contents).expect("write mock config");
    config_path
}

pub fn run_mock_command_with_config(
    temp_dir: &TempDir,
    config_path: &Path,
    args: &[&str],
) -> Output {
    let db_path = temp_db_path(temp_dir);
    let mut command = Command::new(env!("CARGO_BIN_EXE_drive-warden"));
    command.current_dir(workspace_root());
    command.args([
        "--backend",
        "mock",
        "--config",
        config_path.to_str().expect("config path"),
        "--db",
        db_path.to_str().expect("db path"),
    ]);
    command.args(args);
    command.output().expect("run drive-warden")
}

#[allow(dead_code)]
pub fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout utf8")
}

#[allow(dead_code)]
pub fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr utf8")
}
