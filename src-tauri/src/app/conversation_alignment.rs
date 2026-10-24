use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::backup::Backup;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sysinfo::{ProcessesToUpdate, System};
use thiserror::Error;

const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ROLLOUT_HEAD_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_RECOVERY_ROLLOUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECOVERY_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECOVERY_MANIFEST_BYTES: u64 = 128 * 1024 * 1024;
const LEGACY_RECOVERY_MANIFEST_VERSION: u32 = 2;
const RECOVERY_MANIFEST_VERSION: u32 = 3;

#[derive(Debug, Clone)]
pub struct AlignmentPaths {
    pub codex_home: PathBuf,
    pub backup_root: PathBuf,
}

impl AlignmentPaths {
    pub fn from_codex_home(codex_home: PathBuf) -> Self {
        let backup_root = codex_home.join("recovery-backups");
        Self {
            codex_home,
            backup_root,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessState {
    pub desktop_running: bool,
    pub cli_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessClass {
    Desktop,
    Cli,
}

pub fn is_codex_process_name(name: &OsStr) -> bool {
    name.eq_ignore_ascii_case(OsStr::new("codex"))
        || name.eq_ignore_ascii_case(OsStr::new("codex.exe"))
}

pub fn classify_codex_process(
    executable: Option<&Path>,
    installed_root: Option<&Path>,
) -> ProcessClass {
    match (executable, installed_root) {
        (Some(executable), Some(installed_root)) if path_is_within(executable, installed_root) => {
            ProcessClass::Desktop
        }
        _ => ProcessClass::Cli,
    }
}

#[cfg(target_os = "windows")]
fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let root = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase();
    !root.is_empty()
        && (path == root
            || path
                .strip_prefix(&root)
                .is_some_and(|suffix| suffix.starts_with('/')))
}

#[cfg(not(target_os = "windows"))]
fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

pub trait ProcessGate {
    fn process_state(&self) -> Result<ProcessState, AlignmentError>;
    fn stop_desktop(&self) -> Result<bool, AlignmentError>;
}

pub(crate) struct SystemAlignmentProcessGate {
    installed_root: Option<PathBuf>,
    stop_desktop: fn() -> Result<bool, AlignmentError>,
}

impl SystemAlignmentProcessGate {
    pub(crate) fn new(
        installed_root: Option<PathBuf>,
        stop_desktop: fn() -> Result<bool, AlignmentError>,
    ) -> Self {
        Self {
            installed_root: installed_root.and_then(|path| path.canonicalize().ok()),
            stop_desktop,
        }
    }
}

impl ProcessGate for SystemAlignmentProcessGate {
    fn process_state(&self) -> Result<ProcessState, AlignmentError> {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let mut state = ProcessState::default();
        for process in system.processes().values() {
            if !is_codex_process_name(process.name()) {
                continue;
            }
            let executable = process.exe().and_then(|path| path.canonicalize().ok());
            match classify_codex_process(executable.as_deref(), self.installed_root.as_deref()) {
                ProcessClass::Desktop => state.desktop_running = true,
                ProcessClass::Cli => state.cli_running = true,
            }
        }
        Ok(state)
    }

    fn stop_desktop(&self) -> Result<bool, AlignmentError> {
        (self.stop_desktop)()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentScan {
    pub scan_token: String,
    pub active_provider: String,
    pub database_count: usize,
    pub thread_count: u64,
    pub mismatched_threads: u64,
    pub rollout_count: usize,
    pub mismatched_rollouts: usize,
    pub skipped_rollouts: usize,
    pub skip_reasons: Vec<String>,
    pub desktop_running: bool,
    pub cli_running: bool,
    pub can_execute: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentOutcome {
    Committed,
    NoChanges,
    StalePlan,
    Restored,
    RecoveryRequired,
}

pub fn restart_allowed(
    outcome: AlignmentOutcome,
    write_verified: bool,
    rollback_verified: bool,
) -> bool {
    match outcome {
        AlignmentOutcome::Committed => write_verified,
        AlignmentOutcome::Restored => rollback_verified,
        AlignmentOutcome::NoChanges
        | AlignmentOutcome::StalePlan
        | AlignmentOutcome::RecoveryRequired => false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentReport {
    pub outcome: AlignmentOutcome,
    pub recovery_id: Option<String>,
    pub backup_dir: Option<String>,
    pub threads_updated: u64,
    pub rollouts_updated: usize,
    pub rollouts_skipped: usize,
    pub write_verified: bool,
    pub rollback_verified: bool,
    pub codex_was_running: bool,
    pub restart_allowed: bool,
    pub undo_allowed: bool,
    pub elevation_required: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Error)]
pub enum AlignmentError {
    #[error("Codex home is unavailable")]
    CodexHomeUnavailable,
    #[error("Codex config.toml is unavailable")]
    ConfigUnavailable,
    #[error("Codex config.toml is invalid")]
    InvalidConfig,
    #[error("Codex config.toml has no valid top-level model_provider")]
    MissingProvider,
    #[error("conversation metadata contains an unsafe path")]
    UnsafePath,
    #[error("conversation database schema is unsupported")]
    UnsupportedSchema,
    #[error("conversation metadata could not be read")]
    Io,
    #[error("conversation metadata permission denied")]
    PermissionDenied,
    #[error("conversation database could not be read")]
    Database,
    #[error("Codex CLI is running")]
    CliRunning,
    #[error("conversation metadata is not safe to update")]
    BlockedMetadata,
    #[error("conversation metadata changed after the recovery point")]
    StaleRecovery,
    #[error("conversation metadata backup is invalid")]
    InvalidRecovery,
    #[error("conversation metadata backup failed")]
    Backup,
    #[error("conversation metadata update failed")]
    Mutation,
}

impl AlignmentError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::CodexHomeUnavailable => "alignment_codex_home_unavailable",
            Self::ConfigUnavailable => "alignment_config_unavailable",
            Self::InvalidConfig => "alignment_config_invalid",
            Self::MissingProvider => "alignment_provider_missing",
            Self::UnsafePath => "alignment_unsafe_path",
            Self::UnsupportedSchema => "alignment_unsupported_schema",
            Self::Io => "alignment_io",
            Self::PermissionDenied => "alignment_permission_denied",
            Self::Database => "alignment_database",
            Self::CliRunning => "alignment_cli_running",
            Self::BlockedMetadata => "alignment_blocked_metadata",
            Self::StaleRecovery => "alignment_stale_recovery",
            Self::InvalidRecovery => "alignment_invalid_recovery",
            Self::Backup => "alignment_backup_failed",
            Self::Mutation => "alignment_mutation_failed",
        }
    }
}

fn io_error_or(error: std::io::Error, fallback: AlignmentError) -> AlignmentError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        AlignmentError::PermissionDenied
    } else {
        fallback
    }
}

fn sqlite_error_or(error: rusqlite::Error, fallback: AlignmentError) -> AlignmentError {
    match error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code:
                    rusqlite::ffi::ErrorCode::PermissionDenied
                    | rusqlite::ffi::ErrorCode::ReadOnly
                    | rusqlite::ffi::ErrorCode::AuthorizationForStatementDenied,
                ..
            },
            _,
        ) => AlignmentError::PermissionDenied,
        _ => fallback,
    }
}

