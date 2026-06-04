use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::{io, io::Write};

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use clap::{ArgAction, ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Generator};
use gdrive_core::{
    apply_move_orchestration, apply_trash, apply_unshare, apply_unshare_with_options, auth_status,
    duplicate_groups, inspect_exif, inspect_file_details, login, logout, move_orchestration_plan,
    sharing_findings, storage_summary, sync_inventory, trash_plan, unshare_plan,
    unshare_plan_with_options, DriveGateway, DriveScope, InventoryQuery, InventoryRepository,
    MoveDestinationTarget, MoveOptions, OwnerScope, RemoteDbEndpoint, RemoteDbManifest,
    RemoteDbSyncDecision, RemoteFileMetadata, ReportWriter, RetainCopyOptions, SharedWithFilter,
    TrashOptions, TrashedFileEntry, APP_NAME,
};
use gdrive_db::{DatabaseStats, RemoteSyncDirection, SqliteInventoryRepository, VacuumResult};
use gdrive_drive::{GoogleDriveGateway, MockDriveGateway};
use gdrive_report::{
    render_duplicates_report, render_sharing_report, render_storage_report, render_summary_report,
    MarkdownReportWriter,
};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;

const DEFAULT_REMOTE_DB_FOLDER_NAME: &str = "drive-warden-db";
const LEGACY_REMOTE_DB_FOLDER_NAME: &str = "gdrive-optimize-db";

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);
    run(cli).await
}

fn init_tracing(verbose: u8, quiet: bool) {
    let fallback = if quiet {
        "error"
    } else {
        match verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(fallback));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn run(cli: Cli) -> Result<()> {
    let runtime = AppRuntime::from_cli(&cli)?;

    match cli.command {
        Command::Completions(args) => {
            print_completions(args.shell)?;
            Ok(())
        }
        Command::Auth(command) => match command.command {
            AuthCommand::Login => {
                let gateway = runtime.build_gateway();
                let session = login(gateway.as_ref(), DriveScope::MetadataReadonly)
                    .await
                    .map_err(anyhow::Error::msg)?;
                let scopes = session
                    .active_scopes
                    .iter()
                    .map(|scope| scope.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("Logged in as {} with scopes [{}].", session.account.email, scopes);
                Ok(())
            }
            AuthCommand::Logout => {
                let gateway = runtime.build_gateway();
                let removed = logout(gateway.as_ref()).await.map_err(anyhow::Error::msg)?;
                if removed {
                    println!("Logged out.");
                } else {
                    println!("No active login session was found.");
                }
                Ok(())
            }
            AuthCommand::Status => {
                let gateway = runtime.build_gateway();
                let status = auth_status(gateway.as_ref()).await.map_err(anyhow::Error::msg)?;
                match status.session {
                    Some(session) => {
                        let scopes = session
                            .active_scopes
                            .iter()
                            .map(|scope| scope.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("Logged in as {} [{}]", session.account.email, scopes);
                    }
                    None => println!("Not logged in."),
                }
                Ok(())
            }
        },
        Command::Sync(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let gateway = runtime.build_gateway();
            let summary = sync_inventory(gateway.as_ref(), &repository, args.full)
                .await
                .map_err(|error| anyhow::Error::msg(decorate_sync_error(&error.to_string())))?;
            println!(
                "sync complete: mode={} added={} updated={} removed={} files={} paths={} token={}",
                summary.mode.as_str(),
                summary.added,
                summary.updated,
                summary.removed,
                summary.file_count,
                summary.path_count,
                summary.committed_page_token
            );
            Ok(())
        }
        Command::Report(command) => match command {
            ReportCommand::All(args) => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let writer = MarkdownReportWriter;
                let output_dir = resolve_report_dir(&runtime, args.output.as_deref());
                let query = InventoryQuery::default();
                let sync_state = repository.get_sync_state()?;
                let items = repository.load_inventory_items()?;
                let duplicates =
                    duplicate_groups(&repository, &query).map_err(anyhow::Error::msg)?;
                let sharing = sharing_findings(&repository, &query).map_err(anyhow::Error::msg)?;
                let storage = storage_summary(&repository, &query, runtime.stale_threshold_days)
                    .map_err(anyhow::Error::msg)?;

                let summary_path = output_dir.join("summary.md");
                let duplicates_path = output_dir.join("duplicates.md");
                let sharing_path = output_dir.join("sharing.md");
                let storage_path = output_dir.join("storage.md");

                writer.write_markdown(
                    summary_path.to_str().expect("summary path"),
                    &render_summary_report(
                        sync_state.as_ref(),
                        &items,
                        &duplicates,
                        &sharing,
                        &storage,
                    ),
                )?;
                writer.write_markdown(
                    duplicates_path.to_str().expect("duplicates path"),
                    &render_duplicates_report(sync_state.as_ref(), &duplicates),
                )?;
                writer.write_markdown(
                    sharing_path.to_str().expect("sharing path"),
                    &render_sharing_report(sync_state.as_ref(), &sharing),
                )?;
                writer.write_markdown(
                    storage_path.to_str().expect("storage path"),
                    &render_storage_report(sync_state.as_ref(), &storage),
                )?;

                println!(
                    "wrote reports:\n- {}\n- {}\n- {}\n- {}",
                    summary_path.display(),
                    duplicates_path.display(),
                    sharing_path.display(),
                    storage_path.display()
                );
                Ok(())
            }
            ReportCommand::Duplicates(args) => write_single_report(
                &runtime,
                args.output.as_deref(),
                "duplicates.md",
                |repository, sync_state| {
                    let query = InventoryQuery::default();
                    let duplicates = duplicate_groups(repository, &query)?;
                    Ok(render_duplicates_report(sync_state, &duplicates))
                },
            ),
            ReportCommand::Sharing(args) => write_single_report(
                &runtime,
                args.output.as_deref(),
                "sharing.md",
                |repository, sync_state| {
                    let query = InventoryQuery::default();
                    let sharing = sharing_findings(repository, &query)?;
                    Ok(render_sharing_report(sync_state, &sharing))
                },
            ),
            ReportCommand::Storage(args) => write_single_report(
                &runtime,
                args.output.as_deref(),
                "storage.md",
                |repository, sync_state| {
                    let query = InventoryQuery::default();
                    let storage =
                        storage_summary(repository, &query, runtime.stale_threshold_days)?;
                    Ok(render_storage_report(sync_state, &storage))
                },
            ),
            ReportCommand::Summary(args) => write_single_report(
                &runtime,
                args.output.as_deref(),
                "summary.md",
                |repository, sync_state| {
                    let query = InventoryQuery::default();
                    let items = repository.load_inventory_items()?;
                    let duplicates = duplicate_groups(repository, &query)?;
                    let sharing = sharing_findings(repository, &query)?;
                    let storage =
                        storage_summary(repository, &query, runtime.stale_threshold_days)?;
                    Ok(render_summary_report(sync_state, &items, &duplicates, &sharing, &storage))
                },
            ),
        },
        Command::Find(command) => match command {
            FindCommand::Duplicates(args) => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let query = build_inventory_query(&args.filters, None)?;
                let duplicates =
                    duplicate_groups(&repository, &query).map_err(anyhow::Error::msg)?;
                print_find_duplicates(cli.format, &duplicates)?;
                Ok(())
            }
            FindCommand::Shared(args) => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let mut query = build_inventory_query(&args.filters, None)?;
                query.shared_only = true;
                let findings = sharing_findings(&repository, &query).map_err(anyhow::Error::msg)?;
                print_find_shared(cli.format, &findings)?;
                Ok(())
            }
            FindCommand::Large(args) => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let query = build_inventory_query(&args.filters, args.min)?;
                let storage = storage_summary(&repository, &query, runtime.stale_threshold_days)
                    .map_err(anyhow::Error::msg)?;
                print_find_large(cli.format, &storage)?;
                Ok(())
            }
        },
        Command::Inspect(command) => match command {
            InspectCommand::File(args) => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let details =
                    inspect_file_details(&repository, &args.id).map_err(anyhow::Error::msg)?;
                let Some(details) = details else {
                    bail!("file `{}` was not found in the local snapshot", args.id);
                };
                print_inspect_file(cli.format, &details)?;
                Ok(())
            }
            InspectCommand::Exif(args) => {
                let gateway = runtime.build_gateway();
                let details =
                    inspect_exif(gateway.as_ref(), &args.id).await.map_err(anyhow::Error::msg)?;
                print_inspect_exif(cli.format, &details)?;
                Ok(())
            }
        },
        Command::Unshare(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let gateway = runtime.build_gateway();
            let query = build_inventory_query(&args.filters, None)?;
            let retain_copy = retain_copy_options_from_args(&args);
            let plan = if retain_copy.enabled {
                unshare_plan_with_options(&repository, &query, Some(&retain_copy))
                    .map_err(anyhow::Error::msg)?
            } else {
                unshare_plan(&repository, &query).map_err(anyhow::Error::msg)?
            };

            if args.dry_run && args.apply {
                bail!("`--dry-run` cannot be combined with `--apply`");
            }

            if args.dry_run || !args.apply {
                print_unshare_preview(cli.format, &plan)?;
                Ok(())
            } else {
                if !args.yes {
                    if cli.no_interactive {
                        bail!("`unshare --apply` requires `--yes` when `--no-interactive` is set");
                    }
                    confirm_unshare_apply(&plan)?;
                }

                let pre_mutation_release = if plan.actionable_count > 0 {
                    Some(create_pre_mutation_release(gateway.as_ref(), &runtime, "unshare").await?)
                } else {
                    None
                };
                let apply_summary = if retain_copy.enabled {
                    apply_unshare_with_options(
                        gateway.as_ref(),
                        &repository,
                        &query,
                        Some(&retain_copy),
                        "unshare",
                    )
                    .await
                    .map_err(anyhow::Error::msg)?
                } else {
                    apply_unshare(gateway.as_ref(), &repository, &query, "unshare")
                        .await
                        .map_err(anyhow::Error::msg)?
                };
                let sync_summary = sync_inventory(gateway.as_ref(), &repository, true)
                    .await
                    .map_err(anyhow::Error::msg)?;
                print_unshare_apply_summary(
                    cli.format,
                    &plan,
                    &apply_summary,
                    &sync_summary,
                    pre_mutation_release.as_ref(),
                )?;
                Ok(())
            }
        }
        Command::Trash(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let gateway = runtime.build_gateway();
            let query = build_inventory_query(&args.filters, None)?;
            let options = TrashOptions { recursive: args.recursive };
            let plan = trash_plan(&repository, &query, &options).map_err(anyhow::Error::msg)?;

            if args.dry_run && args.apply {
                bail!("`--dry-run` cannot be combined with `--apply`");
            }

            if args.dry_run || !args.apply {
                print_trash_preview(cli.format, &plan)?;
                Ok(())
            } else {
                if !args.yes {
                    if cli.no_interactive {
                        bail!("`trash --apply` requires `--yes` when `--no-interactive` is set");
                    }
                    confirm_trash_apply(&plan)?;
                }

                let pre_mutation_release = if plan.actionable_count > 0 {
                    Some(create_pre_mutation_release(gateway.as_ref(), &runtime, "trash").await?)
                } else {
                    None
                };
                let apply_summary =
                    apply_trash(gateway.as_ref(), &repository, &query, &options, "trash")
                        .await
                        .map_err(anyhow::Error::msg)?;
                let sync_summary = sync_inventory(gateway.as_ref(), &repository, true)
                    .await
                    .map_err(anyhow::Error::msg)?;
                print_trash_apply_summary(
                    cli.format,
                    &plan,
                    &apply_summary,
                    &sync_summary,
                    pre_mutation_release.as_ref(),
                )?;
                Ok(())
            }
        }
        Command::Move(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let gateway = runtime.build_gateway();
            let query = build_inventory_query(&args.filters, None)?;
            let target = move_destination_target_from_args(&args)?;
            let options = move_options_from_args(&args);
            let orchestration = move_orchestration_plan(&repository, &query, target, &options)
                .map_err(anyhow::Error::msg)?;
            let plan = orchestration.move_plan;

            if args.dry_run && args.apply {
                bail!("`--dry-run` cannot be combined with `--apply`");
            }

            if args.dry_run || !args.apply {
                print_move_preview(cli.format, &plan)?;
                Ok(())
            } else {
                if !args.yes {
                    if cli.no_interactive {
                        bail!("`move --apply` requires `--yes` when `--no-interactive` is set");
                    }
                    confirm_move_apply(&plan)?;
                }

                let will_mutate = plan.actionable_count > 0
                    || orchestration
                        .provisioning
                        .as_ref()
                        .is_some_and(|provisioning| provisioning.create_count > 0);
                let pre_mutation_release = if will_mutate {
                    Some(create_pre_mutation_release(gateway.as_ref(), &runtime, "move").await?)
                } else {
                    None
                };
                let apply_summary = apply_move_orchestration(
                    gateway.as_ref(),
                    &repository,
                    &query,
                    move_destination_target_from_args(&args)?,
                    &options,
                    "move",
                )
                .await
                .map_err(anyhow::Error::msg)?;
                let sync_summary = sync_inventory(gateway.as_ref(), &repository, true)
                    .await
                    .map_err(anyhow::Error::msg)?;
                print_move_apply_summary(
                    cli.format,
                    &plan,
                    &apply_summary,
                    &sync_summary,
                    pre_mutation_release.as_ref(),
                )?;
                Ok(())
            }
        }
        Command::MoveHistory(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let entries = load_filtered_move_history(&repository, args.limit, args.only_pending)?;
            print_move_history(cli.format, &entries)?;
            Ok(())
        }
        Command::TrashHistory(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let entries = load_filtered_trash_history(&repository, args.limit, args.only_pending)?;
            print_trash_history(cli.format, &entries)?;
            Ok(())
        }
        Command::TrashStatus(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let entries = repository.load_trashed_files().map_err(anyhow::Error::msg)?;
            print_trash_status(cli.format, &entries, args.within_days)?;
            Ok(())
        }
        Command::TrashRestore(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let entries = find_trash_restore_entries(&repository, &args)?;
            print_trash_restore_guidance(cli.format, &entries)?;
            Ok(())
        }
        Command::Doctor(args) => {
            let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
            let gateway = runtime.build_gateway();
            let status =
                build_operator_status(gateway.as_ref(), &runtime, &repository, args.within_days)
                    .await?;
            print_operator_status(cli.format, &status)?;
            Ok(())
        }
        Command::Db(command) => match command {
            DbCommand::Stats => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let stats = repository.stats()?;
                print_db_stats(cli.format, &stats)?;
                Ok(())
            }
            DbCommand::Vacuum => {
                let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
                let result = repository.vacuum()?;
                print_db_vacuum(cli.format, &result)?;
                Ok(())
            }
            DbCommand::Remote(remote_command) => {
                let gateway = runtime.build_gateway();
                handle_remote_db_command(
                    gateway.as_ref(),
                    &runtime,
                    remote_command,
                    cli.format,
                    cli.no_interactive,
                )
                .await
            }
        },
    }
}

