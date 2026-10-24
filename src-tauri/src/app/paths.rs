use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const SMOKE_RUN_ENV: &str = "CAM_PACKAGED_SMOKE_RUN";
const SMOKE_DATA_DIR_ENV: &str = "CAM_PACKAGED_SMOKE_DATA_DIR";
const SMOKE_CODEX_HOME_ENV: &str = "CAM_PACKAGED_SMOKE_CODEX_HOME";
const SMOKE_DATA_DIR_PREFIX: &str = "codex-app-manager-smoke-";

#[derive(Debug, PartialEq, Eq)]
enum SmokeDataDir {
    Absent,
    Valid { run_id: String, path: PathBuf },
    Invalid,
}

fn valid_smoke_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 64
        && run_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn validate_smoke_data_dir(
    run_id: Option<OsString>,
    requested: Option<OsString>,
    temp_dir: &Path,
) -> SmokeDataDir {
    let (run_id, requested) = match (run_id, requested) {
        (None, None) => return SmokeDataDir::Absent,
        (Some(run_id), Some(requested)) => (run_id, requested),
        (None, Some(_)) | (Some(_), None) => return SmokeDataDir::Invalid,
    };
    let Some(run_id) = run_id.to_str() else {
        return SmokeDataDir::Invalid;
    };
    if !valid_smoke_run_id(run_id) {
        return SmokeDataDir::Invalid;
    }

    let requested = PathBuf::from(requested);
    let expected_leaf = format!("{SMOKE_DATA_DIR_PREFIX}{run_id}");
    if !requested.is_absolute() || requested.file_name() != Some(OsStr::new(&expected_leaf)) {
        return SmokeDataDir::Invalid;
    }

    let Ok(metadata) = std::fs::symlink_metadata(&requested) else {
        return SmokeDataDir::Invalid;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return SmokeDataDir::Invalid;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return SmokeDataDir::Invalid;
        }
    }

    let (Ok(temp_dir), Ok(requested)) = (temp_dir.canonicalize(), requested.canonicalize()) else {
        return SmokeDataDir::Invalid;
    };
    if requested.parent() != Some(temp_dir.as_path()) {
        return SmokeDataDir::Invalid;
    }

    SmokeDataDir::Valid {
        run_id: run_id.to_string(),
        path: requested,
    }
}

fn smoke_data_dir_from_env() -> SmokeDataDir {
    validate_smoke_data_dir(
        std::env::var_os(SMOKE_RUN_ENV),
        std::env::var_os(SMOKE_DATA_DIR_ENV),
        &std::env::temp_dir(),
    )
}

fn select_data_dir(smoke: SmokeDataDir, production: Option<PathBuf>) -> Option<PathBuf> {
    match smoke {
        SmokeDataDir::Valid { path, .. } => Some(path),
        // If either smoke marker is present but the pair is invalid, fail
        // closed. Falling back to the production directory would let a broken
        // smoke harness read settings or run recovery against real user data.
        SmokeDataDir::Invalid => None,
        SmokeDataDir::Absent => production,
    }
}

fn select_staging_root(smoke: SmokeDataDir, temp_dir: &Path, process_id: u32) -> PathBuf {
    match smoke {
        SmokeDataDir::Valid { path, .. } => path.join("staging"),
        SmokeDataDir::Invalid => temp_dir
            .join(format!("codex-app-manager-smoke-invalid-{process_id}"))
            .join("staging"),
        SmokeDataDir::Absent => temp_dir.join("codex-app-manager").join("staging"),
    }
}

fn select_packaged_smoke_result_path(
    smoke: SmokeDataDir,
    requested_run_id: &str,
) -> Option<PathBuf> {
    match smoke {
        SmokeDataDir::Valid { run_id, path } if run_id == requested_run_id => {
            Some(path.join("alignment-smoke-result.json"))
        }
        SmokeDataDir::Absent | SmokeDataDir::Invalid | SmokeDataDir::Valid { .. } => None,
    }
}

fn select_codex_home(
    override_home: Option<OsString>,
    fallback: Option<PathBuf>,
) -> Option<PathBuf> {
    override_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or(fallback)
}

fn select_codex_home_for_smoke(
    smoke: SmokeDataDir,
    requested: Option<OsString>,
    production: Option<PathBuf>,
) -> Option<PathBuf> {
    let (smoke_root, requested) = match (smoke, requested) {
        (SmokeDataDir::Absent, None) => return production,
        (SmokeDataDir::Valid { path, .. }, Some(requested)) => (path, requested),
        (SmokeDataDir::Absent, Some(_))
        | (SmokeDataDir::Valid { .. }, None)
        | (SmokeDataDir::Invalid, _) => return None,
    };

    let requested = PathBuf::from(requested);
    if !requested.is_absolute() {
        return None;
    }
    let metadata = std::fs::symlink_metadata(&requested).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }

    let requested = requested.canonicalize().ok()?;
    if requested.parent() != Some(smoke_root.as_path()) {
        return None;
    }
    Some(requested)
}