#[derive(Debug)]
struct AlignmentPlan {
    home: PathBuf,
    provider: String,
    databases: Vec<DatabaseImpact>,
    rollouts: Vec<RolloutImpact>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct DatabaseImpact {
    path: PathBuf,
    file_len: u64,
    modified_nanos: u128,
    thread_count: u64,
    mismatched_threads: u64,
    provider_counts: Vec<(Option<String>, u64)>,
}

#[derive(Debug)]
struct RolloutImpact {
    path: PathBuf,
    file_len: u64,
    modified_nanos: u128,
    head_sha256: String,
    raw_head: Vec<u8>,
    provider: Option<String>,
    class: RolloutClass,
    recovery_id: Option<String>,
    needs_desktop_upgrade: bool,
    upgrade_skip_reason: Option<&'static str>,
    recovery_artifact_bytes: u64,
    recovery_file_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RolloutClass {
    Aligned,
    Repairable,
    Skipped(&'static str),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryManifest {
    version: u32,
    recovery_id: String,
    provider: String,
    #[serde(default)]
    rollouts_skipped: usize,
    databases: Vec<DatabaseBackup>,
    rollouts: Vec<RolloutBackup>,
    #[serde(default)]
    rollout_upgrades: Vec<RolloutUpgradeBackup>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseBackup {
    relative_path: String,
    backup_file: String,
    thread_count: u64,
    provider_counts: Vec<(Option<String>, u64)>,
    original_rows: Vec<ThreadProviderPreimage>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadProviderPreimage {
    id: String,
    provider: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutBackup {
    relative_path: String,
    original_head_file: String,
    aligned_head_file: String,
    #[serde(skip)]
    original_head: Vec<u8>,
    original_head_sha256: String,
    #[serde(skip)]
    aligned_head: Vec<u8>,
    aligned_head_sha256: String,
    original_tail_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutUpgradeBackup {
    relative_path: String,
    original_file: String,
    upgraded_file: String,
    #[serde(skip)]
    original: Vec<u8>,
    original_sha256: String,
    #[serde(skip)]
    upgraded: Vec<u8>,
    upgraded_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignmentFault {
    None,
    AfterDatabaseCommit,
    AfterRolloutWrite,
    BeforeRolloutWrite,
    RollbackVerificationFailure,
    PermissionBeforeWrite,
    PermissionAfterDatabaseCommit,
    PermissionBeforeRolloutWrite,
    PermissionDuringRollback,
}

#[derive(Debug, Default)]
struct AlignmentMutationProgress {
    database_paths: BTreeSet<String>,
    rollout_paths: BTreeSet<String>,
}

pub fn scan_alignment_at(
    paths: &AlignmentPaths,
    processes: ProcessState,
) -> Result<AlignmentScan, AlignmentError> {
    let plan = build_alignment_plan(paths, processes)?;
    Ok(scan_from_plan(&plan, processes))
}

fn build_alignment_plan(
    paths: &AlignmentPaths,
    processes: ProcessState,
) -> Result<AlignmentPlan, AlignmentError> {
    build_alignment_plan_with_artifact_limit(paths, processes, MAX_RECOVERY_ARTIFACT_BYTES)
}

fn build_alignment_plan_with_artifact_limit(
    paths: &AlignmentPaths,
    _processes: ProcessState,
    artifact_limit: u64,
) -> Result<AlignmentPlan, AlignmentError> {
    let home = canonical_directory(&paths.codex_home)?;
    let config = fs::read(home.join("config.toml")).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => AlignmentError::ConfigUnavailable,
        std::io::ErrorKind::PermissionDenied => AlignmentError::PermissionDenied,
        _ => AlignmentError::Io,
    })?;
    let provider = active_provider(&config)?;
    let databases = discover_databases(&home)?
        .into_iter()
        .map(|path| scan_database(&path, &provider))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rollouts = discover_rollouts(&home)?
        .into_iter()
        .map(|path| scan_rollout(&path, &provider))
        .collect::<Result<Vec<_>, _>>()?;
    apply_recovery_artifact_budget(&mut rollouts, &provider, artifact_limit)?;

    Ok(AlignmentPlan {
        home,
        provider,
        databases,
        rollouts,
        warnings: Vec::new(),
    })
}

fn apply_recovery_artifact_budget(
    rollouts: &mut [RolloutImpact],
    provider: &str,
    artifact_limit: u64,
) -> Result<(), AlignmentError> {
    let mut artifact_bytes = 0_u64;
    for rollout in rollouts.iter_mut() {
        if rollout.class == RolloutClass::Repairable {
            let recovery_bytes = provider_repair_artifact_bytes(rollout, provider)?;
            match artifact_bytes
                .checked_add(recovery_bytes)
                .filter(|total| *total <= artifact_limit)
            {
                Some(total) => artifact_bytes = total,
                None => {
                    rollout.class = RolloutClass::Skipped("alignment_recovery_artifact_budget");
                    rollout.needs_desktop_upgrade = false;
                    rollout.upgrade_skip_reason = Some("alignment_recovery_artifact_budget");
                    rollout.recovery_artifact_bytes = 0;
                    rollout.recovery_file_sha256 = None;
                }
            }
        }
    }
    for rollout in rollouts.iter_mut() {
        if !rollout.needs_desktop_upgrade {
            continue;
        }
        let provider_repair_bytes = if rollout.class == RolloutClass::Repairable {
            provider_repair_artifact_bytes(rollout, provider)?
        } else {
            0
        };
        let without_provider_repair = artifact_bytes
            .checked_sub(provider_repair_bytes)
            .ok_or(AlignmentError::BlockedMetadata)?;
        match without_provider_repair
            .checked_add(rollout.recovery_artifact_bytes)
            .filter(|total| *total <= artifact_limit)
        {
            Some(total) => artifact_bytes = total,
            None => {
                rollout.needs_desktop_upgrade = false;
                rollout.upgrade_skip_reason = Some("alignment_recovery_artifact_budget");
                rollout.recovery_artifact_bytes = 0;
                rollout.recovery_file_sha256 = None;
            }
        }
    }
    Ok(())
}

fn provider_repair_artifact_bytes(
    rollout: &RolloutImpact,
    provider: &str,
) -> Result<u64, AlignmentError> {
    let aligned_head = aligned_rollout_head(&rollout.raw_head, provider)?;
    (rollout.raw_head.len() as u64)
        .checked_add(aligned_head.len() as u64)
        .ok_or(AlignmentError::BlockedMetadata)
}

fn scan_from_plan(plan: &AlignmentPlan, processes: ProcessState) -> AlignmentScan {
    let scan_token = semantic_token(&plan);
    let mut warnings = plan.warnings.clone();
    if processes.cli_running && has_desktop_upgrades(plan) {
        warnings.push("alignment_cli_running".to_string());
    }

    let mut skip_reasons = BTreeSet::new();
    for rollout in &plan.rollouts {
        if let RolloutClass::Skipped(reason) = rollout.class {
            skip_reasons.insert(reason.to_string());
        }
        if let Some(reason) = rollout.upgrade_skip_reason {
            skip_reasons.insert(reason.to_string());
        }
    }

    AlignmentScan {
        scan_token,
        active_provider: plan.provider.clone(),
        database_count: plan.databases.len(),
        thread_count: plan
            .databases
            .iter()
            .map(|database| database.thread_count)
            .sum(),
        mismatched_threads: plan
            .databases
            .iter()
            .map(|database| database.mismatched_threads)
            .sum(),
        rollout_count: plan.rollouts.len(),
        mismatched_rollouts: plan
            .rollouts
            .iter()
            .filter(|rollout| {
                rollout.class == RolloutClass::Repairable || rollout.needs_desktop_upgrade
            })
            .count(),
        skipped_rollouts: plan
            .rollouts
            .iter()
            .filter(|rollout| {
                matches!(rollout.class, RolloutClass::Skipped(_))
                    || rollout.upgrade_skip_reason.is_some()
            })
            .count(),
        skip_reasons: skip_reasons.into_iter().collect(),
        desktop_running: processes.desktop_running,
        cli_running: processes.cli_running,
        can_execute: warnings.is_empty(),
        warnings,
    }
}

pub fn execute_alignment_at(
    paths: &AlignmentPaths,
    scan_token: &str,
    process: &dyn ProcessGate,
) -> Result<AlignmentReport, AlignmentError> {
    execute_alignment_at_with_fault(paths, scan_token, process, AlignmentFault::None)
}

fn execute_alignment_at_with_fault(
    paths: &AlignmentPaths,
    scan_token: &str,
    process: &dyn ProcessGate,
    fault: AlignmentFault,
) -> Result<AlignmentReport, AlignmentError> {
    let initial_state = process.process_state()?;
    let initial_plan = build_alignment_plan(paths, initial_state)?;
    if semantic_token(&initial_plan) != scan_token {
        let mut report = simple_report(AlignmentOutcome::StalePlan, false);
        report.rollouts_skipped = skipped_rollout_count(&initial_plan);
        return Ok(report);
    }
    if initial_state.cli_running && has_desktop_upgrades(&initial_plan) {
        return Err(AlignmentError::CliRunning);
    }
    if !initial_plan.warnings.is_empty() {
        return Err(AlignmentError::BlockedMetadata);
    }
    if plan_change_count(&initial_plan) == 0 {
        let mut report = simple_report(AlignmentOutcome::NoChanges, false);
        report.rollouts_skipped = skipped_rollout_count(&initial_plan);
        report.write_verified = true;
        return Ok(report);
    }

    let codex_was_running = initial_state.desktop_running;
    if codex_was_running && !process.stop_desktop()? {
        return Err(AlignmentError::Mutation);
    }
    let settled_state = match process.process_state() {
        Ok(state) => state,
        Err(error) => return pre_mutation_failure(error, codex_was_running, None),
    };
    if settled_state.desktop_running {
        return pre_mutation_failure(AlignmentError::Mutation, codex_was_running, None);
    }
    let plan = match build_alignment_plan(paths, settled_state) {
        Ok(plan) => plan,
        Err(error) => return pre_mutation_failure(error, codex_was_running, None),
    };
    if semantic_token(&plan) != scan_token {
        let mut report = simple_report(AlignmentOutcome::StalePlan, codex_was_running);
        report.rollouts_skipped = skipped_rollout_count(&plan);
        report.restart_allowed = codex_was_running;
        return Ok(report);
    }
    if settled_state.cli_running && has_desktop_upgrades(&plan) {
        return pre_mutation_failure(AlignmentError::CliRunning, codex_was_running, None);
    }
    if !plan.warnings.is_empty() {
        return pre_mutation_failure(AlignmentError::BlockedMetadata, codex_was_running, None);
    }

    let (backup_dir, manifest) = match create_recovery(paths, &plan) {
        Ok(recovery) => recovery,
        Err(error) => return pre_mutation_failure(error, codex_was_running, None),
    };
    let mutation_state = match process.process_state() {
        Ok(state) => state,
        Err(error) => {
            return pre_mutation_failure(error, codex_was_running, Some((&backup_dir, &manifest)))
        }
    };
    if mutation_state.desktop_running || (mutation_state.cli_running && has_desktop_upgrades(&plan))
    {
        return pre_mutation_failure(
            if mutation_state.cli_running {
                AlignmentError::CliRunning
            } else {
                AlignmentError::Mutation
            },
            codex_was_running,
            Some((&backup_dir, &manifest)),
        );
    }
    let mut progress = AlignmentMutationProgress::default();
    let idle_check = || require_full_rollout_idle(process);
    let mutation =
        apply_alignment_with_idle_check(&plan, &manifest, &mut progress, fault, &idle_check);
    match mutation {
        Ok((threads_updated, rollouts_updated)) => {
            let mut report = recovery_report(
                AlignmentOutcome::Committed,
                &backup_dir,
                &manifest,
                codex_was_running,
            );
            report.threads_updated = threads_updated;
            report.rollouts_updated = rollouts_updated;
            report.write_verified = true;
            report.restart_allowed = restart_allowed(report.outcome, true, false);
            report.undo_allowed = true;
            Ok(report)
        }
        Err(error) => {
            let no_mutation =
                progress.database_paths.is_empty() && progress.rollout_paths.is_empty();
            let rollback = if no_mutation {
                Ok(())
            } else {
                process
                    .process_state()
                    .ok()
                    .filter(|state| {
                        !state.desktop_running
                            && (!state.cli_running
                                || !manifest.rollout_upgrades.iter().any(|upgrade| {
                                    progress.rollout_paths.contains(&upgrade.relative_path)
                                }))
                    })
                    .map_or(Err(AlignmentError::Mutation), |_| {
                        restore_after_failed_alignment(
                            &plan,
                            &backup_dir,
                            &manifest,
                            &progress,
                            fault,
                            &idle_check,
                        )
                    })
            };
            let rollback_verified = rollback.is_ok();
            let rollback_permission_denied =
                matches!(rollback, Err(AlignmentError::PermissionDenied));
            let outcome = if rollback_verified {
                AlignmentOutcome::Restored
            } else {
                AlignmentOutcome::RecoveryRequired
            };
            let mut report = recovery_report(outcome, &backup_dir, &manifest, codex_was_running);
            report.rollback_verified = rollback_verified;
            report.elevation_required = matches!(error, AlignmentError::PermissionDenied)
                && (no_mutation
                    || rollback_verified
                    || (rollback_permission_denied
                        && load_recovery(paths, &plan.home, &manifest.recovery_id).is_ok()));
            report.restart_allowed =
                restart_allowed(report.outcome, false, rollback_verified) && codex_was_running;
            report.error_code = Some(error.code().to_string());
            Ok(report)
        }
    }
}

fn pre_mutation_failure(
    error: AlignmentError,
    codex_was_running: bool,
    recovery: Option<(&Path, &RecoveryManifest)>,
) -> Result<AlignmentReport, AlignmentError> {
    let elevation_required = matches!(error, AlignmentError::PermissionDenied);
    if !codex_was_running && !elevation_required {
        return Err(error);
    }
    let mut report = recovery.map_or_else(
        || simple_report(AlignmentOutcome::StalePlan, true),
        |(backup_dir, manifest)| {
            recovery_report(AlignmentOutcome::StalePlan, backup_dir, manifest, true)
        },
    );
    report.restart_allowed = codex_was_running;
    report.elevation_required = elevation_required;
    report.error_code = Some(error.code().to_string());
    Ok(report)
}

fn simple_report(outcome: AlignmentOutcome, codex_was_running: bool) -> AlignmentReport {
    AlignmentReport {
        outcome,
        recovery_id: None,
        backup_dir: None,
        threads_updated: 0,
        rollouts_updated: 0,
        rollouts_skipped: 0,
        write_verified: false,
        rollback_verified: false,
        codex_was_running,
        restart_allowed: false,
        undo_allowed: false,
        elevation_required: false,
        error_code: None,
    }
}

fn recovery_report(
    outcome: AlignmentOutcome,
    backup_dir: &Path,
    manifest: &RecoveryManifest,
    codex_was_running: bool,
) -> AlignmentReport {
    let mut report = simple_report(outcome, codex_was_running);
    report.recovery_id = Some(manifest.recovery_id.clone());
    report.backup_dir = Some(backup_dir.to_string_lossy().into_owned());
    report.rollouts_skipped = manifest.rollouts_skipped;
    report
}

fn plan_change_count(plan: &AlignmentPlan) -> u64 {
    plan.databases
        .iter()
        .map(|database| database.mismatched_threads)
        .sum::<u64>()
        + plan
            .rollouts
            .iter()
            .filter(|rollout| {
                rollout.class == RolloutClass::Repairable || rollout.needs_desktop_upgrade
            })
            .count() as u64
}

fn has_desktop_upgrades(plan: &AlignmentPlan) -> bool {
    plan.rollouts
        .iter()
        .any(|rollout| rollout.needs_desktop_upgrade)
}

fn require_full_rollout_idle(process: &dyn ProcessGate) -> Result<(), AlignmentError> {
    let state = process.process_state()?;
    if state.cli_running {
        Err(AlignmentError::CliRunning)
    } else if state.desktop_running {
        Err(AlignmentError::Mutation)
    } else {
        Ok(())
    }
}

fn repairable_rollouts(plan: &AlignmentPlan) -> impl Iterator<Item = &RolloutImpact> {
    plan.rollouts
        .iter()
        .filter(|rollout| rollout.class == RolloutClass::Repairable)
}

fn skipped_rollout_count(plan: &AlignmentPlan) -> usize {
    plan.rollouts
        .iter()
        .filter(|rollout| {
            matches!(rollout.class, RolloutClass::Skipped(_))
                || rollout.upgrade_skip_reason.is_some()
        })
        .count()
}

fn create_recovery(
    paths: &AlignmentPaths,
    plan: &AlignmentPlan,
) -> Result<(PathBuf, RecoveryManifest), AlignmentError> {
    let backup_root = prepare_backup_root(&plan.home, &paths.backup_root)?;
    let recovery_id = format!("provider-align-{}-{}", unix_millis(), uuid::Uuid::new_v4());
    let backup_dir = backup_root.join(&recovery_id);
    fs::create_dir(&backup_dir).map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    set_private_directory(&backup_dir)
        .map_err(|error| io_error_or(error, AlignmentError::Backup))?;

    let mut databases = Vec::with_capacity(plan.databases.len());
    for (index, database) in plan.databases.iter().enumerate() {
        let backup_file = format!("database-{}.sqlite", index + 1);
        let backup_path = backup_dir.join(&backup_file);
        backup_database(&database.path, &backup_path)?;
        databases.push(DatabaseBackup {
            relative_path: relative_artifact_path(&plan.home, &database.path)?,
            backup_file,
            thread_count: database.thread_count,
            provider_counts: database.provider_counts.clone(),
            original_rows: read_mismatched_thread_rows(&database.path, &plan.provider)?,
        });
    }

    let mut rollouts = Vec::new();
    let mut recovery_artifact_bytes = 0_u64;
    for (index, rollout) in repairable_rollouts(plan)
        .filter(|rollout| !rollout.needs_desktop_upgrade)
        .enumerate()
    {
        let aligned_head = aligned_rollout_head(&rollout.raw_head, &plan.provider)?;
        if rollout.raw_head.len() as u64 > MAX_ROLLOUT_HEAD_BYTES
            || aligned_head.len() as u64 > MAX_ROLLOUT_HEAD_BYTES
        {
            return Err(AlignmentError::Backup);
        }
        recovery_artifact_bytes = recovery_artifact_bytes
            .checked_add(rollout.raw_head.len() as u64)
            .and_then(|total| total.checked_add(aligned_head.len() as u64))
            .filter(|total| *total <= MAX_RECOVERY_ARTIFACT_BYTES)
            .ok_or(AlignmentError::StaleRecovery)?;
        let original_head_file = format!("rollout-{}-original-head.bin", index + 1);
        let aligned_head_file = format!("rollout-{}-aligned-head.bin", index + 1);
        write_private_new(&backup_dir.join(&original_head_file), &rollout.raw_head)
            .map_err(|error| io_error_or(error, AlignmentError::Backup))?;
        write_private_new(&backup_dir.join(&aligned_head_file), &aligned_head)
            .map_err(|error| io_error_or(error, AlignmentError::Backup))?;
        rollouts.push(RolloutBackup {
            relative_path: relative_artifact_path(&plan.home, &rollout.path)?,
            original_head_file,
            aligned_head_file,
            original_head: rollout.raw_head.clone(),
            original_head_sha256: rollout.head_sha256.clone(),
            aligned_head_sha256: sha256_hex(&aligned_head),
            aligned_head,
            original_tail_sha256: tail_sha256(&rollout.path, rollout.raw_head.len() as u64)?,
        });
    }

    let mut rollout_upgrades = Vec::new();
    for (index, rollout) in plan
        .rollouts
        .iter()
        .filter(|rollout| rollout.needs_desktop_upgrade)
        .enumerate()
    {
        let id = rollout
            .recovery_id
            .as_deref()
            .ok_or(AlignmentError::BlockedMetadata)?;
        let prepared = crate::app::history_recovery::prepare_generated_recovery_upgrade(
            &rollout.path,
            id,
            Some(&plan.provider),
        )?
        .ok_or(AlignmentError::StaleRecovery)?;
        let prepared_artifact_bytes = (prepared.original.len() as u64)
            .checked_add(prepared.upgraded.len() as u64)
            .ok_or(AlignmentError::StaleRecovery)?;
        if prepared.original.len() as u64 > MAX_RECOVERY_ROLLOUT_BYTES
            || prepared.upgraded.len() as u64 > MAX_RECOVERY_ROLLOUT_BYTES
            || prepared_artifact_bytes != rollout.recovery_artifact_bytes
            || rollout.recovery_file_sha256.as_deref()
                != Some(sha256_hex(&prepared.original).as_str())
        {
            return Err(AlignmentError::StaleRecovery);
        }
        recovery_artifact_bytes = recovery_artifact_bytes
            .checked_add(prepared_artifact_bytes)
            .filter(|total| *total <= MAX_RECOVERY_ARTIFACT_BYTES)
            .ok_or(AlignmentError::StaleRecovery)?;
        let original_file = format!("rollout-upgrade-{}-original.jsonl", index + 1);
        let upgraded_file = format!("rollout-upgrade-{}-desktop.jsonl", index + 1);
        write_private_new(&backup_dir.join(&original_file), &prepared.original)
            .map_err(|error| io_error_or(error, AlignmentError::Backup))?;
        write_private_new(&backup_dir.join(&upgraded_file), &prepared.upgraded)
            .map_err(|error| io_error_or(error, AlignmentError::Backup))?;
        rollout_upgrades.push(RolloutUpgradeBackup {
            relative_path: relative_artifact_path(&plan.home, &rollout.path)?,
            original_file,
            upgraded_file,
            original_sha256: sha256_hex(&prepared.original),
            upgraded_sha256: sha256_hex(&prepared.upgraded),
            original: Vec::new(),
            upgraded: Vec::new(),
        });
    }

    let manifest = RecoveryManifest {
        version: RECOVERY_MANIFEST_VERSION,
        recovery_id,
        provider: plan.provider.clone(),
        rollouts_skipped: skipped_rollout_count(plan),
        databases,
        rollouts,
        rollout_upgrades,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|_| AlignmentError::Backup)?;
    write_private_new(&backup_dir.join("manifest.json"), &manifest_bytes)
        .map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    sync_dir(&backup_dir).map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    sync_dir(&backup_root).map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    load_recovery(paths, &plan.home, &manifest.recovery_id).map_err(|_| AlignmentError::Backup)
}

fn prepare_backup_root(home: &Path, requested: &Path) -> Result<PathBuf, AlignmentError> {
    if let Ok(metadata) = fs::symlink_metadata(requested) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AlignmentError::UnsafePath);
        }
    }
    fs::create_dir_all(requested).map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    set_private_directory(requested).map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    let canonical = requested
        .canonicalize()
        .map_err(|error| io_error_or(error, AlignmentError::Backup))?;
    if canonical == home || !canonical.starts_with(home) {
        return Err(AlignmentError::UnsafePath);
    }
    Ok(canonical)
}

fn relative_artifact_path(home: &Path, artifact: &Path) -> Result<String, AlignmentError> {
    let relative = artifact
        .strip_prefix(home)
        .map_err(|_| AlignmentError::UnsafePath)?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(AlignmentError::UnsafePath);
        };
        parts.push(part.to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        return Err(AlignmentError::UnsafePath);
    }
    Ok(parts.join("/"))
}

fn artifact_from_relative(home: &Path, relative: &str) -> Result<PathBuf, AlignmentError> {
    if !is_safe_relative_path(relative) {
        return Err(AlignmentError::InvalidRecovery);
    }
    safe_canonical_path(&home.join(relative), home).map_err(|_| AlignmentError::InvalidRecovery)
}

fn is_safe_relative_path(relative: &str) -> bool {
    let path = Path::new(relative);
    !relative.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn backup_database(source_path: &Path, backup_path: &Path) -> Result<(), AlignmentError> {
    drop(
        create_private_file(backup_path)
            .map_err(|error| io_error_or(error, AlignmentError::Backup))?,
    );
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Backup))?;
    source
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Backup))?;
    let mut destination = Connection::open(backup_path)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Backup))?;
    let backup = Backup::new(&source, &mut destination)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Backup))?;
    backup
        .run_to_completion(100, Duration::from_millis(10), None)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Backup))?;
    drop(backup);
    drop(destination);
    drop(source);
    verify_quick_check(backup_path).map_err(|_| AlignmentError::Backup)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(backup_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_error_or(error, AlignmentError::Backup))
}

fn read_mismatched_thread_rows(
    path: &Path,
    provider: &str,
) -> Result<Vec<ThreadProviderPreimage>, AlignmentError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    connection
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    let id_column = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('threads') WHERE name = 'id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    if id_column != 1 {
        return Err(AlignmentError::UnsupportedSchema);
    }
    let mut statement = connection
        .prepare(
            "SELECT id, model_provider
             FROM threads
             WHERE model_provider IS NULL OR model_provider <> ?1
             ORDER BY id",
        )
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    let rows = statement
        .query_map(params![provider], |row| {
            Ok(ThreadProviderPreimage {
                id: row.get(0)?,
                provider: row.get(1)?,
            })
        })
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?
        .map(|row| row.map_err(|error| sqlite_error_or(error, AlignmentError::Database)))
        .collect();
    rows
}

fn aligned_rollout_head(raw_head: &[u8], provider: &str) -> Result<Vec<u8>, AlignmentError> {
    let has_bom = raw_head.starts_with(b"\xef\xbb\xbf");
    let ending: &[u8] = if raw_head.ends_with(b"\r\n") {
        b"\r\n"
    } else if raw_head.ends_with(b"\n") {
        b"\n"
    } else {
        b""
    };
    let mut value = serde_json::from_slice::<serde_json::Value>(rollout_json_bytes(raw_head))
        .map_err(|_| AlignmentError::BlockedMetadata)?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
        return Err(AlignmentError::BlockedMetadata);
    }
    let payload = value
        .get_mut("payload")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or(AlignmentError::BlockedMetadata)?;
    match payload.get("model_provider") {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => {}
        Some(_) => return Err(AlignmentError::BlockedMetadata),
    }
    payload.insert(
        "model_provider".to_string(),
        serde_json::Value::String(provider.to_string()),
    );
    let json = serde_json::to_vec(&value).map_err(|_| AlignmentError::Mutation)?;
    let mut aligned = Vec::with_capacity(json.len() + ending.len() + usize::from(has_bom) * 3);
    if has_bom {
        aligned.extend_from_slice(b"\xef\xbb\xbf");
    }
    aligned.extend_from_slice(&json);
    aligned.extend_from_slice(ending);
    Ok(aligned)
}

fn tail_sha256(path: &Path, head_len: u64) -> Result<String, AlignmentError> {
    let mut file = File::open(path).map_err(|error| io_error_or(error, AlignmentError::Io))?;
    if file
        .metadata()
        .map_err(|error| io_error_or(error, AlignmentError::Io))?
        .len()
        < head_len
    {
        return Err(AlignmentError::Io);
    }
    file.seek(SeekFrom::Start(head_len))
        .map_err(|error| io_error_or(error, AlignmentError::Io))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(hex_digest(&digest))
}