#[derive(Debug, Parser)]
#[command(
    name = APP_NAME,
    version,
    about = "Organize, audit, and clean up Google Drive without the web UI.",
    long_about = "A local-first Rust CLI for syncing Google Drive metadata into SQLite, auditing duplicates and sharing exposure, and safely previewing remediation commands.",
    after_help = "Examples:\n  drive-warden auth login\n  drive-warden sync --full\n  drive-warden report all -o reports/\n  drive-warden find duplicates --limit 25\n  drive-warden unshare --shared-with anyone --dry-run"
)]
struct Cli {
    #[arg(long, global = true, default_value = "data/config.toml")]
    config: String,

    #[arg(long, global = true)]
    db: Option<String>,

    #[arg(long, global = true, value_enum)]
    backend: Option<BackendKind>,

    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    verbose: u8,

    #[arg(short = 'q', long, global = true, action = ArgAction::SetTrue)]
    quiet: bool,

    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,

    #[arg(long = "no-interactive", global = true, action = ArgAction::SetTrue)]
    no_interactive: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackendKind {
    Google,
    Mock,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Generate shell completions",
        after_help = "Examples:\n  drive-warden completions bash\n  drive-warden completions zsh\n  drive-warden completions fish"
    )]
    Completions(CompletionsArgs),
    #[command(
        about = "Authenticate with Google Drive",
        after_help = "Examples:\n  drive-warden auth login\n  drive-warden auth status\n  drive-warden auth logout"
    )]
    Auth(AuthArgs),
    #[command(
        about = "Sync inventory into the local SQLite cache",
        after_help = "Examples:\n  drive-warden sync\n  drive-warden sync --full"
    )]
    Sync(SyncArgs),
    #[command(subcommand)]
    #[command(
        about = "Generate Markdown reports",
        after_help = "Examples:\n  drive-warden report all -o reports/\n  drive-warden report sharing"
    )]
    Report(ReportCommand),
    #[command(subcommand)]
    #[command(
        about = "Find items in the local inventory",
        after_help = "Examples:\n  drive-warden find duplicates --limit 20\n  drive-warden find shared --shared-with anyone"
    )]
    Find(FindCommand),
    #[command(subcommand)]
    #[command(
        about = "Inspect a single file or its EXIF metadata",
        after_help = "Examples:\n  drive-warden inspect file <id>\n  drive-warden inspect exif <id>"
    )]
    Inspect(InspectCommand),
    #[command(
        about = "Preview or apply permission removals",
        after_help = "Safety: write commands default to dry-run.\nExamples:\n  drive-warden unshare --shared-with anyone --dry-run\n  drive-warden unshare --shared-with anyone --apply --yes"
    )]
    Unshare(UnshareArgs),
    #[command(
        about = "Preview or apply moves to Google Drive trash",
        after_help = "Safety: trash commands default to dry-run and never permanently delete files.\nExamples:\n  drive-warden trash --path '[orphan]/Coors/Model/*'\n  drive-warden trash --path '[orphan]/Coors/Model/*' --recursive --apply --yes"
    )]
    Trash(TrashArgs),
    #[command(
        about = "Preview or apply parent changes into folders",
        after_help = "Safety: move commands default to dry-run. Use --to-root for My Drive root, --provision-missing to create destination paths during apply, and move-history to audit completed moves.\nExamples:\n  drive-warden move --path '[orphan]/eBooks/*' --to-path '/Archive/eBooks'\n  drive-warden move --file-id <id> --to-root --apply --yes\n  drive-warden move --path '/Docs/*' --to-path '/Archive/New' --provision-missing --apply --yes"
    )]
    Move(MoveArgs),
    #[command(
        about = "Show append-only move history",
        after_help = "Examples:\n  drive-warden move-history\n  drive-warden move-history --only-pending --limit 100"
    )]
    MoveHistory(MoveHistoryArgs),
    #[command(
        about = "Show append-only trash history",
        after_help = "Examples:\n  drive-warden trash-history\n  drive-warden trash-history --only-pending --limit 100"
    )]
    TrashHistory(TrashHistoryArgs),
    #[command(
        about = "Summarize trash recovery deadlines",
        after_help = "Examples:\n  drive-warden trash-status\n  drive-warden trash-status --within-days 7"
    )]
    TrashStatus(TrashStatusArgs),
    #[command(
        about = "Print manual restore guidance for a trashed file",
        after_help = "This command is read-only. It does not restore files.\nExamples:\n  drive-warden trash-restore --file-id <drive-file-id>\n  drive-warden trash-restore --path-contains '[orphan]/Coors/Model'"
    )]
    TrashRestore(TrashRestoreArgs),
    #[command(
        about = "Run a read-only operator health check",
        after_help = "Examples:\n  drive-warden doctor\n  drive-warden doctor --within-days 7"
    )]
    Doctor(DoctorArgs),
    #[command(subcommand)]
    #[command(
        about = "Inspect or maintain the local database",
        after_help = "Examples:\n  drive-warden db stats\n  drive-warden db vacuum"
    )]
    Db(DbCommand),
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Args)]
struct CompletionsArgs {
    #[arg(value_enum)]
    shell: CompletionShell,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    Login,
    Logout,
    Status,
}

#[derive(Debug, Args)]
struct SyncArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    full: bool,
}

#[derive(Debug, Subcommand)]
enum ReportCommand {
    All(ReportArgs),
    Duplicates(ReportArgs),
    Sharing(ReportArgs),
    Storage(ReportArgs),
    Summary(ReportArgs),
}

#[derive(Debug, Args)]
struct ReportArgs {
    #[arg(short = 'o', long)]
    output: Option<String>,
}

#[derive(Debug, Subcommand)]
enum FindCommand {
    Duplicates(FindArgs),
    Shared(FindArgs),
    Large(LargeFindArgs),
}

#[derive(Debug, Args, Default)]
struct QueryFilters {
    #[arg(long = "file-id")]
    file_id: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    mime: Option<String>,
    #[arg(long)]
    older_than: Option<String>,
    #[arg(long)]
    larger_than: Option<u64>,
    #[arg(long = "in")]
    in_folder: Option<String>,
    #[arg(long)]
    path: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    shared: bool,
    #[arg(long)]
    shared_with: Option<String>,
    #[arg(long)]
    owner_scope: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    actionable_only: bool,
    #[arg(long)]
    duplicate_of: Option<String>,
    #[arg(long)]
    limit: Option<usize>,
    #[arg(long)]
    offset: Option<usize>,
}

#[derive(Debug, Args, Default)]
struct FindArgs {
    #[command(flatten)]
    filters: QueryFilters,
}

#[derive(Debug, Args, Default)]
struct LargeFindArgs {
    #[arg(long = "min")]
    min: Option<u64>,
    #[command(flatten)]
    filters: QueryFilters,
}

#[derive(Debug, Subcommand)]
enum InspectCommand {
    File(InspectFileArgs),
    Exif(InspectExifArgs),
}

#[derive(Debug, Args)]
struct InspectFileArgs {
    id: String,
}

#[derive(Debug, Args)]
struct InspectExifArgs {
    id: String,
}

#[derive(Debug, Args, Default)]
struct UnshareArgs {
    #[command(flatten)]
    filters: QueryFilters,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    apply: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    retain_copy: bool,
    #[arg(long)]
    backup_root_id: Option<String>,
}

#[derive(Debug, Args, Default)]
struct TrashArgs {
    #[command(flatten)]
    filters: QueryFilters,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    apply: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    recursive: bool,
}

#[derive(Debug, Args, Default)]
#[command(group(
    ArgGroup::new("destination")
        .required(true)
        .args(["to_folder_id", "to_path", "to_root"])
))]
struct MoveArgs {
    #[command(flatten)]
    filters: QueryFilters,
    #[arg(long = "to-folder-id")]
    to_folder_id: Option<String>,
    #[arg(long = "to-path")]
    to_path: Option<String>,
    #[arg(long = "to-root", action = ArgAction::SetTrue)]
    to_root: bool,
    #[arg(long = "provision-missing", action = ArgAction::SetTrue)]
    provision_missing: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    apply: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
}

#[derive(Debug, Args)]
struct MoveHistoryArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, action = ArgAction::SetTrue)]
    only_pending: bool,
}

#[derive(Debug, Args)]
struct TrashHistoryArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, action = ArgAction::SetTrue)]
    only_pending: bool,
}

#[derive(Debug, Args)]
struct TrashStatusArgs {
    #[arg(long, default_value_t = 7)]
    within_days: i64,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("selector")
        .required(true)
        .args(["file_id", "path_contains"])
))]
struct TrashRestoreArgs {
    #[arg(long)]
    file_id: Option<String>,
    #[arg(long)]
    path_contains: Option<String>,
    #[arg(long, default_value_t = 10)]
    limit: usize,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long, default_value_t = 7)]
    within_days: i64,
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    Stats,
    Vacuum,
    #[command(subcommand)]
    #[command(
        about = "Push or pull the local SQLite database to a private Google Drive folder",
        after_help = "Examples:\n  drive-warden db remote status\n  drive-warden db remote sync\n  drive-warden db remote push --yes\n  drive-warden db remote pull --yes"
    )]
    Remote(RemoteDbCommand),
}

