//! In-process theme keeper. The studio ran a detached Node watcher polling a
//! state file; inside the manager the same loop is a tokio task fed by a
//! `watch` channel, reconciling *desired* state (the directive) against
//! *actual* state (the stamp each renderer carries):
//!
//! - every ~900 ms, list `app://` targets, connect/probe new ones, drop dead
//!   ones — quick-chat windows and prewarm shells get themed as they appear;
//! - per session, read `window.__CODEX_THEME_STUDIO__?.stamp` and re-inject /
//!   remove only on mismatch (the payload is itself idempotent, this just
//!   avoids pointless 30 MB evaluates). A page reload resets the stamp to
//!   null, so recovery is at most one tick behind — replacing the studio's
//!   `Page.loadEventFired` + 300 ms dance with something stateless.
//!
//! Dropping the directive sender stops the daemon **without** un-theming: the
//! manager quitting must not strip a running Codex (the injected runtime
//! keeps the theme alive until reload; only an explicit `None` directive
//! removes it).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;

use crate::cdp::{is_theme_excluded_target, list_app_targets, probe_session, CdpSession};
use crate::locale::{
    build_zh_cn_payload, LocalePayload, CURRENT_LOCALE_REPORT_EXPRESSION, REMOVE_LOCALE_EXPRESSION,
};
use crate::payload::{build_payload, BuiltPayload, CURRENT_STAMP_EXPRESSION, REMOVE_EXPRESSION};

const TICK: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub running: bool,
    pub port: u16,
    pub theme_id: Option<String>,
    pub stamp: Option<String>,
    pub locale_stamp: Option<String>,
    pub translated_nodes: usize,
    pub connected_targets: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleDirective {
    ZhCn,
}

/// Theme and locale are independent desired states. Updating either field
/// must preserve the other so theme removal cannot disable localization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directive {
    pub theme_dir: Option<PathBuf>,
    pub locale: Option<LocaleDirective>,
}

impl Directive {
    pub fn with_theme(&self, theme_dir: Option<PathBuf>) -> Self {
        Self {
            theme_dir,
            locale: self.locale,
        }
    }

    pub fn with_locale(&self, locale: Option<LocaleDirective>) -> Self {
        Self {
            theme_dir: self.theme_dir.clone(),
            locale,
        }
    }
}

async fn reconcile_independently<T, L>(
    theme: impl std::future::Future<Output = T>,
    locale: impl std::future::Future<Output = L>,
) -> (T, L) {
    let theme_result = theme.await;
    let locale_result = locale.await;
    (theme_result, locale_result)
}

