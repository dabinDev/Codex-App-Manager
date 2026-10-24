use super::conversation_alignment::{AlignmentOutcome, AlignmentReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlignmentContinuation {
    ReturnReport,
    ElevateAlign,
    ElevateRestoreThenAlign,
    StopRecoveryRequired,
}

pub(crate) fn alignment_continuation(report: &AlignmentReport) -> AlignmentContinuation {
    let permission_failure = report.elevation_required
        && report.error_code.as_deref() == Some("alignment_permission_denied");
    match report.outcome {
        AlignmentOutcome::RecoveryRequired => {
            if permission_failure && report.recovery_id.is_some() {
                AlignmentContinuation::ElevateRestoreThenAlign
            } else {
                AlignmentContinuation::StopRecoveryRequired
            }
        }
        AlignmentOutcome::StalePlan if permission_failure && report.recovery_id.is_none() => {
            AlignmentContinuation::ElevateAlign
        }
        AlignmentOutcome::Restored if permission_failure && report.rollback_verified => {
            AlignmentContinuation::ElevateAlign
        }
        AlignmentOutcome::Committed
        | AlignmentOutcome::NoChanges
        | AlignmentOutcome::StalePlan
        | AlignmentOutcome::Restored => AlignmentContinuation::ReturnReport,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartCommandPolicy {
    ConfirmedManual,
    RejectConfirmation,
    Busy,
}

pub(crate) fn restart_command_policy(
    operation_active: bool,
    confirmed: bool,
) -> RestartCommandPolicy {
    if !confirmed {
        RestartCommandPolicy::RejectConfirmation
    } else if operation_active {
        RestartCommandPolicy::Busy
    } else {
        RestartCommandPolicy::ConfirmedManual
    }
}

#[cfg(test)]
mod tests {
    use super::super::conversation_alignment::{AlignmentOutcome, AlignmentReport};
    use super::{
        alignment_continuation, restart_command_policy, AlignmentContinuation, RestartCommandPolicy,
    };

    fn report(
        outcome: AlignmentOutcome,
        elevation_required: bool,
        rollback_verified: bool,
        recovery_id: Option<&str>,
        error_code: Option<&str>,
    ) -> AlignmentReport {
        AlignmentReport {
            outcome,
            recovery_id: recovery_id.map(str::to_string),
            backup_dir: None,
            threads_updated: 0,
            rollouts_updated: 0,
            rollouts_skipped: 0,
            write_verified: false,
            rollback_verified,
            codex_was_running: false,
            restart_allowed: false,
            undo_allowed: false,
            elevation_required,
            error_code: error_code.map(str::to_string),
        }
    }

    #[test]
    fn elevation_is_allowed_only_after_verified_permission_failure() {
        let before_write = report(
            AlignmentOutcome::StalePlan,
            true,
            false,
            None,
            Some("alignment_permission_denied"),
        );
        let rolled_back = report(
            AlignmentOutcome::Restored,
            true,
            true,
            Some("recovery-1"),
            Some("alignment_permission_denied"),
        );
        let restore_then_align = report(
            AlignmentOutcome::RecoveryRequired,
            true,
            false,
            Some("recovery-2"),
            Some("alignment_permission_denied"),
        );
        let corrupt_rollback = report(
            AlignmentOutcome::RecoveryRequired,
            false,
            false,
            Some("recovery-3"),
            Some("alignment_mutation_failed"),
        );

        assert_eq!(
            alignment_continuation(&before_write),
            AlignmentContinuation::ElevateAlign
        );
        assert_eq!(
            alignment_continuation(&rolled_back),
            AlignmentContinuation::ElevateAlign
        );
        assert_eq!(
            alignment_continuation(&restore_then_align),
            AlignmentContinuation::ElevateRestoreThenAlign
        );
        assert_eq!(
            alignment_continuation(&corrupt_rollback),
            AlignmentContinuation::StopRecoveryRequired
        );
    }

    #[test]
    fn manual_force_restart_requires_confirmation_and_an_idle_operation_slot() {
        assert_eq!(
            restart_command_policy(false, true),
            RestartCommandPolicy::ConfirmedManual
        );
        assert_eq!(
            restart_command_policy(false, false),
            RestartCommandPolicy::RejectConfirmation
        );
        assert_eq!(
            restart_command_policy(true, true),
            RestartCommandPolicy::Busy
        );
    }
}