#[derive(Debug, Subcommand)]
enum RemoteDbCommand {
    Status,
    Sync,
    Push(RemoteDbWriteArgs),
    Pull(RemoteDbWriteArgs),
    #[command(
        about = "Rename the remote DB folder in place",
        after_help = "This is intended for one-time product rename migrations. It preserves the folder ID and all release files.\nExamples:\n  drive-warden db remote rename-folder --yes\n  drive-warden db remote rename-folder --from gdrive-optimize-db --to drive-warden-db --yes"
    )]
    RenameFolder(RemoteDbRenameFolderArgs),
    #[command(
        about = "Create or list named remote DB releases",
        after_help = "Examples:\n  drive-warden db remote release --name coors-trash-v1 --yes\n  drive-warden db remote release list"
    )]
    Release(RemoteDbReleaseArgs),
}

#[derive(Debug, Args, Default)]
struct RemoteDbWriteArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
}

#[derive(Debug, Args)]
struct RemoteDbRenameFolderArgs {
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    to: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
}

#[derive(Debug, Args)]
struct RemoteDbReleaseArgs {
    #[arg(long)]
    name: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
    #[command(subcommand)]
    command: Option<RemoteDbReleaseCommand>,
}

#[derive(Debug, Subcommand)]
enum RemoteDbReleaseCommand {
    List(RemoteDbReleaseListArgs),
}

#[derive(Debug, Args)]
struct RemoteDbReleaseListArgs {
    #[arg(long)]
    prefix: Option<String>,
}

#[derive(Debug)]
struct AppRuntime {
    backend_kind: BackendKind,
    db_path: PathBuf,
    fixture_dir: PathBuf,
    google_credentials_path: PathBuf,
    google_token_path: PathBuf,
    google_session_path: PathBuf,
    mock_state_path: PathBuf,
    reports_output_dir: PathBuf,
    stale_threshold_days: i64,
    remote_db: RemoteDbConfig,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    backend: FileBackendConfig,
    #[serde(default)]
    google: FileGoogleConfig,
    #[serde(default)]
    database: FileDatabaseConfig,
    #[serde(default)]
    reports: FileReportsConfig,
}

#[derive(Debug, Default, Deserialize)]
struct FileBackendConfig {
    kind: Option<String>,
    fixture_dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileGoogleConfig {
    credentials_path: Option<String>,
    token_path: Option<String>,
    session_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileDatabaseConfig {
    path: Option<String>,
    remote_folder_id: Option<String>,
    remote_folder_name: Option<String>,
    remote_db_name: Option<String>,
    remote_manifest_name: Option<String>,
}

#[derive(Debug, Clone)]
struct RemoteDbConfig {
    folder_id: Option<String>,
    folder_name: String,
    legacy_folder_names: Vec<String>,
    db_name: String,
    manifest_name: String,
}

#[derive(Debug, Default, Deserialize)]
struct FileReportsConfig {
    output_dir: Option<String>,
    stale_threshold_days: Option<i64>,
}

impl AppRuntime {
    fn from_cli(cli: &Cli) -> Result<Self> {
        let config = load_file_config(Path::new(&cli.config))?;
        let backend_kind = cli
            .backend
            .or_else(|| config.backend.kind.as_deref().and_then(BackendKind::from_config))
            .unwrap_or(BackendKind::Google);
        let db_path = cli
            .db
            .clone()
            .or(config.database.path)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/inventory.db"));
        let runtime_dir =
            db_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("data"));
        let fixture_dir = config
            .backend
            .fixture_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tests/fixtures/drive_small"));
        let reports_output_dir = config
            .reports
            .output_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("reports"));
        let google_credentials_path =
            env_var_os_any(&["DRIVE_WARDEN_CREDENTIALS", "GDRIVE_OPTIMIZE_CREDENTIALS"])
                .map(PathBuf::from)
                .or_else(|| config.google.credentials_path.map(PathBuf::from))
                .unwrap_or_else(|| runtime_dir.join("credentials.json"));
        let google_token_path = env_var_os_any(&["DRIVE_WARDEN_TOKENS", "GDRIVE_OPTIMIZE_TOKENS"])
            .map(PathBuf::from)
            .or_else(|| config.google.token_path.map(PathBuf::from))
            .unwrap_or_else(|| runtime_dir.join("google-tokens.json"));
        let google_session_path =
            env_var_os_any(&["DRIVE_WARDEN_GOOGLE_SESSION", "GDRIVE_OPTIMIZE_GOOGLE_SESSION"])
                .map(PathBuf::from)
                .or_else(|| config.google.session_path.map(PathBuf::from))
                .unwrap_or_else(|| runtime_dir.join("google-session.json"));

        let (remote_folder_name, legacy_folder_names) = match config.database.remote_folder_name {
            Some(name) => (name, Vec::new()),
            None => {
                (DEFAULT_REMOTE_DB_FOLDER_NAME.into(), vec![LEGACY_REMOTE_DB_FOLDER_NAME.into()])
            }
        };
        let remote_db = RemoteDbConfig {
            folder_id: config.database.remote_folder_id,
            folder_name: remote_folder_name,
            legacy_folder_names,
            db_name: config.database.remote_db_name.unwrap_or_else(|| "inventory.db".into()),
            manifest_name: config
                .database
                .remote_manifest_name
                .unwrap_or_else(|| "inventory.db.manifest.json".into()),
        };

        Ok(Self {
            backend_kind,
            db_path,
            fixture_dir,
            google_credentials_path,
            google_token_path,
            google_session_path,
            mock_state_path: runtime_dir.join("mock-auth.json"),
            reports_output_dir,
            stale_threshold_days: config.reports.stale_threshold_days.unwrap_or(730),
            remote_db,
        })
    }

    fn build_gateway(&self) -> Box<dyn DriveGateway> {
        match self.backend_kind {
            BackendKind::Google => Box::new(GoogleDriveGateway::with_paths(
                &self.google_credentials_path,
                &self.google_token_path,
                &self.google_session_path,
            )),
            BackendKind::Mock => {
                Box::new(MockDriveGateway::new(&self.fixture_dir, &self.mock_state_path))
            }
        }
    }
}

fn env_var_os_any(names: &[&str]) -> Option<std::ffi::OsString> {
    names.iter().find_map(std::env::var_os)
}

fn load_file_config(path: &Path) -> Result<FileConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(toml::from_str(&contents)
            .with_context(|| format!("failed to parse config file at `{}`", path.display()))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(error) => Err(error).with_context(|| format!("failed to read `{}`", path.display())),
    }
}

impl BackendKind {
    fn from_config(raw: &str) -> Option<Self> {
        match raw {
            "google" => Some(Self::Google),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }
}

fn build_inventory_query(
    filters: &QueryFilters,
    min_override: Option<u64>,
) -> Result<InventoryQuery> {
    let larger_than = match (filters.larger_than, min_override) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    };
    let older_than_days = filters.older_than.as_deref().map(parse_older_than_days).transpose()?;
    let shared_with = filters.shared_with.as_deref().map(parse_shared_with_filter).transpose()?;
    let owner_scope = match filters.owner_scope.as_deref() {
        Some("mine") => OwnerScope::Mine,
        Some("all") | None => OwnerScope::All,
        Some(other) => bail!("unsupported owner scope `{other}`"),
    };

    Ok(InventoryQuery {
        file_id: filters.file_id.clone(),
        name_contains: filters.name.clone(),
        mime_contains: filters.mime.clone(),
        older_than_days,
        larger_than,
        in_folder: filters.in_folder.clone(),
        path_glob: filters.path.clone(),
        shared_only: filters.shared,
        shared_with,
        owner_scope,
        actionable_only: filters.actionable_only,
        duplicate_of: filters.duplicate_of.clone(),
        limit: filters.limit,
        offset: filters.offset.unwrap_or(0),
    })
}

fn retain_copy_options_from_args(args: &UnshareArgs) -> RetainCopyOptions {
    RetainCopyOptions { enabled: args.retain_copy, backup_root_id: args.backup_root_id.clone() }
}

fn move_destination_target_from_args(args: &MoveArgs) -> Result<MoveDestinationTarget> {
    if args.to_root {
        return Ok(MoveDestinationTarget::Root);
    }
    if let Some(folder_id) = &args.to_folder_id {
        return Ok(MoveDestinationTarget::FolderId(folder_id.clone()));
    }
    let path = args.to_path.as_ref().context("move destination is required")?.clone();
    Ok(MoveDestinationTarget::Path(path))
}

fn move_options_from_args(args: &MoveArgs) -> MoveOptions {
    MoveOptions { provision_missing: args.provision_missing }
}

fn parse_older_than_days(raw: &str) -> Result<i64> {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_suffix('d').unwrap_or(trimmed);
    stripped.parse::<i64>().with_context(|| format!("invalid --older-than value `{raw}`"))
}

fn parse_shared_with_filter(raw: &str) -> Result<SharedWithFilter> {
    if raw == "anyone" {
        return Ok(SharedWithFilter::Anyone);
    }
    if let Some(domain) = raw.strip_prefix("domain:") {
        return Ok(SharedWithFilter::Domain(domain.to_string()));
    }
    if let Some(email) = raw.strip_prefix("email:") {
        return Ok(SharedWithFilter::Email(email.to_string()));
    }
    bail!("unsupported --shared-with value `{raw}`")
}

fn resolve_report_dir(runtime: &AppRuntime, output: Option<&str>) -> PathBuf {
    output.map(PathBuf::from).unwrap_or_else(|| {
        runtime.reports_output_dir.join(Utc::now().format("%Y-%m-%d").to_string())
    })
}

fn write_single_report<F>(
    runtime: &AppRuntime,
    output: Option<&str>,
    file_name: &str,
    render: F,
) -> Result<()>
where
    F: FnOnce(
        &SqliteInventoryRepository,
        Option<&gdrive_core::SyncState>,
    ) -> gdrive_core::CoreResult<String>,
{
    let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
    let sync_state = repository.get_sync_state()?;
    let contents = render(&repository, sync_state.as_ref()).map_err(anyhow::Error::msg)?;
    let path = resolve_report_dir(runtime, output).join(file_name);
    let writer = MarkdownReportWriter;
    writer.write_markdown(path.to_str().expect("report path"), &contents)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn print_find_duplicates(
    format: OutputFormat,
    groups: &[gdrive_core::DuplicateGroup],
) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(groups)?),
        OutputFormat::Table => {
            for group in groups {
                println!("group {} ({})", group.group_key, group.match_type.as_str());
                for item in &group.items {
                    println!("  {}  {}  {}", item.file.id, item.file.name, item.path.primary_path);
                }
            }
        }
    }
    Ok(())
}

fn print_find_shared(format: OutputFormat, findings: &[gdrive_core::SharingFinding]) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(findings)?),
        OutputFormat::Table => {
            for finding in findings {
                println!(
                    "{}  {}  {}  {}  actionable={}",
                    finding.item.file.id,
                    finding.kind.as_str(),
                    finding.target_label,
                    finding.item.path.primary_path,
                    if finding.actionable { "yes" } else { "no" }
                );
            }
        }
    }
    Ok(())
}

fn print_find_large(format: OutputFormat, storage: &gdrive_core::StorageSummary) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(storage)?),
        OutputFormat::Table => {
            for finding in &storage.large_files {
                println!(
                    "{}  {} bytes  {}",
                    finding.item.file.id, finding.size_bytes, finding.item.path.primary_path
                );
            }
        }
    }
    Ok(())
}