#[cfg(test)]
fn apply_alignment(
    plan: &AlignmentPlan,
    manifest: &RecoveryManifest,
    progress: &mut AlignmentMutationProgress,
    fault: AlignmentFault,
) -> Result<(u64, usize), AlignmentError> {
    apply_alignment_with_idle_check(plan, manifest, progress, fault, &|| Ok(()))
}

fn apply_alignment_with_idle_check(
    plan: &AlignmentPlan,
    manifest: &RecoveryManifest,
    progress: &mut AlignmentMutationProgress,
    fault: AlignmentFault,
    idle_check: &dyn Fn() -> Result<(), AlignmentError>,
) -> Result<(u64, usize), AlignmentError> {
    if fault == AlignmentFault::PermissionBeforeWrite {
        return Err(AlignmentError::PermissionDenied);
    }
    let mut threads_updated = 0_u64;
    for database in &manifest.databases {
        let path = artifact_from_relative(&plan.home, &database.relative_path)?;
        let mut connection = Connection::open(path)
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        connection
            .busy_timeout(DATABASE_BUSY_TIMEOUT)
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        let transaction = connection
            .transaction()
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        for row in &database.original_rows {
            let changed = transaction
                .execute(
                    "UPDATE threads
                     SET model_provider = ?1
                     WHERE id = ?2 AND model_provider IS ?3",
                    params![plan.provider, row.id, row.provider],
                )
                .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
            if changed != 1 {
                return Err(AlignmentError::StaleRecovery);
            }
            threads_updated = threads_updated
                .checked_add(changed as u64)
                .ok_or(AlignmentError::Mutation)?;
        }
        if !database.original_rows.is_empty() {
            progress
                .database_paths
                .insert(database.relative_path.clone());
        }
        transaction
            .commit()
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        if fault == AlignmentFault::AfterDatabaseCommit {
            return Err(AlignmentError::Mutation);
        }
        if matches!(
            fault,
            AlignmentFault::PermissionAfterDatabaseCommit
                | AlignmentFault::PermissionDuringRollback
        ) {
            return Err(AlignmentError::PermissionDenied);
        }
    }

    if fault == AlignmentFault::BeforeRolloutWrite
        || fault == AlignmentFault::RollbackVerificationFailure
    {
        return Err(AlignmentError::Mutation);
    }
    if fault == AlignmentFault::PermissionBeforeRolloutWrite {
        return Err(AlignmentError::PermissionDenied);
    }

    let mut updated_rollout_paths = BTreeSet::new();
    for rollout in &manifest.rollouts {
        let path = artifact_from_relative(&plan.home, &rollout.relative_path)?;
        let mut replacement_landed = false;
        let replacement = replace_rollout_head_tracked(
            &path,
            &rollout.original_head_sha256,
            Some(&rollout.original_tail_sha256),
            &rollout.aligned_head,
            &mut replacement_landed,
        );
        if replacement_landed {
            progress.rollout_paths.insert(rollout.relative_path.clone());
        }
        replacement?;
        if fault == AlignmentFault::AfterRolloutWrite {
            return Err(AlignmentError::Mutation);
        }
        updated_rollout_paths.insert(rollout.relative_path.clone());
    }
    for upgrade in &manifest.rollout_upgrades {
        let path = artifact_from_relative(&plan.home, &upgrade.relative_path)?;
        let mut replacement_landed = false;
        let replacement = crate::app::history_recovery::replace_recovery_rollout(
            &path,
            &upgrade.original_sha256,
            &upgrade.upgraded,
            true,
            &mut replacement_landed,
            idle_check,
        );
        if replacement_landed {
            progress.rollout_paths.insert(upgrade.relative_path.clone());
        }
        replacement?;
        if fault == AlignmentFault::AfterRolloutWrite {
            return Err(AlignmentError::Mutation);
        }
        updated_rollout_paths.insert(upgrade.relative_path.clone());
    }
    verify_aligned_state(plan, manifest)?;
    Ok((threads_updated, updated_rollout_paths.len()))
}

fn verify_aligned_state(
    plan: &AlignmentPlan,
    manifest: &RecoveryManifest,
) -> Result<(), AlignmentError> {
    for expected in &plan.databases {
        let actual = scan_database(&expected.path, &plan.provider)?;
        if actual.thread_count != expected.thread_count || actual.mismatched_threads != 0 {
            return Err(AlignmentError::Mutation);
        }
        verify_quick_check(&expected.path)?;
    }
    for rollout in &manifest.rollouts {
        let path = artifact_from_relative(&plan.home, &rollout.relative_path)?;
        let current_head = read_raw_rollout_head(&path)?;
        if sha256_hex(&current_head) != rollout.aligned_head_sha256 {
            return Err(AlignmentError::Mutation);
        }
        let current = scan_rollout(&path, &plan.provider)?;
        if current.provider.as_deref() != Some(plan.provider.as_str())
            || tail_sha256(&path, current_head.len() as u64)? != rollout.original_tail_sha256
        {
            return Err(AlignmentError::Mutation);
        }
    }
    for upgrade in &manifest.rollout_upgrades {
        let path = artifact_from_relative(&plan.home, &upgrade.relative_path)?;
        if !crate::app::history_recovery::recovery_rollout_starts_with(&path, &upgrade.upgraded)? {
            return Err(AlignmentError::Mutation);
        }
        let current = scan_rollout(&path, &plan.provider)?;
        if current.provider.as_deref() != Some(plan.provider.as_str())
            || current.needs_desktop_upgrade
        {
            return Err(AlignmentError::Mutation);
        }
    }
    Ok(())
}

fn restore_after_failed_alignment(
    plan: &AlignmentPlan,
    _backup_dir: &Path,
    manifest: &RecoveryManifest,
    progress: &AlignmentMutationProgress,
    fault: AlignmentFault,
    idle_check: &dyn Fn() -> Result<(), AlignmentError>,
) -> Result<(), AlignmentError> {
    if fault == AlignmentFault::PermissionDuringRollback {
        return Err(AlignmentError::PermissionDenied);
    }
    apply_selective_restore(&plan.home, manifest, Some(progress), idle_check)?;
    if fault == AlignmentFault::RollbackVerificationFailure {
        Err(AlignmentError::Mutation)
    } else {
        Ok(())
    }
}

fn verify_quick_check(path: &Path) -> Result<(), AlignmentError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| AlignmentError::Database)?;
    connection
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|_| AlignmentError::Database)?;
    let result = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| AlignmentError::Database)?;
    if result == "ok" {
        Ok(())
    } else {
        Err(AlignmentError::Database)
    }
}

fn read_raw_rollout_head(path: &Path) -> Result<Vec<u8>, AlignmentError> {
    let file = File::open(path).map_err(|error| io_error_or(error, AlignmentError::Io))?;
    read_rollout_head_from_reader(&mut BufReader::new(file))
}

fn read_rollout_head_from_reader<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, AlignmentError> {
    let mut head = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        if available.is_empty() {
            return Ok(head);
        }
        if let Some(index) = available.iter().position(|byte| *byte == b'\n') {
            let consumed = index + 1;
            if head.len() + consumed > MAX_ROLLOUT_HEAD_BYTES as usize {
                return Err(AlignmentError::BlockedMetadata);
            }
            head.extend_from_slice(&available[..consumed]);
            reader.consume(consumed);
            return Ok(head);
        }
        let consumed = available.len();
        if head.len() + consumed > MAX_ROLLOUT_HEAD_BYTES as usize {
            return Err(AlignmentError::BlockedMetadata);
        }
        head.extend_from_slice(available);
        reader.consume(consumed);
    }
}

fn replace_rollout_head(
    path: &Path,
    expected_head_sha256: &str,
    replacement_head: &[u8],
) -> Result<(), AlignmentError> {
    replace_rollout_head_tracked(
        path,
        expected_head_sha256,
        None,
        replacement_head,
        &mut false,
    )
}

fn replace_rollout_head_tracked(
    path: &Path,
    expected_head_sha256: &str,
    expected_tail_sha256: Option<&str>,
    replacement_head: &[u8],
    replacement_landed: &mut bool,
) -> Result<(), AlignmentError> {
    *replacement_landed = false;
    let current_head = read_raw_rollout_head(path)?;
    if sha256_hex(&current_head) != expected_head_sha256 {
        return Err(AlignmentError::StaleRecovery);
    }
    if let Some(expected) = expected_tail_sha256 {
        if tail_sha256(path, current_head.len() as u64)? != expected {
            return Err(AlignmentError::StaleRecovery);
        }
    }
    let parent = path.parent().ok_or(AlignmentError::UnsafePath)?;
    let name = path
        .file_name()
        .ok_or(AlignmentError::UnsafePath)?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.{}.alignment.tmp", uuid::Uuid::new_v4()));
    let permissions = path
        .metadata()
        .map_err(|error| io_error_or(error, AlignmentError::Io))?
        .permissions();
    let result = (|| {
        let mut source =
            File::open(path).map_err(|error| io_error_or(error, AlignmentError::Io))?;
        source
            .seek(SeekFrom::Start(current_head.len() as u64))
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        let mut destination = create_replacement_file(path, &temp)
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        destination
            .write_all(replacement_head)
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        std::io::copy(&mut source, &mut destination)
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        destination
            .flush()
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        destination
            .sync_all()
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        drop(source);
        drop(destination);
        fs::set_permissions(&temp, permissions)
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        replace_file(&temp, path).map_err(|error| io_error_or(error, AlignmentError::Io))?;
        *replacement_landed = true;
        sync_dir(parent).map_err(|error| io_error_or(error, AlignmentError::Io))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
fn restore_alignment_at(
    paths: &AlignmentPaths,
    recovery_id: &str,
) -> Result<AlignmentReport, AlignmentError> {
    let home = canonical_directory(&paths.codex_home)?;
    let (backup_dir, manifest) = load_recovery(paths, &home, recovery_id)?;
    validate_undo_preconditions(&home, &manifest)?;
    Ok(restore_loaded_alignment(
        &home,
        &backup_dir,
        &manifest,
        false,
        &|| Ok(()),
    ))
}

pub fn restore_alignment_with_process_at(
    paths: &AlignmentPaths,
    recovery_id: &str,
    process: &dyn ProcessGate,
) -> Result<AlignmentReport, AlignmentError> {
    let home = canonical_directory(&paths.codex_home)?;
    let (backup_dir, manifest) = load_recovery(paths, &home, recovery_id)?;
    validate_undo_preconditions(&home, &manifest)?;

    let initial_state = process.process_state()?;
    let codex_was_running = initial_state.desktop_running;
    if initial_state.cli_running && !manifest.rollout_upgrades.is_empty() {
        return Err(AlignmentError::CliRunning);
    }
    if codex_was_running && !process.stop_desktop()? {
        return Err(AlignmentError::Mutation);
    }
    let settled_state = match process.process_state() {
        Ok(state) => state,
        Err(error) => {
            return pre_mutation_failure(error, codex_was_running, Some((&backup_dir, &manifest)))
        }
    };
    if settled_state.desktop_running
        || (settled_state.cli_running && !manifest.rollout_upgrades.is_empty())
    {
        return pre_mutation_failure(
            if settled_state.cli_running {
                AlignmentError::CliRunning
            } else {
                AlignmentError::Mutation
            },
            codex_was_running,
            Some((&backup_dir, &manifest)),
        );
    }
    if let Err(error) = validate_undo_preconditions(&home, &manifest) {
        return pre_mutation_failure(error, codex_was_running, Some((&backup_dir, &manifest)));
    }
    let mutation_state = match process.process_state() {
        Ok(state) => state,
        Err(error) => {
            return pre_mutation_failure(error, codex_was_running, Some((&backup_dir, &manifest)))
        }
    };
    if mutation_state.desktop_running
        || (mutation_state.cli_running && !manifest.rollout_upgrades.is_empty())
    {
        return pre_mutation_failure(
            if mutation_state.cli_running {
                AlignmentError::CliRunning
            } else {
                AlignmentError::Mutation
            },
            codex_was_running,
            Some((&backup_dir, &manifest)),
        );
    }

    Ok(restore_loaded_alignment(
        &home,
        &backup_dir,
        &manifest,
        codex_was_running,
        &|| require_full_rollout_idle(process),
    ))
}

fn restore_loaded_alignment(
    home: &Path,
    backup_dir: &Path,
    manifest: &RecoveryManifest,
    codex_was_running: bool,
    idle_check: &dyn Fn() -> Result<(), AlignmentError>,
) -> AlignmentReport {
    let undo = apply_selective_undo(&home, &manifest, idle_check);
    match undo {
        Ok((threads_updated, rollouts_updated)) => {
            let mut report = recovery_report(
                AlignmentOutcome::Restored,
                backup_dir,
                manifest,
                codex_was_running,
            );
            report.threads_updated = threads_updated;
            report.rollouts_updated = rollouts_updated;
            report.rollback_verified = true;
            report.restart_allowed =
                restart_allowed(report.outcome, false, true) && codex_was_running;
            report
        }
        Err(error) => {
            let mut report = recovery_report(
                AlignmentOutcome::RecoveryRequired,
                backup_dir,
                manifest,
                codex_was_running,
            );
            report.error_code = Some(error.code().to_string());
            report
        }
    }
}

pub fn recovery_directory_at(
    paths: &AlignmentPaths,
    recovery_id: &str,
) -> Result<PathBuf, AlignmentError> {
    let home = canonical_directory(&paths.codex_home)?;
    load_recovery(paths, &home, recovery_id).map(|(backup_dir, _)| backup_dir)
}

fn load_recovery(
    paths: &AlignmentPaths,
    home: &Path,
    recovery_id: &str,
) -> Result<(PathBuf, RecoveryManifest), AlignmentError> {
    load_recovery_with_artifact_limit(paths, home, recovery_id, MAX_RECOVERY_ARTIFACT_BYTES)
}

fn load_recovery_with_artifact_limit(
    paths: &AlignmentPaths,
    home: &Path,
    recovery_id: &str,
    artifact_limit: u64,
) -> Result<(PathBuf, RecoveryManifest), AlignmentError> {
    if recovery_id.is_empty()
        || recovery_id.len() > 128
        || !recovery_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(AlignmentError::InvalidRecovery);
    }
    let backup_root = paths
        .backup_root
        .canonicalize()
        .map_err(|_| AlignmentError::InvalidRecovery)?;
    if backup_root == home || !backup_root.starts_with(home) {
        return Err(AlignmentError::InvalidRecovery);
    }
    let backup_dir = backup_root
        .join(recovery_id)
        .canonicalize()
        .map_err(|_| AlignmentError::InvalidRecovery)?;
    if backup_dir.parent() != Some(backup_root.as_path()) || !backup_dir.is_dir() {
        return Err(AlignmentError::InvalidRecovery);
    }
    let manifest_path = backup_dir.join("manifest.json");
    let metadata = manifest_path
        .symlink_metadata()
        .map_err(|_| AlignmentError::InvalidRecovery)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_RECOVERY_MANIFEST_BYTES
    {
        return Err(AlignmentError::InvalidRecovery);
    }
    let manifest_bytes = fs::read(&manifest_path).map_err(|_| AlignmentError::InvalidRecovery)?;
    let manifest_value: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).map_err(|_| AlignmentError::InvalidRecovery)?;
    let has_rollout_upgrades = manifest_value.get("rolloutUpgrades").is_some();
    let mut manifest: RecoveryManifest =
        serde_json::from_value(manifest_value).map_err(|_| AlignmentError::InvalidRecovery)?;
    let supported_version = manifest.version == RECOVERY_MANIFEST_VERSION
        || (manifest.version == LEGACY_RECOVERY_MANIFEST_VERSION && !has_rollout_upgrades);
    if !supported_version || manifest.recovery_id != recovery_id {
        return Err(AlignmentError::InvalidRecovery);
    }
    #[cfg(not(target_os = "windows"))]
    if !manifest.rollout_upgrades.is_empty() {
        return Err(AlignmentError::InvalidRecovery);
    }
    let mut recovery_files = BTreeSet::new();
    let mut rollout_paths = BTreeSet::new();
    let mut artifact_bytes = 0_u64;
    for database in &manifest.databases {
        if !is_safe_relative_path(&database.relative_path)
            || !recovery_files.insert(database.backup_file.clone())
        {
            return Err(AlignmentError::InvalidRecovery);
        }
        let backup_path = recovery_file_path(&backup_dir, &database.backup_file)?;
        verify_quick_check(&backup_path).map_err(|_| AlignmentError::InvalidRecovery)?;
    }
    for rollout in &mut manifest.rollouts {
        if !is_safe_relative_path(&rollout.relative_path)
            || !rollout_paths.insert(rollout.relative_path.clone())
            || !is_sha256_hex(&rollout.original_head_sha256)
            || !is_sha256_hex(&rollout.aligned_head_sha256)
            || !is_sha256_hex(&rollout.original_tail_sha256)
            || !recovery_files.insert(rollout.original_head_file.clone())
            || !recovery_files.insert(rollout.aligned_head_file.clone())
        {
            return Err(AlignmentError::InvalidRecovery);
        }
        artifact_bytes = checked_recovery_artifact_bytes(
            &backup_dir,
            &rollout.original_head_file,
            &rollout.original_head_sha256,
            MAX_ROLLOUT_HEAD_BYTES,
            artifact_bytes,
            artifact_limit,
        )?;
        artifact_bytes = checked_recovery_artifact_bytes(
            &backup_dir,
            &rollout.aligned_head_file,
            &rollout.aligned_head_sha256,
            MAX_ROLLOUT_HEAD_BYTES,
            artifact_bytes,
            artifact_limit,
        )?;
    }
    for upgrade in &mut manifest.rollout_upgrades {
        if !is_safe_relative_path(&upgrade.relative_path)
            || !rollout_paths.insert(upgrade.relative_path.clone())
            || !is_sha256_hex(&upgrade.original_sha256)
            || !is_sha256_hex(&upgrade.upgraded_sha256)
            || !recovery_files.insert(upgrade.original_file.clone())
            || !recovery_files.insert(upgrade.upgraded_file.clone())
        {
            return Err(AlignmentError::InvalidRecovery);
        }
        artifact_bytes = checked_recovery_artifact_bytes(
            &backup_dir,
            &upgrade.original_file,
            &upgrade.original_sha256,
            MAX_RECOVERY_ROLLOUT_BYTES,
            artifact_bytes,
            artifact_limit,
        )?;
        artifact_bytes = checked_recovery_artifact_bytes(
            &backup_dir,
            &upgrade.upgraded_file,
            &upgrade.upgraded_sha256,
            MAX_RECOVERY_ROLLOUT_BYTES,
            artifact_bytes,
            artifact_limit,
        )?;
    }

    let mut loaded_bytes = 0_u64;
    for rollout in &mut manifest.rollouts {
        rollout.original_head = read_recovery_bytes(
            &backup_dir,
            &rollout.original_head_file,
            &rollout.original_head_sha256,
            remaining_artifact_bytes(artifact_limit, loaded_bytes, MAX_ROLLOUT_HEAD_BYTES)?,
        )?;
        loaded_bytes = loaded_bytes
            .checked_add(rollout.original_head.len() as u64)
            .ok_or(AlignmentError::InvalidRecovery)?;
        rollout.aligned_head = read_recovery_bytes(
            &backup_dir,
            &rollout.aligned_head_file,
            &rollout.aligned_head_sha256,
            remaining_artifact_bytes(artifact_limit, loaded_bytes, MAX_ROLLOUT_HEAD_BYTES)?,
        )?;
        loaded_bytes = loaded_bytes
            .checked_add(rollout.aligned_head.len() as u64)
            .ok_or(AlignmentError::InvalidRecovery)?;
    }
    for upgrade in &mut manifest.rollout_upgrades {
        upgrade.original = read_recovery_bytes(
            &backup_dir,
            &upgrade.original_file,
            &upgrade.original_sha256,
            remaining_artifact_bytes(artifact_limit, loaded_bytes, MAX_RECOVERY_ROLLOUT_BYTES)?,
        )?;
        loaded_bytes = loaded_bytes
            .checked_add(upgrade.original.len() as u64)
            .ok_or(AlignmentError::InvalidRecovery)?;
        upgrade.upgraded = read_recovery_bytes(
            &backup_dir,
            &upgrade.upgraded_file,
            &upgrade.upgraded_sha256,
            remaining_artifact_bytes(artifact_limit, loaded_bytes, MAX_RECOVERY_ROLLOUT_BYTES)?,
        )?;
        loaded_bytes = loaded_bytes
            .checked_add(upgrade.upgraded.len() as u64)
            .ok_or(AlignmentError::InvalidRecovery)?;
    }
    debug_assert_eq!(loaded_bytes, artifact_bytes);
    Ok((backup_dir, manifest))
}

fn checked_recovery_artifact_bytes(
    backup_dir: &Path,
    filename: &str,
    expected_sha256: &str,
    file_limit: u64,
    accumulated: u64,
    artifact_limit: u64,
) -> Result<u64, AlignmentError> {
    if !is_sha256_hex(expected_sha256) {
        return Err(AlignmentError::InvalidRecovery);
    }
    let bytes = recovery_file_path(backup_dir, filename)?
        .metadata()
        .map_err(|_| AlignmentError::InvalidRecovery)?
        .len();
    if bytes > file_limit {
        return Err(AlignmentError::InvalidRecovery);
    }
    accumulated
        .checked_add(bytes)
        .filter(|total| *total <= artifact_limit)
        .ok_or(AlignmentError::InvalidRecovery)
}

fn remaining_artifact_bytes(
    artifact_limit: u64,
    loaded_bytes: u64,
    file_limit: u64,
) -> Result<u64, AlignmentError> {
    artifact_limit
        .checked_sub(loaded_bytes)
        .map(|remaining| remaining.min(file_limit))
        .ok_or(AlignmentError::InvalidRecovery)
}

fn recovery_file_path(backup_dir: &Path, filename: &str) -> Result<PathBuf, AlignmentError> {
    let path = Path::new(filename);
    let mut components = path.components();
    if filename.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(AlignmentError::InvalidRecovery);
    }
    let path = backup_dir.join(path);
    let metadata = path
        .symlink_metadata()
        .map_err(|_| AlignmentError::InvalidRecovery)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AlignmentError::InvalidRecovery);
    }
    Ok(path)
}