/// Manager data directory shared by settings, provenance, and operation locks.
pub fn data_dir() -> Option<PathBuf> {
    let production = directories::ProjectDirs::from("io.github", "wangnov", "codexappmanager")
        .map(|dirs| dirs.data_dir().to_path_buf());
    select_data_dir(smoke_data_dir_from_env(), production)
}

/// Manager cache directory for re-downloadable, non-critical content (currently
/// catalog preview thumbnails). Safe to clear at any time — distinct from
/// `data_dir`, which holds settings/provenance that must persist.
pub fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "wangnov", "codexappmanager")
        .map(|dirs| dirs.cache_dir().to_path_buf())
}

pub fn packaged_smoke_run_id() -> Option<String> {
    match smoke_data_dir_from_env() {
        SmokeDataDir::Valid { run_id, .. } => Some(run_id),
        SmokeDataDir::Absent | SmokeDataDir::Invalid => None,
    }
}

pub fn packaged_smoke_result_path(run_id: &str) -> Option<PathBuf> {
    select_packaged_smoke_result_path(smoke_data_dir_from_env(), run_id)
}

pub fn staging_root() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    select_staging_root(smoke_data_dir_from_env(), &temp_dir, std::process::id())
}

pub fn settings_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("settings.json"))
}

pub fn provenance_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("provenance.json"))
}

pub fn orange_session_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("orangeapi-session.json"))
}

pub fn orange_backup_root() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("orangeapi-backups"))
}

pub fn codex_home_dir() -> Option<PathBuf> {
    let fallback = directories::UserDirs::new().map(|dirs| dirs.home_dir().join(".codex"));
    let production = select_codex_home(std::env::var_os("CODEX_HOME"), fallback);
    select_codex_home_for_smoke(
        smoke_data_dir_from_env(),
        std::env::var_os(SMOKE_CODEX_HOME_ENV),
        production,
    )
}