fn print_inspect_file(
    format: OutputFormat,
    details: &gdrive_core::InspectFileDetails,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(details)?),
        OutputFormat::Table => {
            println!("id: {}", details.item.file.id);
            println!("name: {}", details.item.file.name);
            println!("path: {}", details.item.path.primary_path);
            println!("mime: {}", details.item.file.mime_type);
            println!("owned_by_me: {}", details.item.file.owned_by_me);
            println!("shared: {}", details.item.file.shared);
            println!("size: {}", details.item.file.size.unwrap_or(0));
            println!("duplicate_groups: {}", details.duplicate_groups.len());
            println!("sharing_findings: {}", details.sharing_findings.len());
            if !details.sharing_findings.is_empty() {
                println!("permissions:");
                for finding in &details.sharing_findings {
                    println!(
                        "  - {} {} actionable={}",
                        finding.kind.as_str(),
                        finding.target_label,
                        if finding.actionable { "yes" } else { "no" }
                    );
                }
            }
        }
    }
    Ok(())
}

fn print_inspect_exif(
    format: OutputFormat,
    details: &gdrive_core::InspectExifDetails,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(details)?),
        OutputFormat::Table => {
            println!("id: {}", details.file_id);
            println!("name: {}", details.name);
            println!("mime: {}", details.mime_type);
            println!("source: {}", details.source.as_str());
            if let Some(link) = &details.web_view_link {
                println!("web_view_link: {link}");
            }
            if let Some(width) = details.metadata.width {
                println!("width: {width}");
            }
            if let Some(height) = details.metadata.height {
                println!("height: {height}");
            }
            if let Some(make) = &details.metadata.camera_make {
                println!("camera_make: {make}");
            }
            if let Some(model) = &details.metadata.camera_model {
                println!("camera_model: {model}");
            }
            if let Some(date_taken) = details.metadata.date_taken {
                println!("date_taken: {}", date_taken.to_rfc3339());
            }
            if let Some(exposure_time) = &details.metadata.exposure_time {
                println!("exposure_time: {exposure_time}");
            }
            if let Some(aperture) = &details.metadata.aperture {
                println!("aperture: {aperture}");
            }
            if let Some(focal_length) = &details.metadata.focal_length {
                println!("focal_length: {focal_length}");
            }
            if let Some(iso_speed) = details.metadata.iso_speed {
                println!("iso_speed: {iso_speed}");
            }
        }
    }
    Ok(())
}

fn print_unshare_preview(format: OutputFormat, plan: &gdrive_core::UnsharePlan) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(plan)?),
        OutputFormat::Table => {
            println!(
                "unshare preview: rows={} actionable={} skipped={} public={} domain={} direct={}",
                plan.rows.len(),
                plan.actionable_count,
                plan.skipped_count,
                plan.public_count,
                plan.domain_count,
                plan.direct_count
            );
            if let Some(retain_copy) = &plan.retain_copy {
                println!(
                    "retain-copy preview: roots={} files={} folders={} destination={}",
                    retain_copy.root_count,
                    retain_copy.total_file_copies,
                    retain_copy.total_folder_copies,
                    retain_copy.destination_label
                );
                for entry in &retain_copy.entries {
                    println!(
                        "backup {}  {}  files={} folders={} owned_by_me={} path_state={}",
                        entry.source_item.file.id,
                        entry.source_item.path.primary_path,
                        entry.descendant_file_count,
                        entry.descendant_folder_count,
                        entry.source_item.file.owned_by_me,
                        entry.source_item.path.path_state.as_str()
                    );
                }
            }
            for row in &plan.rows {
                println!(
                    "{}  {}  {}  reason={} actionable={}",
                    row.item.file.id,
                    row.target_label,
                    row.item.path.primary_path,
                    row.reason.as_str(),
                    if row.actionable { "yes" } else { "no" }
                );
            }
        }
    }
    Ok(())
}

fn print_unshare_apply_summary(
    format: OutputFormat,
    plan: &gdrive_core::UnsharePlan,
    apply_summary: &gdrive_core::UnshareApplySummary,
    sync_summary: &gdrive_core::SyncSummary,
    pre_mutation_release: Option<&RemoteDbRelease>,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan,
                "apply_summary": apply_summary,
                "sync_summary": sync_summary,
                "pre_mutation_release": pre_mutation_release,
            }))?
        ),
        OutputFormat::Table => {
            if let Some(release) = pre_mutation_release {
                println!(
                    "pre-mutation release: name={} db_file={} manifest_file={}",
                    release.name, release.db_file.id, release.manifest_file.id
                );
            }
            if let Some(retain_copy) = &apply_summary.retain_copy {
                println!(
                    "retained copy: roots={} copied_files={} created_folders={} destination={} ({})",
                    retain_copy.root_count,
                    retain_copy.copied_files,
                    retain_copy.created_folders,
                    retain_copy.destination_folder_name,
                    retain_copy.destination_folder_id
                );
            }
            println!(
                "unshare applied: planned={} applied={} skipped={}",
                apply_summary.planned, apply_summary.applied, apply_summary.skipped
            );
            println!(
                "post-apply sync: mode={} files={} token={}",
                sync_summary.mode.as_str(),
                sync_summary.file_count,
                sync_summary.committed_page_token
            );
        }
    }
    Ok(())
}

fn print_trash_preview(format: OutputFormat, plan: &gdrive_core::TrashPlan) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(plan)?),
        OutputFormat::Table => {
            println!(
                "trash preview: rows={} actionable={} skipped={} files={} folders={} bytes={} recursive={}",
                plan.rows.len(),
                plan.actionable_count,
                plan.skipped_count,
                plan.file_count,
                plan.folder_count,
                plan.total_bytes,
                if plan.recursive { "yes" } else { "no" }
            );
            for row in &plan.rows {
                println!(
                    "{}  {}  reason={} actionable={} descendants=files:{} folders:{} bytes={}",
                    row.item.file.id,
                    row.item.path.primary_path,
                    row.reason.as_str(),
                    if row.actionable { "yes" } else { "no" },
                    row.descendant_file_count,
                    row.descendant_folder_count,
                    row.item.file.size.unwrap_or(0)
                );
            }
        }
    }
    Ok(())
}

fn print_trash_apply_summary(
    format: OutputFormat,
    plan: &gdrive_core::TrashPlan,
    apply_summary: &gdrive_core::TrashApplySummary,
    sync_summary: &gdrive_core::SyncSummary,
    pre_mutation_release: Option<&RemoteDbRelease>,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan,
                "apply_summary": apply_summary,
                "sync_summary": sync_summary,
                "pre_mutation_release": pre_mutation_release,
            }))?
        ),
        OutputFormat::Table => {
            if let Some(release) = pre_mutation_release {
                println!(
                    "pre-mutation release: name={} db_file={} manifest_file={}",
                    release.name, release.db_file.id, release.manifest_file.id
                );
            }
            println!(
                "trash applied: planned={} applied={} skipped={} bytes={}",
                apply_summary.planned,
                apply_summary.applied,
                apply_summary.skipped,
                plan.total_bytes
            );
            println!(
                "post-apply sync: mode={} files={} token={}",
                sync_summary.mode.as_str(),
                sync_summary.file_count,
                sync_summary.committed_page_token
            );
        }
    }
    Ok(())
}

fn print_move_preview(format: OutputFormat, plan: &gdrive_core::MovePlan) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(plan)?),
        OutputFormat::Table => {
            let destination = plan
                .destination
                .as_ref()
                .map(|destination| destination.folder.path.primary_path.as_str())
                .unwrap_or("unknown");
            println!(
                "move preview: rows={} actionable={} skipped={} files={} folders={} destination={}",
                plan.rows.len(),
                plan.actionable_count,
                plan.skipped_count,
                plan.file_count,
                plan.folder_count,
                destination
            );
            if let Some(provisioning) = &plan.provisioning {
                if !provisioning.rows.is_empty() {
                    println!(
                        "destination provisioning: create={} path={}",
                        provisioning.create_count, provisioning.destination_path
                    );
                    for row in &provisioning.rows {
                        println!(
                            "  {} parent={} exists={}",
                            row.folder_path,
                            row.parent_path,
                            if row.exists { "yes" } else { "no" }
                        );
                    }
                }
            }
            for row in &plan.rows {
                println!(
                    "{}  {}  -> {}  reason={} actionable={} descendants=files:{} folders:{}",
                    row.item.file.id,
                    row.item.path.primary_path,
                    row.to_path,
                    row.reason.as_str(),
                    if row.actionable { "yes" } else { "no" },
                    row.descendant_file_count,
                    row.descendant_folder_count
                );
            }
        }
    }
    Ok(())
}

fn print_move_apply_summary(
    format: OutputFormat,
    plan: &gdrive_core::MovePlan,
    apply_summary: &gdrive_core::MoveOrchestrationApplySummary,
    sync_summary: &gdrive_core::SyncSummary,
    pre_mutation_release: Option<&RemoteDbRelease>,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan,
                "apply_summary": apply_summary,
                "sync_summary": sync_summary,
                "pre_mutation_release": pre_mutation_release,
            }))?
        ),
        OutputFormat::Table => {
            if let Some(release) = pre_mutation_release {
                println!(
                    "pre-mutation release: name={} db_file={} manifest_file={}",
                    release.name, release.db_file.id, release.manifest_file.id
                );
            }
            if apply_summary.provisioning_created > 0 {
                println!(
                    "destination provisioning: created={} path={}",
                    apply_summary.provisioning_created,
                    plan.provisioning
                        .as_ref()
                        .map(|provisioning| provisioning.destination_path.as_str())
                        .unwrap_or("unknown")
                );
            }
            let destination = plan
                .destination
                .as_ref()
                .map(|destination| destination.folder.path.primary_path.as_str())
                .unwrap_or("unknown");
            println!(
                "move applied: planned={} applied={} skipped={} destination={}",
                apply_summary.move_summary.planned,
                apply_summary.move_summary.applied,
                apply_summary.move_summary.skipped,
                destination
            );
            println!(
                "post-apply sync: mode={} files={} token={}",
                sync_summary.mode.as_str(),
                sync_summary.file_count,
                sync_summary.committed_page_token
            );
        }
    }
    Ok(())
}

fn print_db_stats(format: OutputFormat, stats: &DatabaseStats) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(stats)?),
        OutputFormat::Table => {
            println!("db_path: {}", stats.db_path);
            println!("db_bytes: {}", stats.db_bytes);
            println!("files: {}", stats.file_count);
            println!("parents: {}", stats.parent_count);
            println!("paths: {}", stats.path_count);
            println!("sync_runs: {}", stats.sync_run_count);
            println!("audit_log: {}", stats.audit_log_count);
            if let Some(generation) = stats.committed_generation {
                println!("committed_generation: {generation}");
            }
            if let Some(token) = &stats.committed_page_token {
                println!("committed_page_token: {token}");
            }
            if let Some(status) = &stats.last_sync_status {
                println!("last_sync_status: {status}");
            }
        }
    }
    Ok(())
}