fn read_recovery_bytes(
    backup_dir: &Path,
    filename: &str,
    expected_sha256: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, AlignmentError> {
    if !is_sha256_hex(expected_sha256) {
        return Err(AlignmentError::InvalidRecovery);
    }
    let path = recovery_file_path(backup_dir, filename)?;
    if path
        .metadata()
        .map_err(|_| AlignmentError::InvalidRecovery)?
        .len()
        > max_bytes
    {
        return Err(AlignmentError::InvalidRecovery);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| AlignmentError::InvalidRecovery)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AlignmentError::InvalidRecovery)?;
    if bytes.len() as u64 > max_bytes || sha256_hex(&bytes) != expected_sha256 {
        return Err(AlignmentError::InvalidRecovery);
    }
    Ok(bytes)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_undo_preconditions(
    home: &Path,
    manifest: &RecoveryManifest,
) -> Result<(), AlignmentError> {
    for database in &manifest.databases {
        let path = artifact_from_relative(home, &database.relative_path)?;
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| sqlite_error_or(error, AlignmentError::StaleRecovery))?;
        for row in &database.original_rows {
            if let Some(current) = thread_provider_by_id(&connection, &row.id)? {
                let committed = Some(manifest.provider.clone());
                if current != committed && current != row.provider {
                    return Err(AlignmentError::StaleRecovery);
                }
            }
        }
    }
    for rollout in &manifest.rollouts {
        let path = artifact_from_relative(home, &rollout.relative_path)?;
        let head_hash = sha256_hex(&read_raw_rollout_head(&path)?);
        if head_hash != rollout.aligned_head_sha256 && head_hash != rollout.original_head_sha256 {
            return Err(AlignmentError::StaleRecovery);
        }
    }
    for upgrade in &manifest.rollout_upgrades {
        let path = artifact_from_relative(home, &upgrade.relative_path)?;
        if !crate::app::history_recovery::recovery_rollout_starts_with(&path, &upgrade.upgraded)?
            && !crate::app::history_recovery::recovery_rollout_starts_with(
                &path,
                &upgrade.original,
            )?
        {
            return Err(AlignmentError::StaleRecovery);
        }
    }
    Ok(())
}

fn apply_selective_undo(
    home: &Path,
    manifest: &RecoveryManifest,
    idle_check: &dyn Fn() -> Result<(), AlignmentError>,
) -> Result<(u64, usize), AlignmentError> {
    apply_selective_restore(home, manifest, None, idle_check)
}

fn apply_selective_restore(
    home: &Path,
    manifest: &RecoveryManifest,
    progress: Option<&AlignmentMutationProgress>,
    idle_check: &dyn Fn() -> Result<(), AlignmentError>,
) -> Result<(u64, usize), AlignmentError> {
    let mut threads_updated = 0_u64;
    for database in &manifest.databases {
        if progress
            .is_some_and(|progress| !progress.database_paths.contains(&database.relative_path))
        {
            continue;
        }
        let path = artifact_from_relative(home, &database.relative_path)?;
        let mut connection = Connection::open(path)
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        connection
            .busy_timeout(DATABASE_BUSY_TIMEOUT)
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        let transaction = connection
            .transaction()
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        for row in &database.original_rows {
            let changed = transaction
                .execute(
                    "UPDATE threads
                     SET model_provider = ?1
                     WHERE id = ?2 AND model_provider = ?3",
                    params![row.provider, row.id, manifest.provider],
                )
                .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
            threads_updated = threads_updated
                .checked_add(changed as u64)
                .ok_or(AlignmentError::Mutation)?;
        }
        transaction
            .commit()
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
    }

    let mut rollouts_updated = 0;
    for upgrade in &manifest.rollout_upgrades {
        if progress.is_some_and(|progress| !progress.rollout_paths.contains(&upgrade.relative_path))
        {
            continue;
        }
        let path = artifact_from_relative(home, &upgrade.relative_path)?;
        if crate::app::history_recovery::recovery_rollout_starts_with(&path, &upgrade.upgraded)? {
            let mut replacement_landed = false;
            crate::app::history_recovery::replace_recovery_rollout_prefix(
                &path,
                &upgrade.upgraded,
                &upgrade.original,
                &mut replacement_landed,
                idle_check,
            )?;
            rollouts_updated += 1;
        }
    }
    for rollout in &manifest.rollouts {
        if progress.is_some_and(|progress| !progress.rollout_paths.contains(&rollout.relative_path))
        {
            continue;
        }
        let path = artifact_from_relative(home, &rollout.relative_path)?;
        let current_head = read_raw_rollout_head(&path)?;
        let current_hash = sha256_hex(&current_head);
        if current_hash == rollout.aligned_head_sha256 {
            replace_rollout_head(&path, &rollout.aligned_head_sha256, &rollout.original_head)?;
            rollouts_updated += 1;
        }
    }
    verify_selective_restore(home, manifest, progress)?;
    Ok((threads_updated, rollouts_updated))
}

fn verify_selective_restore(
    home: &Path,
    manifest: &RecoveryManifest,
    progress: Option<&AlignmentMutationProgress>,
) -> Result<(), AlignmentError> {
    for database in &manifest.databases {
        if progress
            .is_some_and(|progress| !progress.database_paths.contains(&database.relative_path))
        {
            continue;
        }
        let path = artifact_from_relative(home, &database.relative_path)?;
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| sqlite_error_or(error, AlignmentError::Mutation))?;
        for row in &database.original_rows {
            if let Some(current) = thread_provider_by_id(&connection, &row.id)? {
                if current != row.provider {
                    return Err(AlignmentError::Mutation);
                }
            }
        }
        verify_quick_check(&path)?;
    }
    for rollout in &manifest.rollouts {
        if progress.is_some_and(|progress| !progress.rollout_paths.contains(&rollout.relative_path))
        {
            continue;
        }
        let path = artifact_from_relative(home, &rollout.relative_path)?;
        if sha256_hex(&read_raw_rollout_head(&path)?) != rollout.original_head_sha256 {
            return Err(AlignmentError::Mutation);
        }
    }
    for upgrade in &manifest.rollout_upgrades {
        if progress.is_some_and(|progress| !progress.rollout_paths.contains(&upgrade.relative_path))
        {
            continue;
        }
        let path = artifact_from_relative(home, &upgrade.relative_path)?;
        if !crate::app::history_recovery::recovery_rollout_starts_with(&path, &upgrade.original)? {
            return Err(AlignmentError::Mutation);
        }
    }
    Ok(())
}

fn thread_provider_by_id(
    connection: &Connection,
    id: &str,
) -> Result<Option<Option<String>>, AlignmentError> {
    let mut statement = connection
        .prepare("SELECT model_provider FROM threads WHERE id = ?1")
        .map_err(|_| AlignmentError::Database)?;
    let mut rows = statement
        .query(params![id])
        .map_err(|_| AlignmentError::Database)?;
    match rows.next().map_err(|_| AlignmentError::Database)? {
        Some(row) => Ok(Some(row.get(0).map_err(|_| AlignmentError::Database)?)),
        None => Ok(None),
    }
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

#[cfg(target_os = "windows")]
pub(crate) fn create_replacement_file(_source: &Path, destination: &Path) -> std::io::Result<File> {
    create_private_file(destination)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn create_replacement_file(_source: &Path, destination: &Path) -> std::io::Result<File> {
    create_private_file(destination)
}

#[cfg(target_os = "windows")]
fn create_private_file(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new().create_new(true).write(true).open(path)?;
    if let Err(error) = set_private_windows_acl(path, false) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

#[cfg(all(not(unix), not(target_os = "windows")))]
fn create_private_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

pub(crate) fn write_private_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = create_private_file(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

#[cfg(unix)]
pub(crate) fn set_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(target_os = "windows")]
pub(crate) fn set_private_directory(path: &Path) -> std::io::Result<()> {
    set_private_windows_acl(path, true)
}

#[cfg(all(not(unix), not(target_os = "windows")))]
pub(crate) fn set_private_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_private_windows_acl(path: &Path, directory: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    let descriptor = if directory {
        "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
    } else {
        "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)"
    };
    let descriptor: Vec<u16> = descriptor.encode_utf16().chain(Some(0)).collect();
    let mut security_descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            SDDL_REVISION_1,
            &mut security_descriptor,
            null_mut(),
        )
    };
    if converted == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let result = (|| {
        let mut dacl_present = 0;
        let mut dacl_defaulted = 0;
        let mut dacl: *mut ACL = null_mut();
        let read = unsafe {
            GetSecurityDescriptorDacl(
                security_descriptor,
                &mut dacl_present,
                &mut dacl,
                &mut dacl_defaulted,
            )
        };
        if read == 0 || dacl_present == 0 || dacl.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let status = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status as i32))
        }
    })();
    unsafe {
        LocalFree(security_descriptor);
    }
    result
}

#[cfg(unix)]
pub(crate) fn sync_dir(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            source.as_ptr(),
            null(),
            REPLACEFILE_WRITE_THROUGH,
            null(),
            null(),
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn active_provider(config: &[u8]) -> Result<String, AlignmentError> {
    let config = config.strip_prefix(b"\xef\xbb\xbf").unwrap_or(config);
    let text = std::str::from_utf8(config).map_err(|_| AlignmentError::InvalidConfig)?;
    let document =
        toml::from_str::<toml::Value>(text).map_err(|_| AlignmentError::InvalidConfig)?;
    let provider = document
        .as_table()
        .and_then(|table| table.get("model_provider"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        })
        .ok_or(AlignmentError::MissingProvider)?;
    Ok(provider.to_string())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, AlignmentError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| AlignmentError::CodexHomeUnavailable)?;
    if !canonical
        .metadata()
        .map_err(|_| AlignmentError::CodexHomeUnavailable)?
        .is_dir()
    {
        return Err(AlignmentError::CodexHomeUnavailable);
    }
    Ok(canonical)
}

fn discover_databases(home: &Path) -> Result<Vec<PathBuf>, AlignmentError> {
    let mut candidates = Vec::new();
    collect_database_candidates(home, home, &mut candidates)?;
    let sqlite = home.join("sqlite");
    if sqlite.exists() {
        let sqlite = safe_canonical_path(&sqlite, home)?;
        if !sqlite.is_dir() {
            return Err(AlignmentError::UnsafePath);
        }
        collect_database_candidates(&sqlite, home, &mut candidates)?;
    }
    deduplicate_paths(candidates)
}

fn collect_database_candidates(
    directory: &Path,
    home: &Path,
    candidates: &mut Vec<PathBuf>,
) -> Result<(), AlignmentError> {
    for entry in fs::read_dir(directory).map_err(|error| io_error_or(error, AlignmentError::Io))? {
        let entry = entry.map_err(|error| io_error_or(error, AlignmentError::Io))?;
        if !database_name_matches(&entry.file_name()) {
            continue;
        }
        let path = safe_canonical_path(&entry.path(), home)?;
        if !path.is_file() {
            return Err(AlignmentError::UnsafePath);
        }
        candidates.push(path);
    }
    Ok(())
}

fn database_name_matches(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("state_") && name.ends_with(".sqlite")
}

fn discover_rollouts(home: &Path) -> Result<Vec<PathBuf>, AlignmentError> {
    let mut candidates = Vec::new();
    let mut visited_directories = BTreeSet::new();
    for root in [home.join("sessions"), home.join("archived_sessions")] {
        if !root.exists() {
            continue;
        }
        let root = safe_canonical_path(&root, home)?;
        collect_rollout_candidates(&root, home, &mut visited_directories, &mut candidates)?;
    }
    deduplicate_paths(candidates)
}