pub async fn run_daemon(
    port: u16,
    mut directive_rx: watch::Receiver<Directive>,
    status_tx: watch::Sender<DaemonStatus>,
) {
    let mut sessions: HashMap<String, CdpSession> = HashMap::new();
    // Keep excluded pet renderers connected after cleaning them once. Closing
    // them would make the census reconnect and repeat cleanup every 900 ms.
    let mut excluded_sessions: HashMap<String, CdpSession> = HashMap::new();
    let mut built: Option<BuiltPayload>;
    let mut last_error: Option<String> = None;

    // Build the initial payload for whatever directive we started with.
    let mut rebuild_for = directive_rx.borrow().clone();
    loop {
        if let Some(dir) = &rebuild_for.theme_dir {
            match build_payload(dir) {
                Ok(payload) => {
                    log::info!(
                        "theme payload built id={} bytes={} assets={}",
                        payload.theme.id,
                        payload.payload_bytes,
                        payload.asset_count
                    );
                    built = Some(payload);
                }
                Err(error) => {
                    log::warn!("theme payload build failed: {error}");
                    last_error = Some(error.to_string());
                    built = None;
                }
            }
        } else {
            built = None;
        }
        let locale_built: Option<LocalePayload> = match rebuild_for.locale {
            Some(LocaleDirective::ZhCn) => match build_zh_cn_payload() {
                Ok(payload) => Some(payload),
                Err(error) => {
                    log::warn!("locale payload build failed: {error}");
                    last_error = Some(error.to_string());
                    None
                }
            },
            None => None,
        };

        // Reconcile until the directive changes (or the sender goes away).
        loop {
            // 1. Target census.
            match list_app_targets(port).await {
                Ok(targets) => {
                    let active: std::collections::HashSet<&str> =
                        targets.iter().map(|t| t.id.as_str()).collect();
                    sessions.retain(|id, session| {
                        let keep = active.contains(id.as_str()) && !session.closed();
                        if !keep {
                            session.close();
                        }
                        keep
                    });
                    excluded_sessions.retain(|id, session| {
                        let keep = active.contains(id.as_str()) && !session.closed();
                        if !keep {
                            session.close();
                        }
                        keep
                    });
                    for target in targets {
                        if sessions.contains_key(&target.id)
                            || excluded_sessions.contains_key(&target.id)
                        {
                            continue;
                        }
                        let id = target.id.clone();
                        match CdpSession::connect(target, port).await {
                            Ok(session) => match probe_session(&session).await {
                                Ok(probe) if probe.codex => {
                                    if is_theme_excluded_target(&session.target) {
                                        let cleanup = async {
                                            session.evaluate(REMOVE_EXPRESSION).await?;
                                            session.evaluate(REMOVE_LOCALE_EXPRESSION).await?;
                                            Ok::<(), crate::ThemeEngineError>(())
                                        }
                                        .await;
                                        match cleanup {
                                            Ok(_) => {
                                                log::info!(
                                                    "renderer daemon excluded pet target {id}"
                                                );
                                                excluded_sessions.insert(id, session);
                                            }
                                            Err(error) => {
                                                session.close();
                                                last_error = Some(error.to_string());
                                            }
                                        }
                                    } else {
                                        log::info!("theme daemon connected target {id}");
                                        sessions.insert(id, session);
                                    }
                                }
                                Ok(_) => session.close(),
                                Err(error) => {
                                    session.close();
                                    last_error = Some(error.to_string());
                                }
                            },
                            Err(error) => last_error = Some(error.to_string()),
                        }
                    }
                }
                Err(error) => {
                    // Codex not running (or CDP gone): drop every session and
                    // report quietly — this is a normal resting state.
                    for session in sessions.values() {
                        session.close();
                    }
                    sessions.clear();
                    for session in excluded_sessions.values() {
                        session.close();
                    }
                    excluded_sessions.clear();
                    last_error = Some(error.to_string());
                }
            }

            // 2. Stamp reconciliation per session.
            let expected = built.as_ref().map(|b| b.stamp.clone());
            let expected_locale = locale_built.as_ref().map(|payload| payload.stamp.clone());
            let mut dead = Vec::new();
            let mut translated_nodes = 0usize;
            for (id, session) in &sessions {
                let theme_work = async {
                    let current = session
                        .evaluate(CURRENT_STAMP_EXPRESSION)
                        .await
                        .map_err(|error| format!("theme stamp probe failed: {error}"))?
                        .as_str()
                        .map(str::to_owned);
                    match (&expected, &current) {
                        (Some(stamp), Some(applied)) if stamp == applied => Ok(()),
                        (Some(_), _) => session
                            .evaluate(&built.as_ref().expect("built payload").payload)
                            .await
                            .map(|_| ())
                            .map_err(|error| format!("theme apply failed: {error}")),
                        (None, Some(_)) => session
                            .evaluate(REMOVE_EXPRESSION)
                            .await
                            .map(|_| ())
                            .map_err(|error| format!("theme removal failed: {error}")),
                        (None, None) => Ok(()),
                    }
                };
                let locale_work = async {
                    let report = session
                        .evaluate(CURRENT_LOCALE_REPORT_EXPRESSION)
                        .await
                        .map_err(|error| format!("locale probe failed: {error}"))?;
                    let current_locale = report
                        .get("stamp")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned);
                    let observer_active = report
                        .get("observerActive")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false);
                    let current_translated_nodes = report
                        .get("translatedNodes")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as usize;
                    match (&expected_locale, &current_locale) {
                        (Some(stamp), Some(applied)) if stamp == applied && observer_active => {
                            Ok(())
                        }
                        (Some(_), _) => session
                            .evaluate(&locale_built.as_ref().expect("locale payload").payload)
                            .await
                            .map(|_| ())
                            .map_err(|error| format!("locale apply failed: {error}")),
                        (None, Some(_)) => session
                            .evaluate(REMOVE_LOCALE_EXPRESSION)
                            .await
                            .map(|_| ())
                            .map_err(|error| format!("locale removal failed: {error}")),
                        (None, None) => Ok(()),
                    }
                    .map(|()| current_translated_nodes)
                };
                let (theme_result, locale_result) =
                    reconcile_independently(theme_work, locale_work).await;
                let mut session_failed = false;
                if let Err(error) = theme_result {
                    log::warn!("theme reconciliation failed for {id}: {error}");
                    last_error = Some(error);
                    session_failed = true;
                }
                match locale_result {
                    Ok(count) => translated_nodes += count,
                    Err(error) => {
                        log::warn!("locale reconciliation failed for {id}: {error}");
                        last_error = Some(error);
                        session_failed = true;
                    }
                }
                if session_failed {
                    dead.push(id.clone());
                }
            }
            for id in dead {
                if let Some(session) = sessions.remove(&id) {
                    session.close();
                }
            }

            let _ = status_tx.send(DaemonStatus {
                running: true,
                port,
                theme_id: built.as_ref().map(|b| b.theme.id.clone()),
                stamp: expected,
                locale_stamp: expected_locale,
                translated_nodes,
                connected_targets: sessions.len(),
                last_error: last_error.clone(),
            });

            // 3. Wait one tick, reacting immediately to directive changes.
            tokio::select! {
                changed = directive_rx.changed() => {
                    if changed.is_err() {
                        // Manager shutting down: leave renderers as they are.
                        for session in sessions.values() {
                            session.close();
                        }
                        for session in excluded_sessions.values() {
                            session.close();
                        }
                        let _ = status_tx.send(DaemonStatus {
                            running: false,
                            port,
                            ..DaemonStatus::default()
                        });
                        return;
                    }
                    rebuild_for = directive_rx.borrow().clone();
                    last_error = None;
                    break; // outer loop rebuilds the payload
                }
                _ = tokio::time::sleep(TICK) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn changing_theme_does_not_clear_locale() {
        let state = Directive {
            theme_dir: None,
            locale: Some(LocaleDirective::ZhCn),
        };
        let changed = state.with_theme(Some(PathBuf::from("orange")));
        assert_eq!(changed.locale, Some(LocaleDirective::ZhCn));
        assert_eq!(changed.theme_dir, Some(PathBuf::from("orange")));
    }

    #[test]
    fn clearing_locale_does_not_clear_theme() {
        let state = Directive {
            theme_dir: Some(PathBuf::from("orange")),
            locale: Some(LocaleDirective::ZhCn),
        };
        let changed = state.with_locale(None);
        assert_eq!(changed.theme_dir, state.theme_dir);
        assert_eq!(changed.locale, None);
    }

    #[tokio::test]
    async fn locale_reconciliation_runs_after_theme_failure() {
        let locale_ran = AtomicBool::new(false);

        let (theme_result, locale_result) =
            reconcile_independently(async { Err::<(), _>("theme failed") }, async {
                locale_ran.store(true, Ordering::SeqCst);
                Ok::<_, &str>(7usize)
            })
            .await;

        assert!(theme_result.is_err());
        assert_eq!(locale_result.unwrap(), 7);
        assert!(locale_ran.load(Ordering::SeqCst));
    }
}