fn print_db_vacuum(format: OutputFormat, result: &VacuumResult) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputFormat::Table => {
            println!(
                "vacuum complete: db={} before_bytes={} after_bytes={}",
                result.db_path, result.before_bytes, result.after_bytes
            );
        }
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct RemoteDbLocalInfo {
    exists: bool,
    path: String,
    bytes: Option<u64>,
    modified_time: Option<String>,
    db_instance_id: Option<String>,
    schema_version: Option<u32>,
    remote_generation: Option<i64>,
    last_manifest_sha256: Option<String>,
    last_remote_file_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct RemoteDbStatus {
    local: RemoteDbLocalInfo,
    remote: RemoteDbEndpoint,
    decision: String,
}

#[derive(Debug, serde::Serialize)]
struct RemoteDbRelease {
    name: String,
    db_file: RemoteFileMetadata,
    manifest_file: RemoteFileMetadata,
    manifest: RemoteDbManifest,
}

#[derive(Debug, serde::Serialize)]
struct RemoteDbFolderRename {
    from_name: String,
    to_name: String,
    folder_id: String,
    previous_name: String,
    current_name: String,
    renamed: bool,
}

#[derive(Debug, serde::Serialize)]
struct RemoteDbReleaseListing {
    releases: Vec<RemoteDbReleaseListItem>,
}

#[derive(Debug, serde::Serialize)]
struct RemoteDbReleaseListItem {
    name: String,
    db_file: Option<RemoteFileMetadata>,
    manifest_file: Option<RemoteFileMetadata>,
}

#[derive(Debug, serde::Serialize)]
struct OperatorStatus {
    auth: String,
    db: DatabaseStats,
    remote_db: RemoteDbStatus,
    trash: TrashDeadlineSummary,
    release_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct TrashDeadlineSummary {
    total: usize,
    pending: usize,
    expired_estimate: usize,
    warning_window_days: i64,
    warning_count: usize,
}

async fn handle_remote_db_command(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    command: RemoteDbCommand,
    format: OutputFormat,
    no_interactive: bool,
) -> Result<()> {
    match command {
        RemoteDbCommand::Status => {
            let status = build_remote_db_status(gateway, runtime, false).await?;
            print_remote_db_status(format, &status)?;
            Ok(())
        }
        RemoteDbCommand::Sync => {
            let status = build_remote_db_status(gateway, runtime, false).await?;
            match gdrive_core::decide_remote_db_sync(
                status.local.exists,
                status.remote.db_file.is_some(),
            ) {
                RemoteDbSyncDecision::PushLocal => {
                    let pushed = push_remote_db(gateway, runtime).await?;
                    print_remote_db_pushed(format, &pushed)?;
                    Ok(())
                }
                RemoteDbSyncDecision::PullRemote => {
                    let pulled = pull_remote_db(gateway, runtime, &status.remote).await?;
                    print_remote_db_pulled(format, &pulled)?;
                    Ok(())
                }
                RemoteDbSyncDecision::NeedsExplicitDirection => {
                    print_remote_db_status(format, &status)?;
                    bail!("local and remote database both exist; choose `db remote push --yes` or `db remote pull --yes` after reviewing timestamps and checksums")
                }
                RemoteDbSyncDecision::NothingToSync => {
                    print_remote_db_status(format, &status)?;
                    bail!("neither local nor remote database exists; run `sync` first or configure an existing remote DB")
                }
            }
        }
        RemoteDbCommand::Push(args) => {
            ensure_confirmed_remote_write(
                "push local database to Google Drive",
                args.yes,
                no_interactive,
            )?;
            let pushed = push_remote_db(gateway, runtime).await?;
            print_remote_db_pushed(format, &pushed)?;
            Ok(())
        }
        RemoteDbCommand::Pull(args) => {
            ensure_confirmed_remote_write(
                "pull remote database over local database",
                args.yes,
                no_interactive,
            )?;
            let status = build_remote_db_status(gateway, runtime, false).await?;
            let pulled = pull_remote_db(gateway, runtime, &status.remote).await?;
            print_remote_db_pulled(format, &pulled)?;
            Ok(())
        }
        RemoteDbCommand::RenameFolder(args) => {
            let from_name =
                args.from.clone().unwrap_or_else(|| LEGACY_REMOTE_DB_FOLDER_NAME.into());
            let to_name = args.to.clone().unwrap_or_else(|| runtime.remote_db.folder_name.clone());
            ensure_confirmed_remote_write(
                &format!("rename remote database folder `{from_name}` to `{to_name}`"),
                args.yes,
                no_interactive,
            )?;
            let rename = rename_remote_db_folder(gateway, runtime, args).await?;
            print_remote_db_folder_rename(format, &rename)?;
            Ok(())
        }
        RemoteDbCommand::Release(args) => {
            if let Some(RemoteDbReleaseCommand::List(list_args)) = args.command {
                let prefix = list_args
                    .prefix
                    .unwrap_or_else(|| release_file_prefix(&runtime.remote_db.db_name));
                let listing = list_remote_db_releases(gateway, runtime, &prefix).await?;
                print_remote_db_release_listing(format, &listing)?;
                return Ok(());
            }
            let name = args.name.context("release name is required")?;
            ensure_confirmed_remote_write(
                &format!("create named remote database release `{name}`"),
                args.yes,
                no_interactive,
            )?;
            let release = create_remote_db_release(gateway, runtime, &name).await?;
            print_remote_db_release(format, &release)?;
            Ok(())
        }
    }
}

async fn build_remote_db_status(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    create_folder: bool,
) -> Result<RemoteDbStatus> {
    let local = local_db_info(&runtime.db_path)?;
    let remote = load_remote_db_endpoint(gateway, &runtime.remote_db, create_folder).await?;
    validate_remote_endpoint_privacy(&remote)?;
    let decision =
        gdrive_core::decide_remote_db_sync(local.exists, remote.db_file.is_some()).as_str().into();
    Ok(RemoteDbStatus { local, remote, decision })
}

async fn build_operator_status(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    repository: &SqliteInventoryRepository,
    within_days: i64,
) -> Result<OperatorStatus> {
    let auth = match auth_status(gateway).await.map_err(anyhow::Error::msg)?.session {
        Some(session) => format!("logged_in:{}", session.account.email),
        None => "not_logged_in".into(),
    };
    let db = repository.stats()?;
    let remote_db = build_remote_db_status(gateway, runtime, false).await?;
    let entries = repository.load_trashed_files().map_err(anyhow::Error::msg)?;
    let trash = trash_deadline_summary(&entries, within_days);
    let releases =
        list_remote_db_releases(gateway, runtime, &release_file_prefix(&runtime.remote_db.db_name))
            .await
            .unwrap_or(RemoteDbReleaseListing { releases: Vec::new() });
    let mut warnings = Vec::new();
    if trash.warning_count > 0 {
        warnings.push(format!(
            "{} trash item(s) recoverability expires within {} day(s)",
            trash.warning_count, within_days
        ));
    }
    if remote_db.decision == RemoteDbSyncDecision::NeedsExplicitDirection.as_str()
        && !remote_db_is_recorded_in_sync(&remote_db)
    {
        warnings.push(
            "remote DB has both local and remote copies; choose explicit push or pull".into(),
        );
    }
    if auth == "not_logged_in" {
        warnings.push("not logged in; remote checks and live sync require auth login".into());
    }
    Ok(OperatorStatus {
        auth,
        db,
        remote_db,
        trash,
        release_count: releases.releases.len(),
        warnings,
    })
}

fn remote_db_is_recorded_in_sync(status: &RemoteDbStatus) -> bool {
    let Some(local_sha256) = &status.local.last_manifest_sha256 else {
        return false;
    };
    let Some(remote_manifest) = &status.remote.manifest else {
        return false;
    };
    if local_sha256 != &remote_manifest.sha256 {
        return false;
    }
    match (status.local.remote_generation, remote_manifest.db_generation) {
        (Some(local), Some(remote)) => local == remote,
        _ => true,
    }
}

fn trash_deadline_summary(entries: &[TrashedFileEntry], within_days: i64) -> TrashDeadlineSummary {
    let now = Utc::now();
    let warning_deadline = now + Duration::days(within_days);
    let pending = entries
        .iter()
        .filter(|entry| entry.recoverable_until.is_none_or(|deadline| deadline >= now))
        .count();
    let expired_estimate = entries
        .iter()
        .filter(|entry| entry.recoverable_until.is_some_and(|deadline| deadline < now))
        .count();
    let warning_count = entries
        .iter()
        .filter(|entry| {
            entry
                .recoverable_until
                .is_some_and(|deadline| deadline >= now && deadline <= warning_deadline)
        })
        .count();
    TrashDeadlineSummary {
        total: entries.len(),
        pending,
        expired_estimate,
        warning_window_days: within_days,
        warning_count,
    }
}

async fn load_remote_db_endpoint(
    gateway: &dyn DriveGateway,
    config: &RemoteDbConfig,
    create_folder: bool,
) -> Result<RemoteDbEndpoint> {
    let folder = if let Some(folder_id) = &config.folder_id {
        Some(RemoteFileMetadata::from(
            gateway.get_file(folder_id).await.map_err(anyhow::Error::msg)?,
        ))
    } else if let Some(existing) = find_remote_db_folder(gateway, config).await? {
        Some(existing)
    } else if create_folder {
        Some(RemoteFileMetadata::from(
            gateway.create_folder("root", &config.folder_name).await.map_err(anyhow::Error::msg)?,
        ))
    } else {
        None
    };

    let Some(folder) = folder else {
        return Ok(RemoteDbEndpoint::default());
    };
    if folder.mime_type != gdrive_core::GOOGLE_DRIVE_FOLDER_MIME {
        bail!("remote DB folder target `{}` is not a Google Drive folder", folder.id);
    }

    let db_file = gateway
        .find_file_in_folder(&folder.id, &config.db_name)
        .await
        .map_err(anyhow::Error::msg)?;
    let manifest_file = gateway
        .find_file_in_folder(&folder.id, &config.manifest_name)
        .await
        .map_err(anyhow::Error::msg)?;
    let manifest = if let Some(manifest_file) = &manifest_file {
        let bytes = gateway.download_file(&manifest_file.id).await.map_err(anyhow::Error::msg)?;
        Some(serde_json::from_slice::<RemoteDbManifest>(&bytes).with_context(|| {
            format!("failed to parse remote DB manifest `{}`", manifest_file.name)
        })?)
    } else {
        None
    };

    Ok(RemoteDbEndpoint { folder: Some(folder), db_file, manifest_file, manifest })
}

async fn find_remote_db_folder(
    gateway: &dyn DriveGateway,
    config: &RemoteDbConfig,
) -> Result<Option<RemoteFileMetadata>> {
    for name in std::iter::once(&config.folder_name).chain(config.legacy_folder_names.iter()) {
        if let Some(existing) =
            gateway.find_file_in_folder("root", name).await.map_err(anyhow::Error::msg)?
        {
            return Ok(Some(existing));
        }
    }
    Ok(None)
}

async fn rename_remote_db_folder(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    args: RemoteDbRenameFolderArgs,
) -> Result<RemoteDbFolderRename> {
    if runtime.remote_db.folder_id.is_some() {
        bail!(
            "remote DB folder rename by name is unavailable when `remote_folder_id` is configured"
        );
    }
    let from_name = args.from.unwrap_or_else(|| LEGACY_REMOTE_DB_FOLDER_NAME.into());
    let to_name = args.to.unwrap_or_else(|| runtime.remote_db.folder_name.clone());
    if from_name == to_name {
        bail!("remote DB folder source and destination names are both `{from_name}`");
    }

    let existing_to =
        gateway.find_file_in_folder("root", &to_name).await.map_err(anyhow::Error::msg)?;
    let existing_from =
        gateway.find_file_in_folder("root", &from_name).await.map_err(anyhow::Error::msg)?;

    match (existing_from, existing_to) {
        (Some(from), Some(to)) if from.id != to.id => {
            bail!(
                "both remote DB folders `{from_name}` ({}) and `{to_name}` ({}) exist; refusing ambiguous rename",
                from.id,
                to.id
            )
        }
        (None, Some(folder)) => {
            ensure_remote_db_folder_metadata(&folder)?;
            validate_remote_endpoint_privacy(&RemoteDbEndpoint {
                folder: Some(folder.clone()),
                ..RemoteDbEndpoint::default()
            })?;
            Ok(RemoteDbFolderRename {
                from_name,
                to_name,
                folder_id: folder.id.clone(),
                previous_name: folder.name.clone(),
                current_name: folder.name,
                renamed: false,
            })
        }
        (Some(folder), _) => {
            ensure_remote_db_folder_metadata(&folder)?;
            validate_remote_endpoint_privacy(&RemoteDbEndpoint {
                folder: Some(folder.clone()),
                ..RemoteDbEndpoint::default()
            })?;
            let renamed =
                gateway.rename_file(&folder.id, &to_name).await.map_err(anyhow::Error::msg)?;
            ensure_remote_db_folder_metadata(&renamed)?;
            validate_remote_endpoint_privacy(&RemoteDbEndpoint {
                folder: Some(renamed.clone()),
                ..RemoteDbEndpoint::default()
            })?;
            Ok(RemoteDbFolderRename {
                from_name,
                to_name,
                folder_id: renamed.id.clone(),
                previous_name: folder.name,
                current_name: renamed.name,
                renamed: true,
            })
        }
        (None, None) => {
            bail!("remote DB folder `{from_name}` was not found and `{to_name}` does not exist")
        }
    }
}

fn ensure_remote_db_folder_metadata(folder: &RemoteFileMetadata) -> Result<()> {
    if folder.mime_type != gdrive_core::GOOGLE_DRIVE_FOLDER_MIME {
        bail!("remote DB folder target `{}` is not a Google Drive folder", folder.id);
    }
    Ok(())
}

async fn push_remote_db(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
) -> Result<RemoteDbEndpoint> {
    if !runtime.db_path.exists() {
        bail!(
            "local database `{}` does not exist; run `sync` first or pull a remote DB",
            runtime.db_path.display()
        );
    }
    let remote = load_remote_db_endpoint(gateway, &runtime.remote_db, true).await?;
    validate_remote_endpoint_privacy(&remote)?;
    let folder = remote.folder.as_ref().context("remote DB folder is missing")?;

    let snapshot_path = runtime.db_path.with_extension("remote-sync.tmp");
    let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
    let identity = repository.db_identity()?;
    let remote_state = repository.remote_sync_state()?;
    let snapshot_info = repository.snapshot_to(&snapshot_path)?;
    let bytes = fs::read(&snapshot_path)
        .with_context(|| format!("failed to read DB snapshot `{}`", snapshot_path.display()))?;
    let _ = fs::remove_file(&snapshot_path);
    let manifest = gdrive_core::build_remote_db_manifest(
        &runtime.remote_db.db_name,
        &bytes,
        Some(identity.db_instance_id),
        Some(remote_state.generation + 1),
        local_modified_time(&runtime.db_path)?,
        Some(snapshot_info.page_count),
        Some(snapshot_info.schema_version),
        source_label(),
    );
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;

    let db_file = upload_or_update_remote_file(
        gateway,
        &folder.id,
        remote.db_file.as_ref(),
        &runtime.remote_db.db_name,
        "application/vnd.sqlite3",
        bytes,
    )
    .await?;
    upload_or_update_remote_file(
        gateway,
        &folder.id,
        remote.manifest_file.as_ref(),
        &runtime.remote_db.manifest_name,
        "application/json",
        manifest_bytes,
    )
    .await?;
    repository.record_remote_sync(
        RemoteSyncDirection::Push,
        manifest.db_generation,
        &db_file.id,
        &manifest.sha256,
        manifest.uploaded_at,
        manifest.byte_len,
        manifest.source_label.as_deref(),
    )?;
    load_remote_db_endpoint(gateway, &runtime.remote_db, false).await
}

async fn pull_remote_db(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    remote: &RemoteDbEndpoint,
) -> Result<RemoteDbEndpoint> {
    validate_remote_endpoint_privacy(remote)?;
    let db_file = remote.remote_db_file()?;
    let manifest = remote
        .manifest
        .as_ref()
        .context("remote DB manifest is missing; refusing unverified pull")?;
    let bytes = gateway.download_file(&db_file.id).await.map_err(anyhow::Error::msg)?;
    gdrive_core::verify_remote_db_manifest(manifest, &bytes).map_err(anyhow::Error::msg)?;

    if let Some(parent_dir) = runtime.db_path.parent() {
        fs::create_dir_all(parent_dir)?;
    }
    if runtime.db_path.exists() {
        let backup = backup_path_for(&runtime.db_path);
        fs::copy(&runtime.db_path, &backup)
            .with_context(|| format!("failed to create local DB backup `{}`", backup.display()))?;
    }
    let temp_path = runtime.db_path.with_extension("remote-pull.tmp");
    fs::write(&temp_path, bytes)
        .with_context(|| format!("failed to write temporary DB `{}`", temp_path.display()))?;
    fs::rename(&temp_path, &runtime.db_path).with_context(|| {
        format!("failed to install pulled DB at `{}`", runtime.db_path.display())
    })?;
    let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
    repository.record_remote_sync(
        RemoteSyncDirection::Pull,
        manifest.db_generation,
        &db_file.id,
        &manifest.sha256,
        manifest.uploaded_at,
        manifest.byte_len,
        manifest.source_label.as_deref(),
    )?;
    load_remote_db_endpoint(gateway, &runtime.remote_db, false).await
}

async fn create_remote_db_release(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    raw_name: &str,
) -> Result<RemoteDbRelease> {
    if !runtime.db_path.exists() {
        bail!("local database `{}` does not exist; run `sync` first", runtime.db_path.display());
    }
    let release_name = sanitize_release_name(raw_name)?;
    let (db_name, manifest_name) = release_file_names(&runtime.remote_db.db_name, &release_name);
    let remote = load_remote_db_endpoint(gateway, &runtime.remote_db, true).await?;
    validate_remote_endpoint_privacy(&remote)?;
    let folder = remote.folder.as_ref().context("remote DB folder is missing")?;
    if gateway
        .find_file_in_folder(&folder.id, &db_name)
        .await
        .map_err(anyhow::Error::msg)?
        .is_some()
        || gateway
            .find_file_in_folder(&folder.id, &manifest_name)
            .await
            .map_err(anyhow::Error::msg)?
            .is_some()
    {
        bail!("remote DB release `{release_name}` already exists; refusing to overwrite");
    }

    let snapshot_path = runtime.db_path.with_extension(format!("{release_name}.release.tmp"));
    let repository = SqliteInventoryRepository::new(&runtime.db_path)?;
    let identity = repository.db_identity()?;
    let remote_state = repository.remote_sync_state()?;
    let snapshot_info = repository.snapshot_to(&snapshot_path)?;
    let bytes = fs::read(&snapshot_path)
        .with_context(|| format!("failed to read DB snapshot `{}`", snapshot_path.display()))?;
    let _ = fs::remove_file(&snapshot_path);
    let manifest = gdrive_core::build_remote_db_manifest(
        &db_name,
        &bytes,
        Some(identity.db_instance_id),
        Some(remote_state.generation),
        local_modified_time(&runtime.db_path)?,
        Some(snapshot_info.page_count),
        Some(snapshot_info.schema_version),
        source_label(),
    );
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let db_file = gateway
        .upload_file_to_folder(&folder.id, &db_name, "application/vnd.sqlite3", bytes)
        .await
        .map_err(anyhow::Error::msg)?;
    let manifest_file = gateway
        .upload_file_to_folder(&folder.id, &manifest_name, "application/json", manifest_bytes)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok(RemoteDbRelease { name: release_name, db_file, manifest_file, manifest })
}

async fn create_pre_mutation_release(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    operation: &str,
) -> Result<RemoteDbRelease> {
    let name = pre_mutation_release_name(operation);
    create_remote_db_release(gateway, runtime, &name)
        .await
        .with_context(|| {
            format!(
                "failed to create required pre-mutation database release `{name}`; refusing to apply `{operation}`"
            )
        })
}

fn pre_mutation_release_name(operation: &str) -> String {
    format!("before-{operation}-{}", Utc::now().format("%Y%m%dT%H%M%S%9fZ"))
}

async fn list_remote_db_releases(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    prefix: &str,
) -> Result<RemoteDbReleaseListing> {
    let remote = load_remote_db_endpoint(gateway, &runtime.remote_db, false).await?;
    validate_remote_endpoint_privacy(&remote)?;
    let folder = remote.folder.as_ref().context("remote DB folder is missing")?;
    let files =
        gateway.list_files_in_folder(&folder.id, Some(prefix)).await.map_err(anyhow::Error::msg)?;
    let mut releases = BTreeMap::<String, RemoteDbReleaseListItem>::new();
    for file in files {
        if file.name == runtime.remote_db.db_name || file.name == runtime.remote_db.manifest_name {
            continue;
        }
        if let Some(name) = parse_release_name(&runtime.remote_db.db_name, &file.name, false) {
            releases
                .entry(name.clone())
                .or_insert_with(|| RemoteDbReleaseListItem {
                    name,
                    db_file: None,
                    manifest_file: None,
                })
                .db_file = Some(file);
        } else if let Some(name) = parse_release_name(&runtime.remote_db.db_name, &file.name, true)
        {
            releases
                .entry(name.clone())
                .or_insert_with(|| RemoteDbReleaseListItem {
                    name,
                    db_file: None,
                    manifest_file: None,
                })
                .manifest_file = Some(file);
        }
    }
    Ok(RemoteDbReleaseListing { releases: releases.into_values().collect() })
}

fn release_file_names(base_db_name: &str, release_name: &str) -> (String, String) {
    let path = Path::new(base_db_name);
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or(base_db_name);
    let extension = path.extension().and_then(|value| value.to_str());
    let db_name = match extension {
        Some(extension) => format!("{stem}.{release_name}.{extension}"),
        None => format!("{stem}.{release_name}"),
    };
    let manifest_name = format!("{db_name}.manifest.json");
    (db_name, manifest_name)
}

fn release_file_prefix(base_db_name: &str) -> String {
    let path = Path::new(base_db_name);
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or(base_db_name);
    format!("{stem}.")
}

fn parse_release_name(base_db_name: &str, file_name: &str, manifest: bool) -> Option<String> {
    let suffix = if manifest { ".manifest.json" } else { "" };
    let db_name = file_name.strip_suffix(suffix)?;
    let path = Path::new(base_db_name);
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or(base_db_name);
    let extension = path.extension().and_then(|value| value.to_str());
    let prefix = format!("{stem}.");
    let release_part = db_name.strip_prefix(&prefix)?;
    match extension {
        Some(extension) => release_part
            .strip_suffix(&format!(".{extension}"))
            .map(ToOwned::to_owned)
            .filter(|name| !name.is_empty()),
        None => Some(release_part.to_string()).filter(|name| !name.is_empty()),
    }
}

fn sanitize_release_name(raw: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("release name cannot be empty");
    }
    if value.len() > 80 {
        bail!("release name must be 80 characters or fewer");
    }
    if !value.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')) {
        bail!("release name may only contain ASCII letters, digits, '.', '_' and '-'");
    }
    Ok(value.to_string())
}