fn collect_rollout_candidates(
    directory: &Path,
    home: &Path,
    visited_directories: &mut BTreeSet<String>,
    candidates: &mut Vec<PathBuf>,
) -> Result<(), AlignmentError> {
    let key = path_key(directory);
    if !visited_directories.insert(key) {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(|error| io_error_or(error, AlignmentError::Io))? {
        let entry = entry.map_err(|error| io_error_or(error, AlignmentError::Io))?;
        let file_type = entry
            .file_type()
            .map_err(|error| io_error_or(error, AlignmentError::Io))?;
        if file_type.is_dir() || file_type.is_symlink() {
            let path = safe_canonical_path(&entry.path(), home)?;
            if path.is_dir() {
                collect_rollout_candidates(&path, home, visited_directories, candidates)?;
                continue;
            }
        }
        if !rollout_name_matches(&entry.file_name()) {
            continue;
        }
        let path = safe_canonical_path(&entry.path(), home)?;
        if !path.is_file() {
            return Err(AlignmentError::UnsafePath);
        }
        candidates.push(path);
    }
    Ok(())
}

fn rollout_name_matches(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn safe_canonical_path(path: &Path, home: &Path) -> Result<PathBuf, AlignmentError> {
    let canonical = path
        .canonicalize()
        .map_err(|error| io_error_or(error, AlignmentError::Io))?;
    if !canonical.starts_with(home) || canonical == home {
        return Err(AlignmentError::UnsafePath);
    }
    Ok(canonical)
}

fn deduplicate_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, AlignmentError> {
    let mut unique = BTreeMap::new();
    for path in paths {
        unique.entry(path_key(&path)).or_insert(path);
    }
    Ok(unique.into_values().collect())
}

fn path_key(path: &Path) -> String {
    let key = path.to_string_lossy().into_owned();
    if cfg!(target_os = "windows") {
        key.to_lowercase()
    } else {
        key
    }
}

fn scan_database(path: &Path, provider: &str) -> Result<DatabaseImpact, AlignmentError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    connection
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    connection
        .execute_batch("BEGIN DEFERRED TRANSACTION")
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;

    let provider_column_count = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('threads') WHERE name = 'model_provider'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    let id_column_count = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('threads') WHERE name = 'id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    if provider_column_count != 1 || id_column_count != 1 {
        return Err(AlignmentError::UnsupportedSchema);
    }

    let thread_count = count_query(&connection, "SELECT COUNT(*) FROM threads", [])?;
    let mismatched_threads = count_query(
        &connection,
        "SELECT COUNT(*) FROM threads WHERE model_provider IS NULL OR model_provider <> ?1",
        params![provider],
    )?;
    let mut statement = connection
        .prepare(
            "SELECT model_provider, COUNT(*)
             FROM threads
             GROUP BY model_provider
             ORDER BY model_provider",
        )
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    let provider_counts = statement
        .query_map([], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?
        .map(|row| {
            let (provider, count) =
                row.map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
            Ok((provider, safe_count(count)?))
        })
        .collect::<Result<Vec<_>, AlignmentError>>()?;
    drop(statement);
    connection
        .execute_batch("COMMIT")
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;

    let (file_len, modified_nanos) = metadata_snapshot(path)?;
    Ok(DatabaseImpact {
        path: path.to_path_buf(),
        file_len,
        modified_nanos,
        thread_count,
        mismatched_threads,
        provider_counts,
    })
}

fn count_query<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<u64, AlignmentError> {
    let value = connection
        .query_row(sql, params, |row| row.get::<_, i64>(0))
        .map_err(|error| sqlite_error_or(error, AlignmentError::Database))?;
    safe_count(value)
}

fn safe_count(value: i64) -> Result<u64, AlignmentError> {
    u64::try_from(value).map_err(|_| AlignmentError::Database)
}

fn scan_rollout(path: &Path, active_provider: &str) -> Result<RolloutImpact, AlignmentError> {
    scan_rollout_with_limits(
        path,
        active_provider,
        MAX_RECOVERY_ROLLOUT_BYTES,
        MAX_RECOVERY_ROLLOUT_BYTES,
    )
}

fn scan_rollout_with_limits(
    path: &Path,
    active_provider: &str,
    recovery_input_limit: u64,
    recovery_output_limit: u64,
) -> Result<RolloutImpact, AlignmentError> {
    let (file_len, modified_nanos) = metadata_snapshot(path)?;
    let file = File::open(path).map_err(|error| io_error_or(error, AlignmentError::Io))?;
    let raw_head = match read_rollout_head_from_reader(&mut BufReader::new(file)) {
        Ok(head) => head,
        Err(AlignmentError::BlockedMetadata) => {
            return Ok(unsupported_rollout(
                path,
                file_len,
                modified_nanos,
                &[],
                "alignment_rollout_head_too_large",
            ));
        }
        Err(error) => return Err(error),
    };
    let head = rollout_json_bytes(&raw_head);
    let head_sha256 = sha256_hex(&raw_head);
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(head) else {
        return Ok(unsupported_rollout(
            path,
            file_len,
            modified_nanos,
            &raw_head,
            "alignment_rollout_invalid_json",
        ));
    };
    if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
        return Ok(unsupported_rollout(
            path,
            file_len,
            modified_nanos,
            &raw_head,
            "alignment_rollout_missing_session_meta",
        ));
    }
    let Some(payload) = value.get("payload").and_then(serde_json::Value::as_object) else {
        return Ok(unsupported_rollout(
            path,
            file_len,
            modified_nanos,
            &raw_head,
            "alignment_rollout_payload_missing",
        ));
    };
    let recovery_id = payload
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_string);
    let mut upgrade_skip_reason = None;
    let recovery_artifact_bytes = if cfg!(target_os = "windows")
        && payload
            .get("originator")
            .and_then(serde_json::Value::as_str)
            == Some("Codex history evidence recovery")
    {
        match recovery_id.as_deref() {
            Some(id) => {
                match crate::app::history_recovery::planned_generated_rollout_upgrade_with_limits(
                    path,
                    id,
                    active_provider,
                    recovery_input_limit,
                    recovery_output_limit,
                ) {
                    Ok(crate::app::history_recovery::GeneratedRecoveryUpgradePlan::Planned(
                        artifact_bytes,
                    )) => artifact_bytes,
                    Ok(crate::app::history_recovery::GeneratedRecoveryUpgradePlan::NotNeeded) => 0,
                    Ok(
                        crate::app::history_recovery::GeneratedRecoveryUpgradePlan::ExceedsOutputLimit,
                    ) => {
                        upgrade_skip_reason = Some("alignment_recovery_artifact_budget");
                        0
                    }
                    Err(AlignmentError::PermissionDenied) => {
                        return Err(AlignmentError::PermissionDenied)
                    }
                    Err(_) => 0,
                }
            }
            None => 0,
        }
    } else {
        0
    };
    let needs_desktop_upgrade = recovery_artifact_bytes > 0;
    let recovery_file_sha256 = if needs_desktop_upgrade {
        Some(sha256_hex(
            &fs::read(path).map_err(|error| io_error_or(error, AlignmentError::Io))?,
        ))
    } else {
        None
    };
    let (provider, class) = match payload.get("model_provider") {
        Some(serde_json::Value::String(provider)) if !provider.trim().is_empty() => {
            let provider = provider.trim().to_string();
            let class = if provider == active_provider {
                RolloutClass::Aligned
            } else {
                RolloutClass::Repairable
            };
            (Some(provider), class)
        }
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => {
            (None, RolloutClass::Repairable)
        }
        Some(_) => {
            return Ok(unsupported_rollout(
                path,
                file_len,
                modified_nanos,
                &raw_head,
                "alignment_rollout_provider_invalid_type",
            ));
        }
    };

    Ok(RolloutImpact {
        path: path.to_path_buf(),
        file_len,
        modified_nanos,
        head_sha256,
        raw_head,
        provider,
        class,
        recovery_id,
        needs_desktop_upgrade,
        upgrade_skip_reason,
        recovery_artifact_bytes,
        recovery_file_sha256,
    })
}

fn unsupported_rollout(
    path: &Path,
    file_len: u64,
    modified_nanos: u128,
    head: &[u8],
    warning: &'static str,
) -> RolloutImpact {
    RolloutImpact {
        path: path.to_path_buf(),
        file_len,
        modified_nanos,
        head_sha256: sha256_hex(head),
        raw_head: head.to_vec(),
        provider: None,
        class: RolloutClass::Skipped(warning),
        recovery_id: None,
        needs_desktop_upgrade: false,
        upgrade_skip_reason: None,
        recovery_artifact_bytes: 0,
        recovery_file_sha256: None,
    }
}

fn rollout_json_bytes(raw_head: &[u8]) -> &[u8] {
    let without_newline = raw_head.strip_suffix(b"\n").unwrap_or(raw_head);
    let without_ending = without_newline
        .strip_suffix(b"\r")
        .unwrap_or(without_newline);
    without_ending
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(without_ending)
}

fn metadata_snapshot(path: &Path) -> Result<(u64, u128), AlignmentError> {
    let metadata = path
        .metadata()
        .map_err(|error| io_error_or(error, AlignmentError::Io))?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    Ok((metadata.len(), modified_nanos))
}

fn semantic_token(plan: &AlignmentPlan) -> String {
    let mut hasher = Sha256::new();
    hash_segment(&mut hasher, b"conversation-alignment-v1");
    hash_segment(&mut hasher, plan.provider.as_bytes());
    for database in &plan.databases {
        hash_segment(&mut hasher, b"database");
        hash_segment(&mut hasher, path_key(&database.path).as_bytes());
        hash_u128(&mut hasher, database.file_len as u128);
        hash_u128(&mut hasher, database.modified_nanos);
        hash_u128(&mut hasher, database.thread_count as u128);
        hash_u128(&mut hasher, database.mismatched_threads as u128);
        for (provider, count) in &database.provider_counts {
            hash_segment(
                &mut hasher,
                provider.as_deref().unwrap_or("<null>").as_bytes(),
            );
            hash_u128(&mut hasher, *count as u128);
        }
    }
    for rollout in &plan.rollouts {
        hash_segment(&mut hasher, b"rollout");
        hash_segment(&mut hasher, path_key(&rollout.path).as_bytes());
        hash_u128(&mut hasher, rollout.file_len as u128);
        hash_u128(&mut hasher, rollout.modified_nanos);
        hash_segment(&mut hasher, rollout.head_sha256.as_bytes());
        hash_segment(
            &mut hasher,
            rollout.provider.as_deref().unwrap_or("<none>").as_bytes(),
        );
        hash_u128(&mut hasher, rollout.recovery_artifact_bytes as u128);
        hash_segment(
            &mut hasher,
            rollout.upgrade_skip_reason.unwrap_or("<none>").as_bytes(),
        );
        hash_segment(
            &mut hasher,
            rollout
                .recovery_file_sha256
                .as_deref()
                .unwrap_or("<none>")
                .as_bytes(),
        );
        let (class, reason) = match rollout.class {
            RolloutClass::Aligned => ("aligned", ""),
            RolloutClass::Repairable => ("repairable", ""),
            RolloutClass::Skipped(reason) => ("skipped", reason),
        };
        hash_segment(&mut hasher, class.as_bytes());
        hash_segment(&mut hasher, reason.as_bytes());
    }
    for warning in &plan.warnings {
        hash_segment(&mut hasher, warning.as_bytes());
    }
    hex_digest(hasher.finalize().as_slice())
}