/// Default Codex-skin store. macOS keeps it beside the manager's data
/// (Application Support is the platform-correct home for app-managed
/// content); Windows uses the LOCAL app-data root instead of the roaming
/// one — skins are megabytes of re-downloadable content that must not ride
/// a domain roaming profile.
pub fn default_skins_store_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("io.github", "wangnov", "codexappmanager")?;
    if cfg!(target_os = "windows") {
        Some(dirs.data_local_dir().join("themes"))
    } else {
        Some(dirs.data_dir().join("themes"))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{
        select_codex_home, select_codex_home_for_smoke, select_data_dir,
        select_packaged_smoke_result_path, select_staging_root, validate_smoke_data_dir,
        SmokeDataDir, SMOKE_DATA_DIR_PREFIX,
    };

    fn test_run_id() -> String {
        format!("test-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn smoke_data_dir_requires_both_markers_and_an_exact_private_temp_child() {
        let temp_dir = std::env::temp_dir().canonicalize().unwrap();
        let run_id = test_run_id();
        let path = temp_dir.join(format!("{SMOKE_DATA_DIR_PREFIX}{run_id}"));
        std::fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        assert_eq!(
            validate_smoke_data_dir(
                Some(run_id.clone().into()),
                Some(path.clone().into_os_string()),
                &temp_dir,
            ),
            SmokeDataDir::Valid {
                run_id: run_id.clone(),
                path: path.canonicalize().unwrap(),
            }
        );
        assert_eq!(
            validate_smoke_data_dir(Some(run_id.clone().into()), None, &temp_dir),
            SmokeDataDir::Invalid
        );
        assert_eq!(
            validate_smoke_data_dir(None, Some(path.clone().into_os_string()), &temp_dir),
            SmokeDataDir::Invalid
        );
        assert_eq!(
            validate_smoke_data_dir(None, None, &temp_dir),
            SmokeDataDir::Absent
        );
        assert_eq!(
            validate_smoke_data_dir(
                Some("../invalid".into()),
                Some(path.clone().into_os_string()),
                &temp_dir,
            ),
            SmokeDataDir::Invalid
        );
        assert_eq!(
            validate_smoke_data_dir(
                Some(run_id.into()),
                Some(temp_dir.join("wrong-leaf").into_os_string()),
                &temp_dir,
            ),
            SmokeDataDir::Invalid
        );

        std::fs::remove_dir(&path).unwrap();
    }

    #[test]
    fn invalid_smoke_override_never_falls_back_to_production_data() {
        let production = std::env::temp_dir().join("real-manager-data");
        assert_eq!(
            select_data_dir(SmokeDataDir::Invalid, Some(production.clone())),
            None
        );
        assert_eq!(
            select_data_dir(SmokeDataDir::Absent, Some(production.clone())),
            Some(production)
        );

        let temp_dir = std::env::temp_dir();
        let production_staging = temp_dir.join("codex-app-manager").join("staging");
        let invalid_staging = select_staging_root(SmokeDataDir::Invalid, &temp_dir, 1234);
        assert_ne!(invalid_staging, production_staging);
        assert_eq!(
            invalid_staging,
            temp_dir
                .join("codex-app-manager-smoke-invalid-1234")
                .join("staging")
        );
    }

    #[test]
    fn packaged_smoke_result_path_requires_the_active_run_id() {
        let root = PathBuf::from("isolated-smoke-root");
        let smoke = SmokeDataDir::Valid {
            run_id: "run-1".to_string(),
            path: root.clone(),
        };

        assert_eq!(
            select_packaged_smoke_result_path(smoke, "run-1"),
            Some(root.join("alignment-smoke-result.json"))
        );
        assert_eq!(
            select_packaged_smoke_result_path(
                SmokeDataDir::Valid {
                    run_id: "run-1".to_string(),
                    path: root,
                },
                "run-2",
            ),
            None
        );
        assert_eq!(
            select_packaged_smoke_result_path(SmokeDataDir::Invalid, "run-1"),
            None
        );
    }

    #[test]
    fn codex_home_prefers_a_valid_absolute_override() {
        let fallback = std::env::temp_dir().join("fallback-codex-home");
        let override_home = std::env::temp_dir().join("custom-codex-home");

        assert_eq!(
            select_codex_home(Some(override_home.clone().into_os_string()), Some(fallback)),
            Some(override_home)
        );
    }

    #[test]
    fn codex_home_ignores_empty_and_relative_overrides() {
        let fallback = std::env::temp_dir().join("fallback-codex-home");

        assert_eq!(
            select_codex_home(Some(OsString::new()), Some(fallback.clone())),
            Some(fallback.clone())
        );
        assert_eq!(
            select_codex_home(
                Some(OsString::from("relative-home")),
                Some(fallback.clone())
            ),
            Some(fallback)
        );
    }

    #[test]
    fn packaged_smoke_codex_home_requires_a_direct_private_child() {
        let temp_dir = std::env::temp_dir().canonicalize().unwrap();
        let run_id = test_run_id();
        let smoke_root = temp_dir.join(format!("{SMOKE_DATA_DIR_PREFIX}{run_id}"));
        let smoke_home = smoke_root.join("codex-home");
        std::fs::create_dir(&smoke_root).unwrap();
        std::fs::create_dir(&smoke_home).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&smoke_root, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(&smoke_home, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let smoke = validate_smoke_data_dir(
            Some(run_id.into()),
            Some(smoke_root.clone().into_os_string()),
            &temp_dir,
        );
        assert_eq!(
            select_codex_home_for_smoke(
                smoke,
                Some(smoke_home.clone().into_os_string()),
                Some(PathBuf::from("real-codex-home")),
            ),
            Some(smoke_home.canonicalize().unwrap())
        );

        std::fs::remove_dir(smoke_home).unwrap();
        std::fs::remove_dir(smoke_root).unwrap();
    }

    #[test]
    fn invalid_smoke_codex_home_never_falls_back_to_production() {
        let production = Some(PathBuf::from("real-codex-home"));

        assert_eq!(
            select_codex_home_for_smoke(SmokeDataDir::Invalid, None, production.clone()),
            None
        );
        assert_eq!(
            select_codex_home_for_smoke(SmokeDataDir::Absent, Some("stray".into()), production),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn smoke_data_dir_rejects_symlinks_and_group_access() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp_dir = std::env::temp_dir().canonicalize().unwrap();
        let target_run = test_run_id();
        let target = temp_dir.join(format!("{SMOKE_DATA_DIR_PREFIX}{target_run}"));
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(
            validate_smoke_data_dir(
                Some(target_run.into()),
                Some(target.clone().into_os_string()),
                &temp_dir,
            ),
            SmokeDataDir::Invalid
        );

        let link_run = test_run_id();
        let link = temp_dir.join(format!("{SMOKE_DATA_DIR_PREFIX}{link_run}"));
        symlink(&target, &link).unwrap();
        assert_eq!(
            validate_smoke_data_dir(
                Some(link_run.into()),
                Some(link.clone().into_os_string()),
                &temp_dir,
            ),
            SmokeDataDir::Invalid
        );

        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir(target).unwrap();
    }
}