trait RemoteEndpointExt {
    fn remote_db_file(&self) -> Result<&RemoteFileMetadata>;
}

impl RemoteEndpointExt for RemoteDbEndpoint {
    fn remote_db_file(&self) -> Result<&RemoteFileMetadata> {
        self.db_file.as_ref().context("remote DB file is missing")
    }
}

async fn upload_or_update_remote_file(
    gateway: &dyn DriveGateway,
    folder_id: &str,
    existing: Option<&RemoteFileMetadata>,
    name: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> Result<RemoteFileMetadata> {
    if let Some(existing) = existing {
        gateway
            .update_file_contents(&existing.id, name, mime_type, bytes)
            .await
            .map_err(anyhow::Error::msg)
    } else {
        gateway
            .upload_file_to_folder(folder_id, name, mime_type, bytes)
            .await
            .map_err(anyhow::Error::msg)
    }
}

fn validate_remote_endpoint_privacy(remote: &RemoteDbEndpoint) -> Result<()> {
    let files = [remote.folder.as_ref(), remote.db_file.as_ref(), remote.manifest_file.as_ref()]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let issues = gdrive_core::validate_remote_db_privacy(&files);
    if issues.is_empty() {
        return Ok(());
    }
    eprintln!("SECURITY ALERT: remote DB folder or file is shared. Refusing to sync.");
    for issue in &issues {
        eprintln!(
            "- {} ({}) permission={} type={} role={} target={}",
            issue.file_name,
            issue.file_id,
            issue.permission_id,
            issue.permission_type,
            issue.role,
            issue.target_label
        );
    }
    bail!("SECURITY ALERT: remote DB endpoint must not be shared with anyone")
}

fn local_db_info(path: &Path) -> Result<RemoteDbLocalInfo> {
    match fs::metadata(path) {
        Ok(metadata) => {
            let repository = SqliteInventoryRepository::new(path)?;
            let identity = repository.db_identity()?;
            let remote_state = repository.remote_sync_state()?;
            Ok(RemoteDbLocalInfo {
                exists: true,
                path: path.display().to_string(),
                bytes: Some(metadata.len()),
                modified_time: metadata
                    .modified()
                    .ok()
                    .map(chrono::DateTime::<chrono::Utc>::from)
                    .map(|time| time.to_rfc3339()),
                db_instance_id: Some(identity.db_instance_id),
                schema_version: Some(identity.schema_version),
                remote_generation: Some(remote_state.generation),
                last_manifest_sha256: remote_state.last_manifest_sha256,
                last_remote_file_id: remote_state.last_remote_file_id,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RemoteDbLocalInfo {
            exists: false,
            path: path.display().to_string(),
            bytes: None,
            modified_time: None,
            db_instance_id: None,
            schema_version: None,
            remote_generation: None,
            last_manifest_sha256: None,
            last_remote_file_id: None,
        }),
        Err(error) => Err(error).with_context(|| format!("failed to inspect `{}`", path.display())),
    }
}

fn local_modified_time(path: &Path) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    Ok(fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from))
}

fn source_label() -> Option<String> {
    let host = std::env::var("HOSTNAME").ok();
    let user = std::env::var("USER").ok();
    match (user, host) {
        (Some(user), Some(host)) => Some(format!("{user}@{host}")),
        (Some(user), None) => Some(user),
        (None, Some(host)) => Some(host),
        (None, None) => None,
    }
}

fn backup_path_for(path: &Path) -> PathBuf {
    let timestamp = Utc::now().format("%Y%m%d%H%M%S");
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}.{timestamp}.bak"))
        .unwrap_or_else(|| format!("inventory.db.{timestamp}.bak"));
    path.with_file_name(file_name)
}