fn hash_segment(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn hash_u128(hasher: &mut Sha256, value: u128) {
    hasher.update(value.to_le_bytes());
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::ffi::OsStr;
    use std::fs::{self, File, OpenOptions};
    use std::io::{Cursor, Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    use rusqlite::{params, Connection};

    #[cfg(target_os = "windows")]
    use super::set_private_directory;
    use super::{
        aligned_rollout_head, apply_alignment, apply_recovery_artifact_budget,
        build_alignment_plan, build_alignment_plan_with_artifact_limit, classify_codex_process,
        create_recovery, execute_alignment_at, execute_alignment_at_with_fault, io_error_or,
        is_codex_process_name, load_recovery, load_recovery_with_artifact_limit,
        pre_mutation_failure, read_rollout_head_from_reader, recovery_directory_at,
        restart_allowed, restore_after_failed_alignment, restore_alignment_at,
        restore_alignment_with_process_at, scan_alignment_at, scan_from_plan,
        scan_rollout_with_limits, sqlite_error_or, verify_aligned_state, AlignmentError,
        AlignmentFault, AlignmentMutationProgress, AlignmentOutcome, AlignmentPaths, ProcessClass,
        ProcessGate, ProcessState, RolloutClass, MAX_RECOVERY_ROLLOUT_BYTES,
    };

    #[derive(Default)]
    struct TestProcessGate;

    impl ProcessGate for TestProcessGate {
        fn process_state(&self) -> Result<ProcessState, AlignmentError> {
            Ok(ProcessState::default())
        }

        fn stop_desktop(&self) -> Result<bool, AlignmentError> {
            Ok(false)
        }
    }

    struct SequencedProcessGate {
        states: RefCell<VecDeque<ProcessState>>,
    }

    impl SequencedProcessGate {
        fn new(states: impl IntoIterator<Item = ProcessState>) -> Self {
            Self {
                states: RefCell::new(states.into_iter().collect()),
            }
        }
    }

    impl ProcessGate for SequencedProcessGate {
        fn process_state(&self) -> Result<ProcessState, AlignmentError> {
            Ok(self.states.borrow_mut().pop_front().unwrap_or_default())
        }

        fn stop_desktop(&self) -> Result<bool, AlignmentError> {
            Ok(false)
        }
    }

    struct StoppingProcessGate {
        states: RefCell<VecDeque<ProcessState>>,
        stop_calls: Cell<usize>,
    }

    impl StoppingProcessGate {
        fn new(states: impl IntoIterator<Item = ProcessState>) -> Self {
            Self {
                states: RefCell::new(states.into_iter().collect()),
                stop_calls: Cell::new(0),
            }
        }

        fn stop_calls(&self) -> usize {
            self.stop_calls.get()
        }
    }

    impl ProcessGate for StoppingProcessGate {
        fn process_state(&self) -> Result<ProcessState, AlignmentError> {
            Ok(self.states.borrow_mut().pop_front().unwrap_or_default())
        }

        fn stop_desktop(&self) -> Result<bool, AlignmentError> {
            self.stop_calls.set(self.stop_calls.get() + 1);
            Ok(true)
        }
    }

    struct Fixture {
        root: PathBuf,
        paths: AlignmentPaths,
    }

    impl Fixture {
        fn new(name: &str, config: &str) -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-data")
                .join(format!(
                    "conversation-alignment-{name}-{}",
                    uuid::Uuid::new_v4()
                ));
            let codex_home = root.join("codex-home");
            fs::create_dir_all(&codex_home).unwrap();
            fs::write(codex_home.join("config.toml"), config).unwrap();
            Self {
                paths: AlignmentPaths::from_codex_home(codex_home),
                root,
            }
        }

        fn home(&self) -> &Path {
            &self.paths.codex_home
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn create_database(path: &Path, providers: &[Option<&str>]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    model_provider TEXT,
                    archived INTEGER NOT NULL DEFAULT 0
                );",
            )
            .unwrap();
        let transaction = connection.transaction().unwrap();
        for (index, provider) in providers.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO threads (id, model_provider) VALUES (?1, ?2)",
                    params![format!("thread-{index}"), provider],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
    }

    fn write_rollout(path: &Path, provider: &str, line_ending: &str, bom: bool) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let prefix = if bom { "\u{feff}" } else { "" };
        let first = serde_json::json!({
            "timestamp": "2026-07-30T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": "session-id",
                "model_provider": provider,
                "cwd": "C:/fixture"
            }
        });
        fs::write(
            path,
            format!(
                "{prefix}{first}{line_ending}{{\"type\":\"response_item\",\"payload\":{{\"safe\":true}}}}{line_ending}"
            ),
        )
        .unwrap();
    }

    fn write_large_rollout(path: &Path, provider: &str, padding_len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let first = serde_json::json!({
            "timestamp": "2026-07-30T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": "session-id",
                "model_provider": provider,
                "cwd": "C:/fixture",
                "padding": "x".repeat(padding_len)
            }
        });
        fs::write(path, format!("{first}\n{{\"type\":\"response_item\"}}\n")).unwrap();
    }

    fn write_generated_legacy_recovery(path: &Path, provider: &str) -> Vec<u8> {
        const ID: &str = "019b5555-1111-7222-8333-444444444444";
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let records = [
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:00Z",
                "type": "session_meta",
                "payload": {
                    "id": ID,
                    "timestamp": "2026-01-01T00:00:00Z",
                    "cwd": "C:/fixture",
                    "originator": "Codex history evidence recovery",
                    "cli_version": "1.0.0",
                    "source": "cli",
                    "model_provider": provider,
                    "thread_source": "user"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:01Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "surviving evidence"}]
                }
            }),
        ];
        let bytes = (records
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n")
            .into_bytes();
        fs::write(path, &bytes).unwrap();
        bytes
    }

    fn thread_provider(path: &Path, id: &str) -> Option<String> {
        Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT model_provider FROM threads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn rollout_provider(path: &Path) -> String {
        let bytes = fs::read(path).unwrap();
        let head = bytes.split(|byte| *byte == b'\n').next().unwrap();
        let head = head.strip_suffix(b"\r").unwrap_or(head);
        let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
        let value: serde_json::Value = serde_json::from_slice(head).unwrap();
        value
            .pointer("/payload/model_provider")
            .and_then(serde_json::Value::as_str)
            .unwrap()
            .to_string()
    }

    fn rollout_tail(path: &Path) -> Vec<u8> {
        let bytes = fs::read(path).unwrap();
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or_else(Vec::new, |index| bytes[index + 1..].to_vec())
    }

    #[test]
    fn process_classification_treats_only_the_installed_tree_as_desktop() {
        let installed_root = PathBuf::from("installed").join("Codex");
        let desktop_exe = installed_root.join("Contents").join("MacOS").join("Codex");
        let cli_exe = PathBuf::from("tools").join("codex");
        let sibling_exe = PathBuf::from("installed").join("Codex CLI").join("codex");

        assert_eq!(
            classify_codex_process(Some(&desktop_exe), Some(&installed_root)),
            ProcessClass::Desktop
        );
        assert_eq!(
            classify_codex_process(Some(&cli_exe), Some(&installed_root)),
            ProcessClass::Cli
        );
        assert_eq!(
            classify_codex_process(Some(&sibling_exe), Some(&installed_root)),
            ProcessClass::Cli
        );
        assert_eq!(
            classify_codex_process(None, Some(&installed_root)),
            ProcessClass::Cli
        );
        assert_eq!(
            classify_codex_process(Some(&desktop_exe), None),
            ProcessClass::Cli
        );
    }

    #[test]
    fn alignment_preserves_native_permission_error_kinds() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert!(matches!(
            io_error_or(io, AlignmentError::Io),
            AlignmentError::PermissionDenied
        ));

        for code in [
            rusqlite::ffi::SQLITE_PERM,
            rusqlite::ffi::SQLITE_READONLY,
            rusqlite::ffi::SQLITE_AUTH,
        ] {
            let sqlite = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            assert!(matches!(
                sqlite_error_or(sqlite, AlignmentError::Database),
                AlignmentError::PermissionDenied
            ));
        }
    }

    #[test]
    fn alignment_permission_denied_before_recovery_returns_elevation_report() {
        let report = pre_mutation_failure(AlignmentError::PermissionDenied, false, None).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::StalePlan);
        assert!(report.elevation_required);
        assert_eq!(
            report.error_code.as_deref(),
            Some("alignment_permission_denied")
        );
        assert!(report.recovery_id.is_none());
    }

    #[test]
    fn restart_requires_a_verified_safe_outcome() {
        assert!(restart_allowed(AlignmentOutcome::Committed, true, false));
        assert!(restart_allowed(AlignmentOutcome::Restored, false, true));
        assert!(!restart_allowed(
            AlignmentOutcome::RecoveryRequired,
            true,
            false
        ));
        assert!(!restart_allowed(AlignmentOutcome::Committed, false, false));
        assert!(!restart_allowed(AlignmentOutcome::Restored, false, false));
        assert!(!restart_allowed(AlignmentOutcome::StalePlan, false, false));
    }

    #[test]
    fn process_name_filter_accepts_only_codex_executables() {
        assert!(is_codex_process_name(OsStr::new("Codex")));
        assert!(is_codex_process_name(OsStr::new("codex.exe")));
        assert!(!is_codex_process_name(OsStr::new("Codex App Manager")));
        assert!(!is_codex_process_name(OsStr::new("Codex Helper")));
        assert!(!is_codex_process_name(OsStr::new("codex-cli")));
    }

    #[test]
    fn scan_counts_databases_and_rollouts_for_the_top_level_provider() {
        let fixture = Fixture::new(
            "scan-counts",
            r#"model_provider = "orange"

[profile.default]
model_provider = "ignored"
"#,
        );
        create_database(
            &fixture.home().join("state_root.sqlite"),
            &[Some("orange"), Some("legacy"), None],
        );
        create_database(
            &fixture.home().join("sqlite/state_nested.sqlite"),
            &[Some("orange")],
        );
        write_rollout(
            &fixture.home().join("sessions/2026/07/rollout-active.jsonl"),
            "legacy",
            "\r\n",
            false,
        );
        write_rollout(
            &fixture
                .home()
                .join("archived_sessions/rollout-archived.jsonl"),
            "orange",
            "\n",
            true,
        );

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_eq!(scan.active_provider, "orange");
        assert_eq!(scan.database_count, 2);
        assert_eq!(scan.thread_count, 4);
        assert_eq!(scan.mismatched_threads, 2);
        assert_eq!(scan.rollout_count, 2);
        assert_eq!(scan.mismatched_rollouts, 1);
        assert!(scan.can_execute);
        assert!(scan.warnings.is_empty());
        assert_eq!(scan.scan_token.len(), 64);
    }

    #[test]
    fn scan_rejects_a_database_without_the_provider_column() {
        let fixture = Fixture::new("scan-schema", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_invalid.sqlite");
        let connection = Connection::open(database).unwrap();
        connection
            .execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY);")
            .unwrap();

        let error = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap_err();

        assert_eq!(error.code(), "alignment_unsupported_schema");
    }

    #[test]
    fn scan_rejects_a_database_without_the_thread_id_column() {
        let fixture = Fixture::new("scan-id-schema", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_invalid_id.sqlite");
        let connection = Connection::open(database).unwrap();
        connection
            .execute_batch("CREATE TABLE threads (model_provider TEXT);")
            .unwrap();

        let error = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap_err();

        assert_eq!(error.code(), "alignment_unsupported_schema");
    }

    #[test]
    fn scan_keeps_valid_rollouts_executable_when_one_rollout_is_malformed() {
        let fixture = Fixture::new("scan-malformed", "model_provider = \"orange\"\n");
        let sessions = fixture.home().join("sessions");
        let malformed = sessions.join("rollout-malformed.jsonl");
        write_rollout(&sessions.join("rollout-valid.jsonl"), "legacy", "\n", false);
        fs::write(&malformed, b"not-json\n{\"type\":\"response_item\"}\n").unwrap();

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_eq!(scan.rollout_count, 2);
        assert_eq!(scan.mismatched_rollouts, 1);
        assert_eq!(scan.skipped_rollouts, 1);
        assert!(scan.can_execute);
        assert!(scan.warnings.is_empty());
        assert_eq!(scan.skip_reasons, vec!["alignment_rollout_invalid_json"]);
    }

    #[test]
    fn scan_repairs_missing_null_and_empty_provider_slots() {
        let fixture = Fixture::new("scan-repairable-provider", "model_provider = \"orange\"\n");
        let sessions = fixture.home().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        for (name, payload) in [
            ("missing", serde_json::json!({"id": "missing"})),
            (
                "null",
                serde_json::json!({"id": "null", "model_provider": null}),
            ),
            (
                "empty",
                serde_json::json!({"id": "empty", "model_provider": ""}),
            ),
        ] {
            let head = serde_json::json!({
                "type": "session_meta",
                "payload": payload,
            });
            fs::write(
                sessions.join(format!("rollout-{name}.jsonl")),
                format!("{head}\n{{\"type\":\"response_item\"}}\n"),
            )
            .unwrap();
        }

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_eq!(scan.rollout_count, 3);
        assert_eq!(scan.mismatched_rollouts, 3);
        assert_eq!(scan.skipped_rollouts, 0);
        assert!(scan.skip_reasons.is_empty());
        assert!(scan.can_execute);
    }

    #[test]
    fn scan_token_is_stable_until_relevant_metadata_changes() {
        let fixture = Fixture::new("scan-token", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_token.sqlite");
        create_database(&database, &[Some("legacy")]);
        let first = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let unchanged = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        assert_eq!(first.scan_token, unchanged.scan_token);

        let connection = Connection::open(database).unwrap();
        connection
            .execute("UPDATE threads SET model_provider = ?1", params!["orange"])
            .unwrap();
        let changed = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_ne!(first.scan_token, changed.scan_token);
        assert_eq!(changed.mismatched_threads, 0);
    }

    #[test]
    fn scan_reports_an_active_cli_without_blocking_writes() {
        let fixture = Fixture::new("scan-cli", "model_provider = \"orange\"\n");

        let scan = scan_alignment_at(
            &fixture.paths,
            ProcessState {
                desktop_running: true,
                cli_running: true,
            },
        )
        .unwrap();

        assert!(scan.desktop_running);
        assert!(scan.cli_running);
        assert!(scan.can_execute);
        assert!(scan.warnings.is_empty());
    }

    #[test]
    fn scan_reads_only_the_rollout_first_line() {
        let head = b"{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"orange\"}}\n";
        let mut bytes = head.to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 2 * 1024 * 1024));
        let mut reader = Cursor::new(bytes);

        let actual = read_rollout_head_from_reader(&mut reader).unwrap();

        assert_eq!(actual, head);
        assert_eq!(reader.position(), head.len() as u64);
    }

    #[test]
    fn alignment_no_op_does_not_create_a_backup() {
        let fixture = Fixture::new("alignment-no-op", "model_provider = \"orange\"\n");
        create_database(
            &fixture.home().join("state_aligned.sqlite"),
            &[Some("orange")],
        );
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::NoChanges);
        assert!(report.backup_dir.is_none());
        assert!(!fixture.paths.backup_root.exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn aligned_generated_legacy_recovery_is_scannable_upgradable_and_undoable() {
        let fixture = Fixture::new(
            "alignment-generated-legacy-only",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-legacy.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "orange");

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_eq!(scan.mismatched_threads, 0);
        assert_eq!(scan.mismatched_rollouts, 1);
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        assert_eq!(
            committed.outcome,
            AlignmentOutcome::Committed,
            "report: {committed:?}"
        );
        assert_eq!(committed.rollouts_updated, 1);
        assert!(fs::read_to_string(&rollout)
            .unwrap()
            .contains("\"task_started\""));

        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();
        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert_eq!(fs::read(&rollout).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn provider_repair_and_generated_recovery_upgrade_share_one_reversible_mutation() {
        let fixture = Fixture::new(
            "alignment-generated-provider-overlap",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-provider-overlap.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "legacy");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(
            committed.outcome,
            AlignmentOutcome::Committed,
            "report: {committed:?}"
        );
        assert_eq!(committed.rollouts_updated, 1);
        assert_eq!(rollout_provider(&rollout), "orange");
        assert!(fs::read_to_string(&rollout)
            .unwrap()
            .contains("\"task_complete\""));
        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();
        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert_eq!(fs::read(&rollout).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn upgraded_recovery_verification_allows_events_appended_after_replacement() {
        let fixture = Fixture::new(
            "alignment-generated-verify-append",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-verify-append.jsonl");
        write_generated_legacy_recovery(&rollout, "orange");
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (_, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let upgrade = &manifest.rollout_upgrades[0];
        let mut replacement_landed = false;
        crate::app::history_recovery::replace_recovery_rollout(
            &rollout,
            &upgrade.original_sha256,
            &upgrade.upgraded,
            true,
            &mut replacement_landed,
            &|| Ok(()),
        )
        .unwrap();
        assert!(replacement_landed);
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n")
            .unwrap();

        assert!(verify_aligned_state(&plan, &manifest).is_ok());
    }

    #[test]
    fn generated_recovery_with_an_interrupted_desktop_turn_is_not_rewritten() {
        let fixture = Fixture::new(
            "alignment-generated-interrupted",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-interrupted.jsonl");
        let mut original = write_generated_legacy_recovery(&rollout, "orange");
        let interrupted = [
            serde_json::json!({"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}),
            serde_json::json!({"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"user_message","message":"resumed"}}),
            serde_json::json!({"timestamp":"2026-01-01T00:00:04Z","type":"response_item","payload":{"type":"function_call","name":"shell"}}),
        ]
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        original.extend_from_slice(interrupted.as_bytes());
        fs::write(&rollout, &original).unwrap();

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_eq!(scan.mismatched_rollouts, 0);
        assert_eq!(fs::read(&rollout).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_upgrade_blocks_an_active_cli() {
        let fixture = Fixture::new(
            "alignment-generated-active-cli",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-active-cli.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "orange");
        let active_cli = ProcessState {
            desktop_running: false,
            cli_running: true,
        };
        let scan = scan_alignment_at(&fixture.paths, active_cli).unwrap();
        let gate = StoppingProcessGate::new([active_cli]);

        assert!(!scan.can_execute);
        assert!(matches!(
            execute_alignment_at(&fixture.paths, &scan.scan_token, &gate),
            Err(AlignmentError::CliRunning)
        ));
        assert_eq!(fs::read(&rollout).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_undo_preserves_events_appended_after_upgrade() {
        let fixture = Fixture::new(
            "alignment-generated-undo-append",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-undo-append.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "orange");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        let appended = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(appended)
            .unwrap();

        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();

        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert_eq!(
            fs::read(&rollout).unwrap(),
            [original, appended.to_vec()].concat()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_undo_separates_appends_from_an_original_without_newline() {
        let fixture = Fixture::new(
            "alignment-generated-undo-no-newline",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-undo-no-newline.jsonl");
        let mut original = write_generated_legacy_recovery(&rollout, "orange");
        assert_eq!(original.pop(), Some(b'\n'));
        fs::write(&rollout, &original).unwrap();
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        let appended = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(appended)
            .unwrap();

        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();

        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert_eq!(
            fs::read(&rollout).unwrap(),
            [original, b"\n".to_vec(), appended.to_vec()].concat()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_undo_blocks_an_active_cli() {
        let fixture = Fixture::new(
            "alignment-generated-undo-active-cli",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-undo-active-cli.jsonl");
        write_generated_legacy_recovery(&rollout, "orange");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        let upgraded = fs::read(&rollout).unwrap();
        let active_cli = ProcessState {
            desktop_running: false,
            cli_running: true,
        };
        let gate = StoppingProcessGate::new([active_cli]);

        assert!(matches!(
            restore_alignment_with_process_at(
                &fixture.paths,
                committed.recovery_id.as_deref().unwrap(),
                &gate,
            ),
            Err(AlignmentError::CliRunning)
        ));
        assert_eq!(fs::read(&rollout).unwrap(), upgraded);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_rollback_blocks_an_active_cli() {
        let fixture = Fixture::new(
            "alignment-generated-rollback-active-cli",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-rollback-active-cli.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "orange");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let active_cli = ProcessState {
            desktop_running: false,
            cli_running: true,
        };
        let gate = SequencedProcessGate::new([
            ProcessState::default(),
            ProcessState::default(),
            ProcessState::default(),
            ProcessState::default(),
            active_cli,
        ]);

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &gate,
            AlignmentFault::AfterRolloutWrite,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::RecoveryRequired);
        assert!(!report.rollback_verified);
        assert_ne!(fs::read(&rollout).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_rollback_allows_cli_when_only_database_changes_landed() {
        let fixture = Fixture::new(
            "alignment-generated-database-only-rollback",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_database_only_rollback.sqlite");
        create_database(&database, &[Some("legacy")]);
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-database-only-rollback.jsonl");
        let original_rollout = write_generated_legacy_recovery(&rollout, "orange");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let active_cli = ProcessState {
            desktop_running: false,
            cli_running: true,
        };
        let gate = SequencedProcessGate::new([
            ProcessState::default(),
            ProcessState::default(),
            ProcessState::default(),
            active_cli,
            active_cli,
        ]);

        let report = execute_alignment_at(&fixture.paths, &scan.scan_token, &gate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert_eq!(
            thread_provider(&database, "thread-0"),
            Some("legacy".to_string())
        );
        assert_eq!(fs::read(&rollout).unwrap(), original_rollout);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_no_op_failure_is_already_rolled_back() {
        let fixture = Fixture::new(
            "alignment-generated-no-op-rollback",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-no-op-rollback.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "orange");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let active_cli = ProcessState {
            desktop_running: false,
            cli_running: true,
        };
        let gate = SequencedProcessGate::new([
            ProcessState::default(),
            ProcessState::default(),
            ProcessState::default(),
            active_cli,
        ]);

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &gate,
            AlignmentFault::BeforeRolloutWrite,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert_eq!(fs::read(&rollout).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn generated_recovery_undo_streams_suffixes_beyond_the_preimage_limit() {
        let fixture = Fixture::new(
            "alignment-generated-undo-large",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-undo-large.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "orange");
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        let upgraded_len = fs::metadata(&rollout).unwrap().len();
        let marker = b"appended-after-large-suffix";
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&rollout)
            .unwrap();
        file.set_len(MAX_RECOVERY_ROLLOUT_BYTES + 1).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(marker).unwrap();
        drop(file);

        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();

        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert_eq!(
            fs::metadata(&rollout).unwrap().len(),
            original.len() as u64
                + (MAX_RECOVERY_ROLLOUT_BYTES + 1 - upgraded_len)
                + marker.len() as u64
        );
        let mut file = File::open(&rollout).unwrap();
        file.seek(SeekFrom::End(-(marker.len() as i64))).unwrap();
        let mut tail = vec![0; marker.len()];
        file.read_exact(&mut tail).unwrap();
        assert_eq!(tail, marker);
    }

    #[test]
    fn malformed_generated_recovery_tail_still_allows_provider_repair() {
        let fixture = Fixture::new(
            "alignment-generated-malformed-tail",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-generated-malformed-tail.jsonl");
        write_generated_legacy_recovery(&rollout, "legacy");
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(b"{truncated\n")
            .unwrap();
        let original_tail = rollout_tail(&rollout);

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        assert_eq!(scan.mismatched_rollouts, 1);
        assert!(scan.can_execute, "warnings: {:?}", scan.warnings);
        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(rollout_provider(&rollout), "orange");
        assert_eq!(rollout_tail(&rollout), original_tail);
    }

    #[test]
    fn alignment_commits_after_backup_and_verifies_database_and_rollout() {
        let fixture = Fixture::new("alignment-commit", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_commit.sqlite");
        let rollout = fixture.home().join("sessions/rollout-commit.jsonl");
        create_database(&database, &[Some("legacy"), None]);
        write_rollout(&rollout, "legacy", "\r\n", true);
        let original_tail = rollout_tail(&rollout);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(
            report.outcome,
            AlignmentOutcome::Committed,
            "report: {report:?}"
        );
        assert!(report.write_verified);
        assert!(report.undo_allowed);
        assert!(report.restart_allowed);
        assert_eq!(report.threads_updated, 2);
        assert_eq!(report.rollouts_updated, 1);
        let backup = PathBuf::from(report.backup_dir.as_ref().unwrap());
        assert!(backup.join("manifest.json").is_file());
        assert!(backup.join("database-1.sqlite").is_file());
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("orange")
        );
        assert_eq!(
            thread_provider(&database, "thread-1").as_deref(),
            Some("orange")
        );
        assert_eq!(rollout_provider(&rollout), "orange");
        assert_eq!(rollout_tail(&rollout), original_tail);
        let integrity: String = Connection::open(database)
            .unwrap()
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn alignment_does_not_stop_or_block_an_active_cli() {
        let fixture = Fixture::new("alignment-active-cli", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_active_cli.sqlite");
        create_database(&database, &[Some("legacy")]);
        let active_cli = ProcessState {
            desktop_running: false,
            cli_running: true,
        };
        let scan = scan_alignment_at(&fixture.paths, active_cli).unwrap();
        let gate = StoppingProcessGate::new([active_cli, active_cli, active_cli]);

        let report = execute_alignment_at(&fixture.paths, &scan.scan_token, &gate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(gate.stop_calls(), 0);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("orange")
        );
    }

    #[test]
    fn alignment_commits_repairable_files_and_preserves_skipped_files() {
        let fixture = Fixture::new("alignment-partial", "model_provider = \"orange\"\n");
        let sessions = fixture.home().join("sessions");
        let repairable = sessions.join("rollout-repairable.jsonl");
        let broken = sessions.join("rollout-broken.jsonl");
        write_rollout(&repairable, "legacy", "\n", false);
        fs::write(&broken, b"not-json\n{\"type\":\"response_item\"}\n").unwrap();
        let broken_before = fs::read(&broken).unwrap();
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(report.rollouts_updated, 1);
        assert_eq!(report.rollouts_skipped, 1);
        assert!(!report.elevation_required);
        assert_eq!(rollout_provider(&repairable), "orange");
        assert_eq!(fs::read(&broken).unwrap(), broken_before);
    }

    #[test]
    fn missing_provider_is_inserted_without_changing_rollout_tail() {
        let fixture = Fixture::new(
            "alignment-missing-provider",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture.home().join("sessions/rollout-missing.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        fs::write(
            &rollout,
            b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"missing\"}}\n{\"type\":\"response_item\",\"payload\":{\"safe\":true}}\n",
        )
        .unwrap();
        let tail_before = rollout_tail(&rollout);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(report.rollouts_updated, 1);
        assert_eq!(rollout_provider(&rollout), "orange");
        assert_eq!(rollout_tail(&rollout), tail_before);
    }

    #[test]
    fn alignment_rejects_a_stale_token_without_creating_a_backup() {
        let fixture = Fixture::new("alignment-stale", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_stale.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE threads SET model_provider = ?1",
                params!["different-legacy"],
            )
            .unwrap();

        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::StalePlan);
        assert!(report.backup_dir.is_none());
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("different-legacy")
        );
        assert!(!fixture.paths.backup_root.exists());
    }

    #[test]
    fn alignment_failure_after_database_write_restores_every_preimage() {
        let fixture = Fixture::new("alignment-rollback", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_rollback.sqlite");
        let rollout = fixture.home().join("sessions/rollout-rollback.jsonl");
        create_database(&database, &[Some("legacy")]);
        write_rollout(&rollout, "legacy", "\n", false);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &TestProcessGate,
            AlignmentFault::BeforeRolloutWrite,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
        assert_eq!(rollout_provider(&rollout), "legacy");
        assert!(report
            .backup_dir
            .as_ref()
            .is_some_and(|path| Path::new(path).is_dir()));
    }

    #[test]
    fn alignment_permission_denied_before_writes_allows_elevated_retry() {
        let fixture = Fixture::new(
            "alignment-permission-before",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_permission_before.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &TestProcessGate,
            AlignmentFault::PermissionBeforeWrite,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert!(report.elevation_required);
        assert_eq!(
            report.error_code.as_deref(),
            Some("alignment_permission_denied")
        );
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
    }

    #[test]
    fn alignment_permission_denied_after_database_commit_rolls_back_before_retry() {
        let fixture = Fixture::new(
            "alignment-permission-database",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_permission_database.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &TestProcessGate,
            AlignmentFault::PermissionAfterDatabaseCommit,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert!(report.elevation_required);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
    }

    #[test]
    fn alignment_permission_denied_before_rollout_replacement_restores_database() {
        let fixture = Fixture::new(
            "alignment-permission-rollout",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_permission_rollout.sqlite");
        let rollout = fixture.home().join("sessions/rollout-permission.jsonl");
        create_database(&database, &[Some("legacy")]);
        write_rollout(&rollout, "legacy", "\n", false);
        let rollout_before = fs::read(&rollout).unwrap();
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &TestProcessGate,
            AlignmentFault::PermissionBeforeRolloutWrite,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert!(report.elevation_required);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
        assert_eq!(fs::read(&rollout).unwrap(), rollout_before);
    }

    #[test]
    fn alignment_permission_denied_during_rollback_requires_elevated_restore() {
        let fixture = Fixture::new(
            "alignment-permission-rollback",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_permission_rollback.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &TestProcessGate,
            AlignmentFault::PermissionDuringRollback,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::RecoveryRequired);
        assert!(!report.rollback_verified);
        assert!(report.elevation_required);
        assert_eq!(
            report.error_code.as_deref(),
            Some("alignment_permission_denied")
        );
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("orange")
        );
    }

    #[test]
    fn failed_alignment_rollback_preserves_concurrent_columns_and_later_rows() {
        let fixture = Fixture::new(
            "alignment-selective-rollback",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_selective_rollback.sqlite");
        let rollout = fixture
            .home()
            .join("sessions/rollout-selective-rollback.jsonl");
        create_database(&database, &[Some("legacy")]);
        write_rollout(&rollout, "legacy", "\n", false);
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let mut progress = AlignmentMutationProgress::default();

        assert!(matches!(
            apply_alignment(
                &plan,
                &manifest,
                &mut progress,
                AlignmentFault::BeforeRolloutWrite
            ),
            Err(AlignmentError::Mutation)
        ));
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE threads SET archived = 1 WHERE id = ?1",
                params!["thread-0"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, model_provider, archived) VALUES (?1, ?2, ?3)",
                params!["later-thread", "later-provider", 1],
            )
            .unwrap();
        drop(connection);

        assert!(restore_after_failed_alignment(
            &plan,
            &backup_dir,
            &manifest,
            &progress,
            AlignmentFault::None,
            &|| Ok(()),
        )
        .is_ok());
        let connection = Connection::open(&database).unwrap();
        let archived: i64 = connection
            .query_row(
                "SELECT archived FROM threads WHERE id = ?1",
                params!["thread-0"],
                |row| row.get(0),
            )
            .unwrap();
        let later: (String, i64) = connection
            .query_row(
                "SELECT model_provider, archived FROM threads WHERE id = ?1",
                params!["later-thread"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
        assert_eq!(archived, 1);
        assert_eq!(later, ("later-provider".to_string(), 1));
    }

    #[test]
    fn alignment_allows_cli_before_mutation_when_preimages_are_stable() {
        let fixture = Fixture::new(
            "alignment-cli-before-mutation",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_cli_before_mutation.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let gate = SequencedProcessGate::new([
            ProcessState::default(),
            ProcessState::default(),
            ProcessState {
                cli_running: true,
                ..ProcessState::default()
            },
        ]);

        let report = execute_alignment_at(&fixture.paths, &scan.scan_token, &gate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("orange")
        );
    }

    #[test]
    fn alignment_continues_after_cli_appears_once_desktop_was_stopped() {
        let fixture = Fixture::new("alignment-cli-after-stop", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_cli_after_stop.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(
            &fixture.paths,
            ProcessState {
                desktop_running: true,
                cli_running: false,
            },
        )
        .unwrap();
        let gate = StoppingProcessGate::new([
            ProcessState {
                desktop_running: true,
                cli_running: false,
            },
            ProcessState {
                desktop_running: false,
                cli_running: true,
            },
        ]);

        let report = execute_alignment_at(&fixture.paths, &scan.scan_token, &gate).unwrap();

        assert_eq!(gate.stop_calls(), 1);
        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert!(report.error_code.is_none());
        assert!(report.codex_was_running);
        assert!(report.restart_allowed);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("orange")
        );
    }

    #[test]
    fn alignment_reports_restart_when_backup_validation_fails_after_stop() {
        let mut fixture = Fixture::new(
            "alignment-backup-after-stop",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_backup_after_stop.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(
            &fixture.paths,
            ProcessState {
                desktop_running: true,
                cli_running: false,
            },
        )
        .unwrap();
        fixture.paths.backup_root = fixture.home().join("blocked-backup-root");
        fs::write(&fixture.paths.backup_root, b"not-a-directory").unwrap();
        let gate = StoppingProcessGate::new([
            ProcessState {
                desktop_running: true,
                cli_running: false,
            },
            ProcessState::default(),
        ]);

        let report = execute_alignment_at(&fixture.paths, &scan.scan_token, &gate).unwrap();

        assert_eq!(gate.stop_calls(), 1);
        assert_eq!(report.outcome, AlignmentOutcome::StalePlan);
        assert_eq!(report.error_code.as_deref(), Some("alignment_unsafe_path"));
        assert!(report.restart_allowed);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
    }

    #[test]
    fn restore_validates_recovery_before_stopping_desktop() {
        let fixture = Fixture::new(
            "restore-validates-before-stop",
            "model_provider = \"orange\"\n",
        );
        let gate = StoppingProcessGate::new([ProcessState {
            desktop_running: true,
            cli_running: false,
        }]);

        let error =
            restore_alignment_with_process_at(&fixture.paths, "missing", &gate).unwrap_err();

        assert!(matches!(error, AlignmentError::InvalidRecovery));
        assert_eq!(gate.stop_calls(), 0);
    }

    #[test]
    fn restore_allows_cli_after_final_recovery_validation() {
        let fixture = Fixture::new(
            "restore-final-process-gate",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_restore_final_gate.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        let gate = StoppingProcessGate::new([
            ProcessState {
                desktop_running: true,
                cli_running: false,
            },
            ProcessState::default(),
            ProcessState {
                desktop_running: false,
                cli_running: true,
            },
        ]);

        let report = restore_alignment_with_process_at(
            &fixture.paths,
            committed.recovery_id.as_deref().unwrap(),
            &gate,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.error_code.is_none());
        assert!(report.restart_allowed);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
    }

    #[test]
    fn alignment_rolls_back_when_cli_appears_after_mutation_failure() {
        let fixture = Fixture::new(
            "alignment-cli-before-rollback",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_cli_before_rollback.sqlite");
        let rollout = fixture
            .home()
            .join("sessions/rollout-cli-before-rollback.jsonl");
        create_database(&database, &[Some("legacy")]);
        write_rollout(&rollout, "legacy", "\n", false);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let gate = SequencedProcessGate::new([
            ProcessState::default(),
            ProcessState::default(),
            ProcessState::default(),
            ProcessState {
                cli_running: true,
                ..ProcessState::default()
            },
        ]);

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &gate,
            AlignmentFault::BeforeRolloutWrite,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Restored);
        assert!(report.rollback_verified);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
        assert!(report
            .backup_dir
            .as_ref()
            .is_some_and(|path| Path::new(path).is_dir()));
    }

    #[test]
    fn alignment_mutation_rejects_a_changed_recorded_preimage() {
        let fixture = Fixture::new(
            "alignment-changed-preimage",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_changed_preimage.sqlite");
        create_database(&database, &[Some("legacy")]);
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (_, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE threads SET model_provider = ?1 WHERE id = ?2",
                params!["concurrent-provider", "thread-0"],
            )
            .unwrap();
        let mut progress = AlignmentMutationProgress::default();

        let error =
            apply_alignment(&plan, &manifest, &mut progress, AlignmentFault::None).unwrap_err();

        assert!(matches!(error, AlignmentError::StaleRecovery));
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("concurrent-provider")
        );
    }

    #[test]
    fn alignment_rejects_a_rollout_tail_changed_after_recovery() {
        let fixture = Fixture::new("alignment-changed-tail", "model_provider = \"orange\"\n");
        let rollout = fixture.home().join("sessions/rollout-changed-tail.jsonl");
        write_rollout(&rollout, "legacy", "\n", false);
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (_, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(b"{\"type\":\"response_item\",\"payload\":{\"live\":true}}\n")
            .unwrap();
        let changed_tail = rollout_tail(&rollout);
        let mut progress = AlignmentMutationProgress::default();

        let error =
            apply_alignment(&plan, &manifest, &mut progress, AlignmentFault::None).unwrap_err();

        assert!(matches!(error, AlignmentError::StaleRecovery));
        assert_eq!(rollout_provider(&rollout), "legacy");
        assert_eq!(rollout_tail(&rollout), changed_tail);
    }

    #[test]
    fn failed_alignment_does_not_restore_a_changed_null_preimage_it_never_wrote() {
        let fixture = Fixture::new(
            "alignment-changed-null-preimage",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_changed_null_preimage.sqlite");
        create_database(&database, &[None]);
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE threads SET model_provider = ?1 WHERE id = ?2",
                params!["orange", "thread-0"],
            )
            .unwrap();
        let mut progress = AlignmentMutationProgress::default();

        let error =
            apply_alignment(&plan, &manifest, &mut progress, AlignmentFault::None).unwrap_err();

        assert!(matches!(error, AlignmentError::StaleRecovery));
        assert!(restore_after_failed_alignment(
            &plan,
            &backup_dir,
            &manifest,
            &progress,
            AlignmentFault::None,
            &|| Ok(()),
        )
        .is_ok());
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("orange")
        );
    }

    #[test]
    fn failed_alignment_tracks_a_database_whose_commit_may_have_landed() {
        let fixture = Fixture::new(
            "alignment-database-commit-progress",
            "model_provider = \"orange\"\n",
        );
        let database = fixture.home().join("state_database_commit_progress.sqlite");
        create_database(&database, &[Some("legacy")]);
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let mut progress = AlignmentMutationProgress::default();

        let error = apply_alignment(
            &plan,
            &manifest,
            &mut progress,
            AlignmentFault::AfterDatabaseCommit,
        )
        .unwrap_err();

        assert!(matches!(error, AlignmentError::Mutation));
        assert!(restore_after_failed_alignment(
            &plan,
            &backup_dir,
            &manifest,
            &progress,
            AlignmentFault::None,
            &|| Ok(()),
        )
        .is_ok());
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
    }

    #[test]
    fn failed_alignment_tracks_a_rollout_whose_replacement_may_have_landed() {
        let fixture = Fixture::new(
            "alignment-rollout-write-progress",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture.home().join("sessions/rollout-write-progress.jsonl");
        write_rollout(&rollout, "legacy", "\n", false);
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let mut progress = AlignmentMutationProgress::default();

        let error = apply_alignment(
            &plan,
            &manifest,
            &mut progress,
            AlignmentFault::AfterRolloutWrite,
        )
        .unwrap_err();

        assert!(matches!(error, AlignmentError::Mutation));
        assert!(restore_after_failed_alignment(
            &plan,
            &backup_dir,
            &manifest,
            &progress,
            AlignmentFault::None,
            &|| Ok(()),
        )
        .is_ok());
        assert_eq!(rollout_provider(&rollout), "legacy");
    }

    #[test]
    fn recovery_externalizes_large_rollout_heads_and_loads_them_before_mutation() {
        let fixture = Fixture::new("alignment-large-recovery", "model_provider = \"orange\"\n");
        write_large_rollout(
            &fixture.home().join("sessions/rollout-large-one.jsonl"),
            "legacy",
            512 * 1024,
        );
        write_large_rollout(
            &fixture.home().join("sessions/rollout-large-two.jsonl"),
            "legacy",
            512 * 1024,
        );
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();

        let (_, loaded) =
            load_recovery(&fixture.paths, &plan.home, manifest.recovery_id.as_str()).unwrap();
        let manifest_json = fs::read_to_string(backup_dir.join("manifest.json")).unwrap();
        let manifest_value: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
        let rollouts = manifest_value["rollouts"].as_array().unwrap();

        assert_eq!(loaded.rollouts.len(), 2);
        assert!(loaded
            .rollouts
            .iter()
            .all(|rollout| rollout.original_head.len() > 512 * 1024));
        assert!(manifest_json.len() < 32 * 1024);
        assert!(!manifest_json.contains("\"originalHead\""));
        assert!(!manifest_json.contains("\"alignedHead\""));
        assert!(rollouts.iter().all(|rollout| {
            rollout["originalHeadFile"].as_str().is_some()
                && rollout["alignedHeadFile"].as_str().is_some()
        }));
        assert_eq!(
            rollout_provider(&fixture.home().join("sessions/rollout-large-one.jsonl")),
            "legacy"
        );
        assert_eq!(
            rollout_provider(&fixture.home().join("sessions/rollout-large-two.jsonl")),
            "legacy"
        );
    }

    #[test]
    fn recovery_manifest_emits_version_three() {
        let fixture = Fixture::new(
            "alignment-recovery-version-three",
            "model_provider = \"orange\"\n",
        );
        write_rollout(
            &fixture.home().join("sessions/rollout-version-three.jsonl"),
            "legacy",
            "\n",
            false,
        );
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, _) = create_recovery(&fixture.paths, &plan).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(backup_dir.join("manifest.json")).unwrap()).unwrap();

        assert_eq!(value["version"], 3);
        assert!(value.get("rolloutUpgrades").is_some());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recovery_planning_skips_an_upgrade_over_budget_but_keeps_provider_repair() {
        let fixture = Fixture::new(
            "alignment-recovery-planning-budget",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-recovery-planning-budget.jsonl");
        let original = write_generated_legacy_recovery(&rollout, "legacy");
        let original_tail = original
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| original[index + 1..].to_vec())
            .unwrap();

        let unbounded = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let provider_repair_bytes = (unbounded.rollouts[0].raw_head.len() as u64)
            + aligned_rollout_head(&unbounded.rollouts[0].raw_head, "orange")
                .unwrap()
                .len() as u64;
        assert!(unbounded.rollouts[0].recovery_artifact_bytes > provider_repair_bytes);
        let plan = build_alignment_plan_with_artifact_limit(
            &fixture.paths,
            ProcessState::default(),
            provider_repair_bytes,
        )
        .unwrap();

        assert_eq!(plan.rollouts.len(), 1);
        assert!(!plan.rollouts[0].needs_desktop_upgrade);
        assert_eq!(plan.rollouts[0].class, RolloutClass::Repairable);
        let (_, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let mut progress = AlignmentMutationProgress::default();
        let (_, rollouts_updated) =
            apply_alignment(&plan, &manifest, &mut progress, AlignmentFault::None).unwrap();
        assert_eq!(rollouts_updated, 1);
        assert_eq!(rollout_provider(&rollout), "orange");
        assert_eq!(rollout_tail(&rollout), original_tail);
        assert!(!fs::read_to_string(&rollout)
            .unwrap()
            .contains("\"task_started\""));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recovery_planning_reserves_provider_repairs_before_optional_upgrades() {
        let fixture = Fixture::new(
            "alignment-recovery-provider-priority",
            "model_provider = \"orange\"\n",
        );
        let aligned = fixture
            .home()
            .join("sessions/rollout-a-aligned-generated.jsonl");
        let repairable = fixture
            .home()
            .join("sessions/rollout-b-repairable-generated.jsonl");
        write_generated_legacy_recovery(&aligned, "orange");
        write_generated_legacy_recovery(&repairable, "legacy");
        let unbounded = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let aligned_impact = unbounded
            .rollouts
            .iter()
            .find(|rollout| rollout.path.file_name() == aligned.file_name())
            .unwrap();
        let repairable_impact = unbounded
            .rollouts
            .iter()
            .find(|rollout| rollout.path.file_name() == repairable.file_name())
            .unwrap();
        let provider_repair_bytes = (repairable_impact.raw_head.len() as u64)
            + aligned_rollout_head(&repairable_impact.raw_head, "orange")
                .unwrap()
                .len() as u64;
        let limit = aligned_impact
            .recovery_artifact_bytes
            .checked_add(provider_repair_bytes)
            .unwrap()
            - 1;

        let plan = build_alignment_plan_with_artifact_limit(
            &fixture.paths,
            ProcessState::default(),
            limit,
        )
        .unwrap();
        let aligned_impact = plan
            .rollouts
            .iter()
            .find(|rollout| rollout.path.file_name() == aligned.file_name())
            .unwrap();
        let repairable_impact = plan
            .rollouts
            .iter()
            .find(|rollout| rollout.path.file_name() == repairable.file_name())
            .unwrap();

        assert!(!aligned_impact.needs_desktop_upgrade);
        assert_eq!(repairable_impact.class, RolloutClass::Repairable);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recovery_scan_reports_an_aligned_upgrade_skipped_by_the_artifact_budget() {
        let fixture = Fixture::new(
            "alignment-recovery-upgrade-budget-report",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-upgrade-budget-report.jsonl");
        write_generated_legacy_recovery(&rollout, "orange");
        let plan =
            build_alignment_plan_with_artifact_limit(&fixture.paths, ProcessState::default(), 0)
                .unwrap();

        let scan = scan_from_plan(&plan, ProcessState::default());

        assert_eq!(scan.mismatched_rollouts, 0);
        assert_eq!(scan.skipped_rollouts, 1);
        assert_eq!(
            scan.skip_reasons,
            vec!["alignment_recovery_artifact_budget"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recovery_scan_reports_output_overflow_without_losing_provider_repair() {
        let fixture = Fixture::new(
            "alignment-recovery-output-budget-report",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-recovery-output-budget-report.jsonl");
        write_generated_legacy_recovery(&rollout, "legacy");

        let mut impact = scan_rollout_with_limits(&rollout, "orange", 4096, 512).unwrap();
        assert_eq!(impact.class, RolloutClass::Repairable);
        assert!(!impact.needs_desktop_upgrade);
        assert_eq!(
            impact.upgrade_skip_reason,
            Some("alignment_recovery_artifact_budget")
        );

        apply_recovery_artifact_budget(std::slice::from_mut(&mut impact), "orange", u64::MAX)
            .unwrap();
        assert_eq!(impact.class, RolloutClass::Repairable);
        assert!(!impact.needs_desktop_upgrade);
        assert_eq!(
            impact.upgrade_skip_reason,
            Some("alignment_recovery_artifact_budget")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recovery_loader_rejects_upgrade_artifacts_over_the_aggregate_memory_limit() {
        let fixture = Fixture::new(
            "alignment-recovery-aggregate-limit",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions/rollout-recovery-aggregate-limit.jsonl");
        write_generated_legacy_recovery(&rollout, "orange");
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let artifact_bytes = manifest
            .rollout_upgrades
            .iter()
            .flat_map(|upgrade| [&upgrade.original_file, &upgrade.upgraded_file])
            .map(|filename| fs::metadata(backup_dir.join(filename)).unwrap().len())
            .sum::<u64>();
        assert!(artifact_bytes > 1);

        assert!(matches!(
            load_recovery_with_artifact_limit(
                &fixture.paths,
                &plan.home,
                &manifest.recovery_id,
                artifact_bytes - 1,
            ),
            Err(AlignmentError::InvalidRecovery)
        ));
    }

    #[test]
    fn recovery_loader_accepts_legacy_version_two_without_rollout_upgrades() {
        let fixture = Fixture::new(
            "alignment-recovery-version-two",
            "model_provider = \"orange\"\n",
        );
        write_rollout(
            &fixture.home().join("sessions/rollout-version-two.jsonl"),
            "legacy",
            "\n",
            false,
        );
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let manifest_path = backup_dir.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        value["version"] = serde_json::json!(2);
        value.as_object_mut().unwrap().remove("rolloutUpgrades");
        fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        assert!(load_recovery(&fixture.paths, &plan.home, &manifest.recovery_id).is_ok());
    }

    #[test]
    fn recovery_loader_rejects_version_two_with_rollout_upgrades() {
        let fixture = Fixture::new(
            "alignment-recovery-version-two-upgrades",
            "model_provider = \"orange\"\n",
        );
        write_rollout(
            &fixture
                .home()
                .join("sessions/rollout-version-two-upgrades.jsonl"),
            "legacy",
            "\n",
            false,
        );
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let manifest_path = backup_dir.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        value["version"] = serde_json::json!(2);
        fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        assert!(matches!(
            load_recovery(&fixture.paths, &plan.home, &manifest.recovery_id),
            Err(AlignmentError::InvalidRecovery)
        ));
    }

    #[test]
    fn recovery_loader_rejects_invalid_rollout_hash_metadata() {
        let fixture = Fixture::new(
            "alignment-invalid-recovery-hash",
            "model_provider = \"orange\"\n",
        );
        write_rollout(
            &fixture.home().join("sessions/rollout-invalid-hash.jsonl"),
            "legacy",
            "\n",
            false,
        );
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let manifest_path = backup_dir.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        value["rollouts"][0]["originalTailSha256"] =
            serde_json::Value::String("not-a-sha256".to_string());
        fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let error =
            load_recovery(&fixture.paths, &plan.home, manifest.recovery_id.as_str()).unwrap_err();

        assert!(matches!(error, AlignmentError::InvalidRecovery));
    }

    #[test]
    fn recovery_loader_rejects_unsafe_tampered_or_oversized_head_files() {
        let fixture = Fixture::new(
            "alignment-invalid-recovery-head",
            "model_provider = \"orange\"\n",
        );
        write_rollout(
            &fixture.home().join("sessions/rollout-invalid-head.jsonl"),
            "legacy",
            "\n",
            false,
        );
        let plan = build_alignment_plan(&fixture.paths, ProcessState::default()).unwrap();
        let (backup_dir, manifest) = create_recovery(&fixture.paths, &plan).unwrap();
        let manifest_path = backup_dir.join("manifest.json");
        let manifest_bytes = fs::read(&manifest_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        let original_head_file = value["rollouts"][0]["originalHeadFile"]
            .as_str()
            .unwrap()
            .to_string();

        value["rollouts"][0]["originalHeadFile"] =
            serde_json::Value::String("../outside.bin".to_string());
        fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert!(matches!(
            load_recovery(&fixture.paths, &plan.home, &manifest.recovery_id),
            Err(AlignmentError::InvalidRecovery)
        ));

        fs::write(&manifest_path, &manifest_bytes).unwrap();
        fs::write(backup_dir.join(&original_head_file), b"tampered").unwrap();
        assert!(matches!(
            load_recovery(&fixture.paths, &plan.home, &manifest.recovery_id),
            Err(AlignmentError::InvalidRecovery)
        ));

        fs::write(
            backup_dir.join(&original_head_file),
            vec![b'x'; super::MAX_ROLLOUT_HEAD_BYTES as usize + 1],
        )
        .unwrap();
        assert!(matches!(
            load_recovery(&fixture.paths, &plan.home, &manifest.recovery_id),
            Err(AlignmentError::InvalidRecovery)
        ));
    }

    #[test]
    fn alignment_reports_recovery_required_when_rollback_cannot_be_verified() {
        let fixture = Fixture::new("alignment-recovery", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_recovery.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report = execute_alignment_at_with_fault(
            &fixture.paths,
            &scan.scan_token,
            &TestProcessGate,
            AlignmentFault::RollbackVerificationFailure,
        )
        .unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::RecoveryRequired);
        assert!(!report.rollback_verified);
        assert!(!report.elevation_required);
        assert!(!report.restart_allowed);
        assert!(!report.undo_allowed);
        assert!(report
            .backup_dir
            .as_ref()
            .is_some_and(|path| Path::new(path).is_dir()));
    }

    #[test]
    fn alignment_undo_preserves_later_threads_and_rollout_tail_appends() {
        let fixture = Fixture::new("alignment-undo", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_undo.sqlite");
        let rollout = fixture.home().join("sessions/rollout-undo.jsonl");
        create_database(&database, &[Some("legacy")]);
        write_rollout(&rollout, "legacy", "\n", false);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute(
                "INSERT INTO threads (id, model_provider) VALUES (?1, ?2)",
                params!["later-thread", "orange"],
            )
            .unwrap();
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(b"{\"type\":\"response_item\",\"payload\":{\"later\":true}}\n")
            .unwrap();

        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();

        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert!(restored.rollback_verified);
        assert_eq!(
            thread_provider(&database, "thread-0").as_deref(),
            Some("legacy")
        );
        assert_eq!(
            thread_provider(&database, "later-thread").as_deref(),
            Some("orange")
        );
        assert_eq!(rollout_provider(&rollout), "legacy");
        assert!(String::from_utf8(rollout_tail(&rollout))
            .unwrap()
            .contains("\"later\":true"));
    }

    #[test]
    fn recovery_directory_requires_a_valid_existing_manifest() {
        let fixture = Fixture::new("recovery-directory", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_recovery_directory.sqlite");
        create_database(&database, &[Some("legacy")]);
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();
        let recovery_id = committed.recovery_id.as_deref().unwrap();

        assert_eq!(
            recovery_directory_at(&fixture.paths, recovery_id).unwrap(),
            PathBuf::from(committed.backup_dir.unwrap())
        );
        assert!(matches!(
            recovery_directory_at(&fixture.paths, "../outside"),
            Err(AlignmentError::InvalidRecovery)
        ));
        assert!(matches!(
            recovery_directory_at(&fixture.paths, "missing"),
            Err(AlignmentError::InvalidRecovery)
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn private_recovery_directory_replaces_permissive_windows_acl() {
        let fixture = Fixture::new("private-windows-acl", "model_provider = \"orange\"\n");
        let directory = fixture.home().join("private-recovery");
        fs::create_dir(&directory).unwrap();
        let grant = std::process::Command::new("icacls.exe")
            .arg(&directory)
            .args(["/grant", "*S-1-1-0:(OI)(CI)F"])
            .output()
            .unwrap();
        assert!(grant.status.success());

        set_private_directory(&directory).unwrap();

        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "& { param([string]$target) (Get-Acl -LiteralPath $target).Sddl }",
            ])
            .arg(&directory)
            .output()
            .unwrap();
        assert!(output.status.success());
        let sddl = String::from_utf8_lossy(&output.stdout);
        assert!(sddl.contains("D:P"), "DACL must be protected: {sddl}");
        assert!(!sddl.contains(";;;WD)"), "Everyone retained access: {sddl}");
        assert!(sddl.contains(";;;OW)"), "owner access missing: {sddl}");
        assert!(sddl.contains(";;;SY)"), "SYSTEM access missing: {sddl}");
        assert!(
            sddl.contains(";;;BA)"),
            "administrators access missing: {sddl}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn rollout_alignment_and_restore_preserve_windows_dacl() {
        let fixture = Fixture::new(
            "preserve-windows-rollout-acl",
            "model_provider = \"orange\"\n",
        );
        let rollout = fixture
            .home()
            .join("sessions")
            .join("rollout-preserve-acl.jsonl");
        write_rollout(&rollout, "legacy", "\n", false);
        let grant = std::process::Command::new("icacls.exe")
            .arg(&rollout)
            .args(["/grant", "*S-1-1-0:F"])
            .output()
            .unwrap();
        assert!(grant.status.success());
        let read_sddl = || {
            let output = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "& { param([string]$target) (Get-Acl -LiteralPath $target).Sddl }",
                ])
                .arg(&rollout)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        let original_sddl = read_sddl();

        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
        let committed =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(read_sddl(), original_sddl);
        let restored =
            restore_alignment_at(&fixture.paths, committed.recovery_id.as_deref().unwrap())
                .unwrap();
        assert_eq!(restored.outcome, AlignmentOutcome::Restored);
        assert_eq!(read_sddl(), original_sddl);
    }

    #[test]
    fn alignment_backs_up_and_updates_uncheckpointed_wal_rows() {
        let fixture = Fixture::new("alignment-wal", "model_provider = \"orange\"\n");
        let database = fixture.home().join("state_wal.sqlite");
        create_database(&database, &[Some("legacy")]);
        let wal_connection = Connection::open(&database).unwrap();
        let mode: String = wal_connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        wal_connection
            .execute_batch("PRAGMA wal_autocheckpoint = 0;")
            .unwrap();
        wal_connection
            .execute(
                "INSERT INTO threads (id, model_provider) VALUES (?1, ?2)",
                params!["wal-thread", "legacy"],
            )
            .unwrap();
        let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

        let report =
            execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(report.threads_updated, 2);
        let backup = PathBuf::from(report.backup_dir.unwrap()).join("database-1.sqlite");
        let backed_up_threads: i64 = Connection::open(backup)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backed_up_threads, 2);
        assert_eq!(
            thread_provider(&database, "wal-thread").as_deref(),
            Some("orange")
        );
        drop(wal_connection);
    }
}