fn ensure_confirmed_remote_write(action: &str, yes: bool, no_interactive: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if no_interactive {
        bail!("`db remote {action}` requires `--yes` when `--no-interactive` is set");
    }
    print!("{action}? [y/N]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    if normalized == "y" || normalized == "yes" {
        Ok(())
    } else {
        bail!("remote DB {action} cancelled")
    }
}

fn print_remote_db_status(format: OutputFormat, status: &RemoteDbStatus) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(status)?),
        OutputFormat::Table => {
            println!(
                "remote db status: decision={} local_exists={} remote_exists={}",
                status.decision,
                status.local.exists,
                status.remote.db_file.is_some()
            );
            println!("local: {}", status.local.path);
            if let Some(bytes) = status.local.bytes {
                println!("local_bytes: {bytes}");
            }
            if let Some(modified) = &status.local.modified_time {
                println!("local_modified: {modified}");
            }
            if let Some(instance_id) = &status.local.db_instance_id {
                println!("local_db_instance_id: {instance_id}");
            }
            if let Some(generation) = status.local.remote_generation {
                println!("local_remote_generation: {generation}");
            }
            if let Some(sha256) = &status.local.last_manifest_sha256 {
                println!("local_last_manifest_sha256: {sha256}");
            }
            if let Some(folder) = &status.remote.folder {
                println!("remote_folder: {} {}", folder.id, folder.name);
            }
            if let Some(file) = &status.remote.db_file {
                println!(
                    "remote_db: {} {} bytes={} modified={}",
                    file.id,
                    file.name,
                    file.size.unwrap_or(0),
                    file.modified_time
                        .map(|time| time.to_rfc3339())
                        .unwrap_or_else(|| "unknown".into())
                );
            }
            if let Some(manifest) = &status.remote.manifest {
                println!(
                    "remote_manifest: sha256={} bytes={} uploaded_at={}",
                    manifest.sha256, manifest.byte_len, manifest.uploaded_at
                );
                if let Some(instance_id) = &manifest.db_instance_id {
                    println!("remote_db_instance_id: {instance_id}");
                }
                if let Some(generation) = manifest.db_generation {
                    println!("remote_db_generation: {generation}");
                }
            }
        }
    }
    Ok(())
}

fn print_remote_db_pushed(format: OutputFormat, remote: &RemoteDbEndpoint) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(remote)?),
        OutputFormat::Table => {
            let db = remote.db_file.as_ref().context("remote DB missing after push")?;
            println!("remote db pushed: {} {} bytes={}", db.id, db.name, db.size.unwrap_or(0));
            if let Some(manifest) = &remote.manifest {
                println!("manifest: sha256={} bytes={}", manifest.sha256, manifest.byte_len);
            }
        }
    }
    Ok(())
}

fn print_remote_db_pulled(format: OutputFormat, remote: &RemoteDbEndpoint) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(remote)?),
        OutputFormat::Table => {
            let db = remote.db_file.as_ref().context("remote DB missing after pull")?;
            println!("remote db pulled: {} {} bytes={}", db.id, db.name, db.size.unwrap_or(0));
            if let Some(manifest) = &remote.manifest {
                println!(
                    "verified manifest: sha256={} bytes={}",
                    manifest.sha256, manifest.byte_len
                );
            }
        }
    }
    Ok(())
}

fn print_remote_db_folder_rename(
    format: OutputFormat,
    rename: &RemoteDbFolderRename,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(rename)?),
        OutputFormat::Table => {
            if rename.renamed {
                println!(
                    "remote db folder renamed: from={} to={} folder={}",
                    rename.from_name, rename.to_name, rename.folder_id
                );
            } else {
                println!(
                    "remote db folder already renamed: name={} folder={}",
                    rename.current_name, rename.folder_id
                );
            }
        }
    }
    Ok(())
}

fn print_remote_db_release(format: OutputFormat, release: &RemoteDbRelease) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(release)?),
        OutputFormat::Table => {
            println!(
                "remote db release created: name={} db_file={} manifest_file={}",
                release.name, release.db_file.id, release.manifest_file.id
            );
            println!(
                "manifest: sha256={} bytes={} generation={}",
                release.manifest.sha256,
                release.manifest.byte_len,
                release
                    .manifest
                    .db_generation
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
        }
    }
    Ok(())
}

fn print_remote_db_release_listing(
    format: OutputFormat,
    listing: &RemoteDbReleaseListing,
) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(listing)?),
        OutputFormat::Table => {
            println!("remote db releases: count={}", listing.releases.len());
            for release in &listing.releases {
                let db = release
                    .db_file
                    .as_ref()
                    .map(|file| format!("{} bytes={}", file.id, file.size.unwrap_or(0)))
                    .unwrap_or_else(|| "missing".into());
                let manifest = release
                    .manifest_file
                    .as_ref()
                    .map(|file| file.id.clone())
                    .unwrap_or_else(|| "missing".into());
                println!("{}  db={}  manifest={}", release.name, db, manifest);
            }
        }
    }
    Ok(())
}

fn print_operator_status(format: OutputFormat, status: &OperatorStatus) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(status)?),
        OutputFormat::Table => {
            println!("doctor: warnings={}", status.warnings.len());
            println!("auth: {}", status.auth);
            println!(
                "db: path={} bytes={} files={} paths={} last_sync={}",
                status.db.db_path,
                status.db.db_bytes,
                status.db.file_count,
                status.db.path_count,
                status.db.last_sync_status.as_deref().unwrap_or("unknown")
            );
            println!(
                "remote_db: decision={} remote_exists={} releases={}",
                status.remote_db.decision,
                status.remote_db.remote.db_file.is_some(),
                status.release_count
            );
            println!(
                "trash: total={} pending={} expired_estimate={} warnings_within_{}d={}",
                status.trash.total,
                status.trash.pending,
                status.trash.expired_estimate,
                status.trash.warning_window_days,
                status.trash.warning_count
            );
            for warning in &status.warnings {
                println!("WARNING: {warning}");
            }
        }
    }
    Ok(())
}

fn load_filtered_move_history(
    repository: &SqliteInventoryRepository,
    limit: usize,
    only_pending: bool,
) -> Result<Vec<gdrive_core::MovedFileEntry>> {
    let mut entries = repository.load_moved_files().map_err(anyhow::Error::msg)?;
    if only_pending {
        let applied = entries
            .iter()
            .filter(|entry| entry.status == "applied")
            .map(|entry| (entry.file_id.clone(), entry.at))
            .collect::<std::collections::BTreeMap<_, _>>();
        entries.retain(|entry| {
            entry.status == "pending"
                && applied.get(&entry.file_id).is_none_or(|applied_at| entry.at >= *applied_at)
        });
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.at));
    if entries.len() > limit {
        entries.truncate(limit);
    }
    Ok(entries)
}

fn print_move_history(format: OutputFormat, entries: &[gdrive_core::MovedFileEntry]) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(entries)?),
        OutputFormat::Table => {
            println!("move history: rows={}", entries.len());
            for entry in entries {
                println!(
                    "{}  status={}  explicit={}  {} -> {}",
                    entry.at.to_rfc3339(),
                    entry.status,
                    if entry.explicitly_requested { "yes" } else { "no" },
                    entry.from_path,
                    entry.to_path
                );
            }
        }
    }
    Ok(())
}

fn load_filtered_trash_history(
    repository: &SqliteInventoryRepository,
    limit: usize,
    only_pending: bool,
) -> Result<Vec<TrashedFileEntry>> {
    let now = Utc::now();
    let mut entries = repository.load_trashed_files().map_err(anyhow::Error::msg)?;
    if only_pending {
        entries.retain(|entry| entry.recoverable_until.is_none_or(|deadline| deadline >= now));
    }
    entries.sort_by_key(|entry| entry.recoverable_until);
    if entries.len() > limit {
        entries.truncate(limit);
    }
    Ok(entries)
}

fn print_trash_history(format: OutputFormat, entries: &[TrashedFileEntry]) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(entries)?),
        OutputFormat::Table => {
            println!("trash history: rows={}", entries.len());
            for entry in entries {
                println!(
                    "{}  recoverable_until={}  explicit={}  {}",
                    entry.at.to_rfc3339(),
                    entry
                        .recoverable_until
                        .map(|deadline| deadline.to_rfc3339())
                        .unwrap_or_else(|| "unknown".into()),
                    if entry.explicitly_requested { "yes" } else { "no" },
                    entry.file_path
                );
            }
        }
    }
    Ok(())
}

fn print_trash_status(
    format: OutputFormat,
    entries: &[TrashedFileEntry],
    within_days: i64,
) -> Result<()> {
    let now = Utc::now();
    let warning_deadline = now + Duration::days(within_days);
    let pending = entries
        .iter()
        .filter(|entry| entry.recoverable_until.is_none_or(|deadline| deadline >= now))
        .count();
    let expired = entries
        .iter()
        .filter(|entry| entry.recoverable_until.is_some_and(|deadline| deadline < now))
        .count();
    let warning_entries = entries
        .iter()
        .filter(|entry| {
            entry
                .recoverable_until
                .is_some_and(|deadline| deadline >= now && deadline <= warning_deadline)
        })
        .collect::<Vec<_>>();
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "total": entries.len(),
                "pending": pending,
                "expired_estimate": expired,
                "warning_window_days": within_days,
                "warning_count": warning_entries.len(),
                "warnings": warning_entries,
            }))?
        ),
        OutputFormat::Table => {
            println!(
                "trash status: total={} pending={} expired_estimate={} warning_window_days={} warnings={}",
                entries.len(),
                pending,
                expired,
                within_days,
                warning_entries.len()
            );
            if !warning_entries.is_empty() {
                println!("WARNING: trash recoverability expires within {within_days} day(s):");
                for entry in warning_entries.iter().take(25) {
                    println!(
                        "{}  {}",
                        entry
                            .recoverable_until
                            .map(|deadline| deadline.to_rfc3339())
                            .unwrap_or_else(|| "unknown".into()),
                        entry.file_path
                    );
                }
            }
        }
    }
    Ok(())
}

fn find_trash_restore_entries(
    repository: &SqliteInventoryRepository,
    args: &TrashRestoreArgs,
) -> Result<Vec<TrashedFileEntry>> {
    let mut entries = repository.load_trashed_files().map_err(anyhow::Error::msg)?;
    if let Some(file_id) = &args.file_id {
        entries.retain(|entry| entry.file_id == *file_id);
    }
    if let Some(path_contains) = &args.path_contains {
        entries.retain(|entry| entry.file_path.contains(path_contains));
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.at));
    if entries.len() > args.limit {
        entries.truncate(args.limit);
    }
    if entries.is_empty() {
        bail!("no trash history rows matched the restore selector");
    }
    Ok(entries)
}

fn print_trash_restore_guidance(format: OutputFormat, entries: &[TrashedFileEntry]) -> Result<()> {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "restore_supported_by_cli": false,
                "manual_restore_steps": [
                    "Open Google Drive Trash while signed in as the owning account.",
                    "Search by file name or use the file ID in the Drive URL when available.",
                    "Select the item and choose Restore before recoverable_until.",
                    "Run drive-warden sync --full after restoring to refresh the local snapshot."
                ],
                "matches": entries,
            }))?
        ),
        OutputFormat::Table => {
            println!("trash restore guidance: matches={}", entries.len());
            println!("CLI restore is not implemented; restore these items manually in Google Drive Trash.");
            println!(
                "After restore, run `drive-warden sync --full` to refresh the local inventory."
            );
            for entry in entries {
                println!(
                    "{}  recoverable_until={}  file_id={}  path={}",
                    entry.file_name,
                    entry
                        .recoverable_until
                        .map(|deadline| deadline.to_rfc3339())
                        .unwrap_or_else(|| "unknown".into()),
                    entry.file_id,
                    entry.file_path
                );
            }
        }
    }
    Ok(())
}

fn confirm_unshare_apply(plan: &gdrive_core::UnsharePlan) -> Result<()> {
    if let Some(retain_copy) = &plan.retain_copy {
        print!(
            "Create retained copies for {} root item(s), then apply {} actionable unshare change(s) and skip {} row(s)? [y/N]: ",
            retain_copy.root_count, plan.actionable_count, plan.skipped_count
        );
    } else {
        print!(
            "Apply {} actionable unshare change(s) and skip {} row(s)? [y/N]: ",
            plan.actionable_count, plan.skipped_count
        );
    }
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    if normalized == "y" || normalized == "yes" {
        Ok(())
    } else {
        bail!("unshare apply cancelled")
    }
}

fn confirm_move_apply(plan: &gdrive_core::MovePlan) -> Result<()> {
    let destination = plan
        .destination
        .as_ref()
        .map(|destination| destination.folder.path.primary_path.as_str())
        .unwrap_or("unknown");
    let create_count =
        plan.provisioning.as_ref().map(|provisioning| provisioning.create_count).unwrap_or(0);
    if create_count > 0 {
        print!(
            "Create {} destination folder(s), move {} actionable item(s) into `{}`, skip {} row(s)? [y/N]: ",
            create_count, plan.actionable_count, destination, plan.skipped_count
        );
    } else {
        print!(
            "Move {} actionable item(s) into `{}`, skip {} row(s)? [y/N]: ",
            plan.actionable_count, destination, plan.skipped_count
        );
    }
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    if normalized == "y" || normalized == "yes" {
        Ok(())
    } else {
        bail!("move apply cancelled")
    }
}

fn confirm_trash_apply(plan: &gdrive_core::TrashPlan) -> Result<()> {
    print!(
        "Move {} actionable item(s) to trash, skip {} row(s), affecting {} byte(s)? [y/N]: ",
        plan.actionable_count, plan.skipped_count, plan.total_bytes
    );
    if plan.folder_count > 0 && plan.recursive {
        print!("This includes recursive folder trashing. ");
    }
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    if normalized == "y" || normalized == "yes" {
        Ok(())
    } else {
        bail!("trash apply cancelled")
    }
}

fn print_completions(shell: CompletionShell) -> Result<()> {
    let mut command = Cli::command();
    let mut stdout = io::stdout();
    match shell {
        CompletionShell::Bash => {
            generate_shell(clap_complete::shells::Bash, &mut command, &mut stdout)
        }
        CompletionShell::Zsh => {
            generate_shell(clap_complete::shells::Zsh, &mut command, &mut stdout)
        }
        CompletionShell::Fish => {
            generate_shell(clap_complete::shells::Fish, &mut command, &mut stdout)
        }
    }
    Ok(())
}

fn generate_shell<G: Generator>(generator: G, command: &mut clap::Command, output: &mut dyn Write) {
    generate(generator, command, APP_NAME, output);
}

fn decorate_sync_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("410 gone") || lower.contains("invalid page token") {
        format!("{message}. Rerun `drive-warden sync --full` to rebuild the local snapshot.")
    } else if lower.contains("revoked")
        || lower.contains("expired")
        || lower.contains("invalid_grant")
    {
        format!("{message}. Run `drive-warden auth login` to refresh the session.")
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_file(id: &str, name: &str) -> RemoteFileMetadata {
        RemoteFileMetadata {
            id: id.into(),
            name: name.into(),
            mime_type: "application/vnd.sqlite3".into(),
            size: Some(123),
            modified_time: None,
            owned_by_me: true,
            shared: false,
            permissions: Vec::new(),
        }
    }

    fn remote_status(local_sha: Option<&str>, local_generation: Option<i64>) -> RemoteDbStatus {
        RemoteDbStatus {
            local: RemoteDbLocalInfo {
                exists: true,
                path: "data/inventory.db".into(),
                bytes: Some(123),
                modified_time: None,
                db_instance_id: Some("db-1".into()),
                schema_version: Some(5),
                remote_generation: local_generation,
                last_manifest_sha256: local_sha.map(ToOwned::to_owned),
                last_remote_file_id: Some("remote-db".into()),
            },
            remote: RemoteDbEndpoint {
                folder: Some(remote_file("folder", DEFAULT_REMOTE_DB_FOLDER_NAME)),
                db_file: Some(remote_file("remote-db", "inventory.db")),
                manifest_file: Some(remote_file("manifest", "inventory.db.manifest.json")),
                manifest: Some(RemoteDbManifest {
                    version: 1,
                    db_name: "inventory.db".into(),
                    db_instance_id: Some("db-1".into()),
                    db_generation: Some(7),
                    sha256: "sha-remote".into(),
                    byte_len: 123,
                    uploaded_at: Utc::now(),
                    local_modified_time: None,
                    sqlite_page_count: Some(10),
                    sqlite_schema_version: Some(5),
                    source_label: Some("test".into()),
                }),
            },
            decision: RemoteDbSyncDecision::NeedsExplicitDirection.as_str().into(),
        }
    }

    fn trashed_entry(
        file_id: &str,
        recoverable_until: Option<chrono::DateTime<Utc>>,
    ) -> TrashedFileEntry {
        TrashedFileEntry {
            at: Utc::now(),
            recoverable_until,
            command: "trash".into(),
            file_id: file_id.into(),
            file_name: format!("{file_id}.bin"),
            file_path: format!("/Trash/{file_id}.bin"),
            mime_type: "application/octet-stream".into(),
            size: Some(1),
            md5_checksum: None,
            modified_time: None,
            trashed_via_file_id: None,
            trashed_via_path: None,
            explicitly_requested: true,
            descendant_file_count: 0,
            descendant_folder_count: 0,
            trash_via: "explicit".into(),
            note: None,
        }
    }

    #[test]
    fn release_name_helpers_validate_and_parse_tags() {
        assert_eq!(sanitize_release_name(" coors-v1 ").expect("tag"), "coors-v1");
        assert!(sanitize_release_name("").is_err());
        assert!(sanitize_release_name("bad/tag").is_err());
        assert!(sanitize_release_name(&"x".repeat(81)).is_err());

        assert_eq!(
            release_file_names("inventory.db", "coors-v1"),
            ("inventory.coors-v1.db".into(), "inventory.coors-v1.db.manifest.json".into())
        );
        assert_eq!(release_file_prefix("inventory.db"), "inventory.");
        assert_eq!(
            parse_release_name("inventory.db", "inventory.coors-v1.db", false).as_deref(),
            Some("coors-v1")
        );
        assert_eq!(
            parse_release_name("inventory.db", "inventory.coors-v1.db.manifest.json", true)
                .as_deref(),
            Some("coors-v1")
        );
        assert!(parse_release_name("inventory.db", "inventory.db", false).is_none());
        assert!(parse_release_name("inventory.db", "other.coors-v1.db", false).is_none());
    }

    #[test]
    fn doctor_sync_helper_distinguishes_current_and_conflicting_remote_db() {
        assert!(remote_db_is_recorded_in_sync(&remote_status(Some("sha-remote"), Some(7))));
        assert!(!remote_db_is_recorded_in_sync(&remote_status(Some("sha-other"), Some(7))));
        assert!(!remote_db_is_recorded_in_sync(&remote_status(Some("sha-remote"), Some(6))));
        assert!(!remote_db_is_recorded_in_sync(&remote_status(None, Some(7))));

        let mut missing_manifest = remote_status(Some("sha-remote"), Some(7));
        missing_manifest.remote.manifest = None;
        assert!(!remote_db_is_recorded_in_sync(&missing_manifest));
    }

    #[test]
    fn trash_deadline_summary_counts_pending_expired_and_warnings() {
        let now = Utc::now();
        let entries = vec![
            trashed_entry("soon", Some(now + Duration::days(2))),
            trashed_entry("later", Some(now + Duration::days(20))),
            trashed_entry("expired", Some(now - Duration::days(1))),
            trashed_entry("unknown", None),
        ];
        let summary = trash_deadline_summary(&entries, 7);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.pending, 3);
        assert_eq!(summary.expired_estimate, 1);
        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.warning_window_days, 7);
    }

    #[test]
    fn decorate_sync_error_adds_operator_remediation() {
        assert!(decorate_sync_error("410 Gone").contains("sync --full"));
        assert!(decorate_sync_error("invalid_grant").contains("auth login"));
        assert_eq!(decorate_sync_error("plain failure"), "plain failure");
    }

    #[test]
    fn inventory_query_helpers_parse_filters_and_reject_invalid_values() {
        let filters = QueryFilters {
            file_id: Some("file-id".into()),
            name: Some("model".into()),
            mime: Some("application".into()),
            older_than: Some("30d".into()),
            larger_than: Some(10),
            in_folder: Some("folder-id".into()),
            path: Some("[orphan]/*".into()),
            shared: true,
            shared_with: Some("domain:example.com".into()),
            owner_scope: Some("mine".into()),
            actionable_only: true,
            duplicate_of: Some("hash".into()),
            limit: Some(25),
            offset: Some(5),
        };
        let query = build_inventory_query(&filters, Some(20)).expect("query");
        assert_eq!(query.file_id.as_deref(), Some("file-id"));
        assert_eq!(query.name_contains.as_deref(), Some("model"));
        assert_eq!(query.mime_contains.as_deref(), Some("application"));
        assert_eq!(query.older_than_days, Some(30));
        assert_eq!(query.larger_than, Some(20));
        assert_eq!(query.in_folder.as_deref(), Some("folder-id"));
        assert_eq!(query.path_glob.as_deref(), Some("[orphan]/*"));
        assert!(query.shared_only);
        assert!(
            matches!(query.shared_with, Some(SharedWithFilter::Domain(domain)) if domain == "example.com")
        );
        assert_eq!(query.owner_scope, OwnerScope::Mine);
        assert!(query.actionable_only);
        assert_eq!(query.duplicate_of.as_deref(), Some("hash"));
        assert_eq!(query.limit, Some(25));
        assert_eq!(query.offset, 5);

        assert_eq!(parse_older_than_days("90").expect("days"), 90);
        assert!(parse_older_than_days("bad").is_err());
        assert!(matches!(
            parse_shared_with_filter("anyone").expect("anyone"),
            SharedWithFilter::Anyone
        ));
        assert!(matches!(
            parse_shared_with_filter("email:user@example.com").expect("email"),
            SharedWithFilter::Email(email) if email == "user@example.com"
        ));
        assert!(parse_shared_with_filter("group:team").is_err());
        assert!(build_inventory_query(
            &QueryFilters { owner_scope: Some("theirs".into()), ..QueryFilters::default() },
            None,
        )
        .is_err());
    }
}
